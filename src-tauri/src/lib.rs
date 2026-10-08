mod android;
mod backup;
mod filters;
mod manifest;
mod phone;
mod profile;
mod rsync;
mod scan;
mod snapshots;
mod transfer;
mod volumes;

use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

/// A dry run is reused by the next backup of the same, unchanged profile within this window, so the source is not scanned twice.
const PLAN_REUSE_WINDOW: Duration = Duration::from_secs(120);

struct ActiveRun {
  profile_id: String,
  control: backup::RunControl,
}

struct CachedPlan {
  fingerprint: String,
  created: Instant,
  plan: Arc<backup::PreparedPlan>,
}

#[derive(Default)]
struct AppState {
  runs: Mutex<HashMap<String, ActiveRun>>,
  last_plan: Mutex<Option<CachedPlan>>,
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
  app.path().app_data_dir().map_err(|error| error.to_string())
}

fn fingerprint(profile: &profile::BackupProfile) -> String {
  serde_json::to_string(profile).unwrap_or_default()
}

/// Runs blocking file system work off the main thread so the window stays responsive.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
  tauri::async_runtime::spawn_blocking(work).await.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn load_profiles(app: AppHandle) -> Result<Vec<Value>, String> {
  let directory = data_dir(&app)?;
  blocking(move || profile::ProfileStore::new(&directory).load(&directory)).await
}

#[tauri::command]
async fn save_profiles(app: AppHandle, profiles: Vec<Value>) -> Result<(), String> {
  let directory = data_dir(&app)?;
  blocking(move || profile::ProfileStore::new(&directory).save(&profiles)).await
}

#[tauri::command]
async fn forget_job_history(app: AppHandle, state: State<'_, AppState>, profile_id: String) -> Result<(), String> {
  if state.runs.lock().map_err(|error| error.to_string())?.values().any(|run| run.profile_id == profile_id) {
    return Err("This job is running. Stop it before removing it.".to_string());
  }
  let manifest = manifest::Manifest::open(&data_dir(&app)?)?;
  blocking(move || manifest.forget(&profile_id)).await
}

#[tauri::command]
async fn list_presets(app: AppHandle) -> Result<Vec<Value>, String> {
  let directory = data_dir(&app)?;
  blocking(move || Ok(profile::presets(&directory))).await
}

#[tauri::command]
async fn plan_backup(app: AppHandle, state: State<'_, AppState>, profile: profile::BackupProfile) -> Result<backup::BackupPlan, String> {
  let manifest = manifest::Manifest::open(&data_dir(&app)?)?;
  let key = fingerprint(&profile);
  let prepared = Arc::new(blocking(move || backup::plan(&profile, &manifest)).await?);
  let plan = prepared.plan.clone();
  *state.last_plan.lock().map_err(|error| error.to_string())? = Some(CachedPlan { fingerprint: key, created: Instant::now(), plan: prepared });
  Ok(plan)
}

#[tauri::command]
async fn run_backup(app: AppHandle, state: State<'_, AppState>, profile: profile::BackupProfile, run_id: String) -> Result<backup::BackupRunResult, String> {
  let manifest = manifest::Manifest::open(&data_dir(&app)?)?;
  let control = backup::RunControl::new();
  {
    let mut runs = state.runs.lock().map_err(|error| error.to_string())?;
    if runs.values().any(|run| run.profile_id == profile.id) {
      return Err("This job is already running.".to_string());
    }
    runs.insert(run_id.clone(), ActiveRun { profile_id: profile.id.clone(), control: control.clone() });
  }

  let key = fingerprint(&profile);
  let cached = state
    .last_plan
    .lock()
    .map_err(|error| error.to_string())?
    .take()
    .filter(|cached| cached.fingerprint == key && cached.created.elapsed() < PLAN_REUSE_WINDOW)
    .map(|cached| cached.plan);

  let worker_app = app.clone();
  let worker_id = run_id.clone();
  let result = blocking(move || {
    backup::run(profile, manifest, control, worker_id, cached, &|progress| {
      let _ = worker_app.emit("backup-progress", progress);
    })
  })
  .await;
  state.runs.lock().map_err(|error| error.to_string())?.remove(&run_id);
  result
}

fn control_run(app: &AppHandle, state: &AppState, run_id: String, event: &str, action: impl Fn(&backup::RunControl)) -> Result<(), String> {
  if let Some(run) = state.runs.lock().map_err(|error| error.to_string())?.get(&run_id) {
    action(&run.control);
  }
  let _ = app.emit("backup-progress", backup::BackupProgress::control_event(run_id, event));
  Ok(())
}

#[tauri::command]
fn pause_backup(app: AppHandle, state: State<'_, AppState>, run_id: String) -> Result<(), String> {
  control_run(&app, &state, run_id, "paused", backup::RunControl::pause)
}

#[tauri::command]
fn resume_backup(app: AppHandle, state: State<'_, AppState>, run_id: String) -> Result<(), String> {
  control_run(&app, &state, run_id, "resumed", backup::RunControl::resume)
}

#[tauri::command]
fn cancel_backup(app: AppHandle, state: State<'_, AppState>, run_id: String) -> Result<(), String> {
  control_run(&app, &state, run_id, "cancelled", backup::RunControl::cancel)
}

#[tauri::command]
async fn list_qnap_directories(profile: profile::BackupProfile) -> Result<Vec<String>, String> {
  blocking(move || rsync::list_directories(&profile)).await
}

#[tauri::command]
async fn detect_volumes() -> Result<Vec<volumes::MountedVolume>, String> {
  blocking(|| Ok(volumes::detect())).await
}

#[tauri::command]
async fn check_volume_health(path: String, level: String) -> Result<volumes::VolumeHealth, String> {
  blocking(move || Ok(volumes::health(path, level))).await
}

#[tauri::command]
async fn inspect_destination(path: String) -> Result<volumes::DestinationInfo, String> {
  blocking(move || Ok(volumes::inspect_destination(&path))).await
}

#[tauri::command]
async fn create_job_volume(path: String, name: String) -> Result<String, String> {
  blocking(move || volumes::create_job_volume(&path, &name)).await
}

#[tauri::command]
async fn list_snapshots(destination_root: String) -> Result<Vec<snapshots::Snapshot>, String> {
  blocking(move || Ok(snapshots::list(std::path::Path::new(&destination_root)))).await
}

/// Shows a snapshot or backup folder in Finder.
#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
  if !std::path::Path::new(&path).exists() {
    return Err("That folder is not available. Is the drive connected?".to_string());
  }
  let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
  std::process::Command::new(opener).arg(&path).status().map(|_| ()).map_err(|error| error.to_string())
}

#[tauri::command]
async fn detect_android_devices() -> Result<Vec<android::AndroidDevice>, String> {
  blocking(android::detect).await
}

#[tauri::command]
async fn list_android_storages(device_id: String) -> Result<Vec<android::AndroidStorage>, String> {
  android::list_storages(&device_id).await
}

#[tauri::command]
async fn send_to_android(app: AppHandle, request: android::SendRequest) -> Result<android::AndroidSendResult, String> {
  let directory = data_dir(&app)?;
  let remember = (!request.dry_run).then(|| {
    (request.device_id.clone(), android::PhoneSettings { source: Some(request.source.clone()), album: request.album.clone(), storage_id: request.storage_id })
  });
  let result = android::send(request, &directory, |progress| { let _ = app.emit("android-progress", progress); }).await?;
  if let Some((device_id, settings)) = remember {
    android::save_settings(&directory, &device_id, settings)?;
  }
  Ok(result)
}

#[tauri::command]
fn cancel_android_send() {
  android::cancel_send();
}

#[tauri::command]
fn free_android_device() -> Result<(), String> {
  android::free_device()
}

#[tauri::command]
fn load_android_settings(app: AppHandle, device_id: String) -> Result<android::PhoneSettings, String> {
  Ok(android::load_settings(&data_dir(&app)?, &device_id))
}

#[tauri::command]
fn save_android_settings(app: AppHandle, device_id: String, settings: android::PhoneSettings) -> Result<(), String> {
  android::save_settings(&data_dir(&app)?, &device_id, settings)
}

#[tauri::command]
async fn list_android_albums(device_id: String, storage_id: Option<u64>) -> Result<Vec<android::AndroidAlbum>, String> {
  android::list_albums(&device_id, storage_id).await
}

#[tauri::command]
async fn create_android_album(device_id: String, storage_id: Option<u64>, name: String) -> Result<(), String> {
  android::create_album(&device_id, storage_id, &name).await
}

#[tauri::command]
async fn rename_android_album(device_id: String, storage_id: Option<u64>, name: String, new_name: String) -> Result<(), String> {
  android::rename_album(&device_id, storage_id, &name, &new_name).await
}

#[tauri::command]
async fn android_device_name(device_id: String) -> Option<String> {
  android::display_name(&device_id).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .manage(AppState::default())
    .plugin(tauri_plugin_dialog::init())
    .invoke_handler(tauri::generate_handler![
      app_info,
      load_profiles,
      save_profiles,
      forget_job_history,
      list_presets,
      plan_backup,
      run_backup,
      pause_backup,
      resume_backup,
      cancel_backup,
      list_qnap_directories,
      detect_volumes,
      check_volume_health,
      inspect_destination,
      create_job_volume,
      list_snapshots,
      reveal_path,
      detect_android_devices,
      list_android_storages,
      send_to_android,
      cancel_android_send,
      free_android_device,
      load_android_settings,
      save_android_settings,
      list_android_albums,
      create_android_album,
      rename_android_album,
      android_device_name
    ])
    .setup(|app| {
      let handle = app.handle().clone();
      android::watch(move |devices| { let _ = handle.emit("android-devices", devices); });
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }
      Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while building tauri application");
}

#[tauri::command]
fn app_info() -> String {
  format!("{} {}", std::env::consts::OS, env!("CARGO_PKG_VERSION"))
}
