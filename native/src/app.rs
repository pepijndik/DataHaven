//! App state and everything that happens off the UI thread. Slow work (scanning, copying, phone
//! access, health checks) runs on worker threads and reports back through one channel, so the
//! window never waits for the disk.

use crate::model::{self, Job, Qnap, Source};
use crate::phone::PhonePanel;
use crate::style;
use backup_engine::android::{self, AndroidDevice};
use backup_engine::backup::{self, BackupPlan, BackupProgress, BackupRunResult, PreparedPlan, RunControl};
use backup_engine::manifest::Manifest;
use backup_engine::snapshots::{self, Snapshot};
use backup_engine::volumes::{self, DestinationInfo, MountedVolume, VolumeHealth};
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A dry run is reused by the next backup of the same, unchanged job within this window, so the source is not scanned twice.
const PLAN_REUSE_WINDOW: Duration = Duration::from_secs(120);
const SAVE_DELAY: Duration = Duration::from_millis(400);
const VOLUME_POLL: Duration = Duration::from_secs(4);
const AUTO_RUN_TICK: Duration = Duration::from_secs(60);
/// A drive that is plugged in and out quickly should not start a backup every time.
const RECONNECT_GRACE_SECONDS: u64 = 15 * 60;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The selected backup.
    Backup,
    NewBackup,
    Drives,
    Phone,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Status,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Busy {
    Profiles,
    Volumes,
    Plan,
    Run,
    Health,
    QnapFolders,
}

pub enum Event {
    Jobs(Result<Vec<Job>, String>),
    Volumes(Vec<MountedVolume>, bool),
    Phones(Vec<AndroidDevice>),
    Plan(Result<(String, Arc<PreparedPlan>), String>),
    Progress(BackupProgress),
    RunDone { job_id: String, result: Result<BackupRunResult, String> },
    Health(VolumeHealth),
    QnapFolders(Result<Vec<String>, String>),
    Destination { root: String, info: DestinationInfo, snapshots: Vec<Snapshot> },
    VolumeCreated { job_id: String, name: String, result: Result<String, String> },
    JobForgotten { job_id: String, result: Result<(), String> },
    Saved(Result<(), String>),
    Phone(crate::phone::PhoneEvent),
}

pub struct ActiveRun {
    pub id: String,
    pub job_id: String,
    pub control: RunControl,
    pub paused: bool,
}

struct CachedPlan {
    fingerprint: String,
    created: Instant,
    plan: Arc<PreparedPlan>,
}

/// What the destination of a job looks like, loaded once per path.
#[derive(Default)]
pub struct DestinationState {
    pub info: Option<DestinationInfo>,
    pub snapshots: Vec<Snapshot>,
}

/// Per job editor state that is not saved.
#[derive(Default)]
pub struct EditorState {
    pub job_id: String,
    pub volume_name: Option<String>,
    pub volume_status: Option<String>,
    pub creating_volume: bool,
    pub filter_input: String,
    pub advanced_open: bool,
}

pub struct App {
    pub ctx: egui::Context,
    pub sender: Sender<Event>,
    receiver: Receiver<Event>,
    pub data_dir: PathBuf,
    pub view: View,
    pub tab: Tab,
    pub jobs: Vec<Job>,
    pub jobs_loaded: bool,
    pub selected: String,
    pub volumes: Vec<MountedVolume>,
    pub phones: Vec<AndroidDevice>,
    pub plan: Option<BackupPlan>,
    pub run_result: Option<BackupRunResult>,
    pub busy: Option<Busy>,
    pub run: Option<ActiveRun>,
    pub progress: Option<BackupProgress>,
    pub health: Option<VolumeHealth>,
    pub message: String,
    pub qnap_folders: Vec<String>,
    pub destinations: HashMap<String, DestinationState>,
    destinations_loading: HashSet<String>,
    pub editor: EditorState,
    pub phone: PhonePanel,
    /// The open New backup card, if any.
    pub wizard: Option<crate::wizard::Wizard>,
    pub icon: Option<egui::TextureHandle>,
    last_plan: Option<CachedPlan>,
    dirty_since: Option<Instant>,
    was_available: HashMap<String, bool>,
    last_auto_tick: Instant,
}

impl App {
    pub fn new(ctx: egui::Context) -> Self {
        Self::build(ctx, model::data_dir(), true)
    }

    /// `watch` starts the drive and phone watchers; tests leave them off.
    pub fn build(ctx: egui::Context, data_dir: PathBuf, watch: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut app = Self {
            ctx: ctx.clone(),
            sender,
            receiver,
            data_dir,
            view: View::Backup,
            tab: Tab::Status,
            jobs: Vec::new(),
            jobs_loaded: false,
            selected: String::new(),
            volumes: Vec::new(),
            phones: Vec::new(),
            plan: None,
            run_result: None,
            busy: None,
            run: None,
            progress: None,
            health: None,
            message: "Ready for a backup".to_string(),
            qnap_folders: Vec::new(),
            destinations: HashMap::new(),
            destinations_loading: HashSet::new(),
            editor: EditorState::default(),
            phone: PhonePanel::default(),
            wizard: None,
            icon: None,
            last_plan: None,
            dirty_since: None,
            was_available: HashMap::new(),
            last_auto_tick: Instant::now(),
        };
        app.load_jobs();
        if watch {
            app.start_watchers();
        }
        app
    }

    /// Runs `work` on a worker thread and repaints the window when it reports back.
    pub fn spawn(&self, work: impl FnOnce() -> Event + Send + 'static) {
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(work());
            ctx.request_repaint();
        });
    }

    fn load_jobs(&mut self) {
        self.busy = Some(Busy::Profiles);
        let directory = self.data_dir.clone();
        self.spawn(move || Event::Jobs(model::load_jobs(&directory)));
    }

    /// Drives are polled every few seconds; phones are pushed by the engine when plugged in or removed.
    fn start_watchers(&self) {
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        std::thread::Builder::new()
            .name("volume-watch".into())
            .spawn(move || loop {
                let found = volumes::detect();
                if sender.send(Event::Volumes(found, true)).is_err() {
                    return;
                }
                ctx.request_repaint();
                std::thread::sleep(VOLUME_POLL);
            })
            .ok();

        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            if let Ok(devices) = android::detect() {
                let _ = sender.send(Event::Phones(devices));
                ctx.request_repaint();
            }
            let watch_sender = sender.clone();
            let watch_ctx = ctx.clone();
            android::watch(move |devices| {
                let _ = watch_sender.send(Event::Phones(devices));
                watch_ctx.request_repaint();
            });
        });
    }

    pub fn selected_job(&self) -> Option<&Job> {
        self.jobs.iter().find(|job| job.id == self.selected).or(self.jobs.first())
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.jobs.iter().position(|job| job.id == self.selected).or(if self.jobs.is_empty() { None } else { Some(0) })
    }

    pub fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    pub fn select(&mut self, id: &str) {
        self.selected = id.to_string();
        self.view = View::Backup;
    }

    pub fn open_wizard(&mut self, wizard: crate::wizard::Wizard) {
        self.wizard = Some(wizard);
        self.view = View::NewBackup;
    }

    // ---- events ----------------------------------------------------------------------------

    pub fn handle_events(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            self.handle(event);
        }
        if let Some(since) = self.dirty_since {
            if since.elapsed() >= SAVE_DELAY {
                self.dirty_since = None;
                let directory = self.data_dir.clone();
                let jobs = self.jobs.clone();
                self.spawn(move || Event::Saved(model::save_jobs(&directory, &jobs)));
            } else {
                self.ctx.request_repaint_after(SAVE_DELAY);
            }
        }
        if self.last_auto_tick.elapsed() >= AUTO_RUN_TICK {
            self.last_auto_tick = Instant::now();
            self.auto_run_tick();
        }
        self.ctx.request_repaint_after(AUTO_RUN_TICK);
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Jobs(result) => {
                self.busy = None;
                match result {
                    Ok(jobs) => {
                        self.selected = jobs.iter().find(|job| job.id == "lightroom-optimized").or(jobs.first()).map(|job| job.id.clone()).unwrap_or_default();
                        self.jobs = jobs;
                        self.jobs_loaded = true;
                        // Nothing set up yet: start with the New backup card.
                        if !self.jobs.iter().any(Job::is_configured) {
                            self.open_wizard(crate::wizard::Wizard::new(crate::wizard::Kind::Photos));
                        }
                        self.message = "Your backup jobs are ready.".to_string();
                        self.auto_run_tick();
                    }
                    Err(error) => self.message = error,
                }
            }
            Event::Volumes(found, silent) => {
                let changed = found.iter().map(|volume| &volume.path).ne(self.volumes.iter().map(|volume| &volume.path));
                self.volumes = found;
                if !silent {
                    self.busy = None;
                    self.message = "Storage connections refreshed.".to_string();
                }
                if changed {
                    // A drive came or went, so what we know about destinations may be stale.
                    self.destinations.clear();
                    self.auto_run_tick();
                }
            }
            Event::Phones(devices) => {
                let changed = devices != self.phones;
                self.phones = devices;
                if changed {
                    self.auto_run_tick();
                }
            }
            Event::Plan(result) => {
                self.busy = None;
                match result {
                    Ok((fingerprint, prepared)) => {
                        self.message = format!("{} files are ready to be written.", prepared.plan.totals.files_to_write);
                        self.plan = Some(prepared.plan.clone());
                        self.last_plan = Some(CachedPlan { fingerprint, created: Instant::now(), plan: prepared });
                    }
                    Err(error) => self.message = error,
                }
            }
            Event::Progress(progress) => {
                let ours = self.run.as_ref().is_some_and(|run| run.id == progress.run_id);
                if ours {
                    self.progress = Some(progress);
                }
            }
            Event::RunDone { job_id, result } => {
                self.run = None;
                self.busy = None;
                match result {
                    Ok(result) => {
                        if !result.cancelled {
                            let now = model::unix_now();
                            if let Some(job) = self.jobs.iter_mut().find(|job| job.id == job_id) {
                                job.last_run_at = Some(now);
                            }
                            self.mark_dirty();
                        }
                        let versions = result.snapshot.as_ref().map(|name| format!(" Snapshot {name} saved.")).unwrap_or_default();
                        self.message = if result.failed_files == 0 {
                            format!("{} files backed up successfully.{versions}", result.copied_files)
                        } else {
                            format!("{} files need attention.{versions}", result.failed_files)
                        };
                        self.plan = Some(result.plan.clone());
                        self.run_result = Some(result);
                    }
                    Err(error) => self.message = error,
                }
                self.destinations.clear();
            }
            Event::Health(health) => {
                self.busy = None;
                let level = style::title_case(&health.level);
                self.message = format!("{level} health check finished.");
                self.health = Some(health);
            }
            Event::QnapFolders(result) => {
                self.busy = None;
                match result {
                    Ok(folders) => {
                        self.qnap_folders = folders;
                        self.message = "QNAP folders loaded.".to_string();
                    }
                    Err(error) => self.message = error,
                }
            }
            Event::Destination { root, info, snapshots } => {
                self.destinations_loading.remove(&root);
                self.destinations.insert(root, DestinationState { info: Some(info), snapshots });
            }
            Event::VolumeCreated { job_id, name, result } => {
                if self.editor.job_id == job_id {
                    self.editor.creating_volume = false;
                }
                match result {
                    Ok(mount) => {
                        if let Some(job) = self.jobs.iter_mut().find(|job| job.id == job_id) {
                            job.destination_root = mount;
                            if job.sources.len() == 1 {
                                job.layout = Some("mirror".to_string());
                            }
                        }
                        self.mark_dirty();
                        if self.editor.job_id == job_id {
                            self.editor.volume_name = None;
                            self.editor.volume_status = Some(format!("Volume {name} is ready. It shares the free space of the drive."));
                        }
                    }
                    Err(error) => {
                        if self.editor.job_id == job_id {
                            self.editor.volume_status = Some(error);
                        }
                    }
                }
            }
            Event::JobForgotten { job_id, result } => match result {
                Ok(()) => {
                    let name = self.jobs.iter().find(|job| job.id == job_id).map(|job| job.name.clone()).unwrap_or_default();
                    self.jobs.retain(|job| job.id != job_id);
                    self.selected = self.jobs.first().map(|job| job.id.clone()).unwrap_or_default();
                    self.plan = None;
                    self.run_result = None;
                    self.message = format!("{name} removed.");
                    self.mark_dirty();
                }
                Err(error) => self.message = error,
            },
            Event::Saved(result) => {
                if let Err(error) = result {
                    self.message = format!("Could not save backup jobs: {error}");
                }
            }
            Event::Phone(event) => crate::phone::handle(self, event),
        }
    }

    /// Loads filesystem details and snapshots for a destination folder once.
    pub fn destination(&mut self, root: &str) -> Option<&DestinationState> {
        if root.is_empty() {
            return None;
        }
        if !self.destinations.contains_key(root) && self.destinations_loading.insert(root.to_string()) {
            let root = root.to_string();
            self.spawn(move || {
                let info = volumes::inspect_destination(&root);
                let snapshots = snapshots::list(Path::new(&root));
                Event::Destination { root, info, snapshots }
            });
        }
        self.destinations.get(root)
    }

    // ---- automatic runs -------------------------------------------------------------------

    /// A job can run when its destination and every source are reachable right now.
    pub fn job_available(&self, job: &Job) -> bool {
        if job.destination_root.is_empty() || job.sources.is_empty() {
            return false;
        }
        let mounted = |path: &str| !path.starts_with("/Volumes/") || self.volumes.iter().any(|volume| path == volume.path || path.starts_with(&format!("{}/", volume.path)));
        let destination_ready = match job.destination_kind.as_str() {
            "android" => !self.phones.is_empty(),
            "qnap-rsync" => true,
            _ => mounted(&job.destination_root),
        };
        destination_ready && job.sources.iter().all(|source| if source.is_phone() { !self.phones.is_empty() } else { !source.path.is_empty() && mounted(&source.path) })
    }

    /// Starts a job when its drive (or phone) is connected, and on its schedule. One run at a time.
    fn auto_run_tick(&mut self) {
        if !self.jobs_loaded || self.busy.is_some() || self.run.is_some() {
            return;
        }
        let now = model::unix_now();
        let mut due = None;
        for job in &self.jobs {
            let Some(auto) = &job.auto_run else { continue };
            let when_connected = auto.when_connected.unwrap_or(false);
            let every = auto.every_hours.unwrap_or(0);
            if !when_connected && every == 0 {
                continue;
            }
            let available = self.job_available(job);
            let appeared = available && !self.was_available.get(&job.id).copied().unwrap_or(false);
            self.was_available.insert(job.id.clone(), available);
            if !available {
                continue;
            }
            let since = now.saturating_sub(job.last_run_at.unwrap_or(0));
            let connected_due = when_connected && appeared && since > RECONNECT_GRACE_SECONDS;
            let schedule_due = every > 0 && since >= u64::from(every) * 3600;
            if connected_due || schedule_due {
                due = Some(job.id.clone());
                break;
            }
        }
        if let Some(id) = due {
            self.run_backup(&id, true);
        }
    }

    // ---- actions --------------------------------------------------------------------------

    pub fn refresh_volumes(&mut self) {
        self.busy = Some(Busy::Volumes);
        self.spawn(|| Event::Volumes(volumes::detect(), false));
    }

    pub fn make_plan(&mut self) {
        let Some(job) = self.selected_job().cloned() else {
            self.message = "Choose a configured job first.".to_string();
            return;
        };
        let profile = match job.engine_profile() {
            Ok(profile) => profile,
            Err(error) => return self.message = error,
        };
        self.busy = Some(Busy::Plan);
        self.run_result = None;
        let directory = self.data_dir.clone();
        let fingerprint = job.fingerprint();
        self.spawn(move || {
            let result = Manifest::open(&directory).and_then(|manifest| backup::plan(&profile, &manifest)).map(|prepared| (fingerprint, Arc::new(prepared)));
            Event::Plan(result)
        });
    }

    pub fn run_backup(&mut self, job_id: &str, automatic: bool) {
        if self.run.is_some() {
            self.message = "A backup is already running.".to_string();
            return;
        }
        let Some(job) = self.jobs.iter().find(|job| job.id == job_id).cloned() else {
            self.message = "Choose a configured job first.".to_string();
            return;
        };
        let profile = match job.engine_profile() {
            Ok(profile) => profile,
            Err(error) => return self.message = error,
        };
        let manifest = match Manifest::open(&self.data_dir) {
            Ok(manifest) => manifest,
            Err(error) => return self.message = error,
        };
        let fingerprint = job.fingerprint();
        let cached = self.last_plan.take().filter(|cached| cached.fingerprint == fingerprint && cached.created.elapsed() < PLAN_REUSE_WINDOW).map(|cached| cached.plan);
        let run_id = model::new_id("run");
        let control = RunControl::new();
        self.run = Some(ActiveRun { id: run_id.clone(), job_id: job.id.clone(), control: control.clone(), paused: false });
        self.progress = None;
        self.busy = Some(Busy::Run);
        if automatic {
            self.selected = job.id.clone();
            self.message = format!("Automatic backup started: {}.", job.name);
        }
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        let job_id = job.id.clone();
        std::thread::Builder::new()
            .name("backup-run".into())
            .spawn(move || {
                let progress_sender = sender.clone();
                let progress_ctx = ctx.clone();
                let emit = move |progress: BackupProgress| {
                    let _ = progress_sender.send(Event::Progress(progress));
                    progress_ctx.request_repaint();
                };
                let result = backup::run(profile, manifest, control, run_id, cached, &emit);
                let _ = sender.send(Event::RunDone { job_id, result });
                ctx.request_repaint();
            })
            .ok();
    }

    pub fn pause_or_resume(&mut self) {
        let Some(run) = &mut self.run else { return };
        if run.paused {
            run.control.resume();
            run.paused = false;
            self.message = "Backup resumed.".to_string();
        } else {
            run.control.pause();
            run.paused = true;
            self.message = "Backup paused. Partial copies resume where they stopped.".to_string();
        }
    }

    pub fn cancel_run(&mut self) {
        if let Some(run) = &self.run {
            run.control.cancel();
            self.message = "Stopping. Partial copies resume on the next run.".to_string();
        }
    }

    pub fn check_health(&mut self, path: String, level: &'static str) {
        self.busy = Some(Busy::Health);
        self.spawn(move || Event::Health(volumes::health(path, level.to_string())));
    }

    pub fn load_qnap_folders(&mut self) {
        let Some(profile) = self.selected_job().map(Job::engine_profile) else { return };
        let profile = match profile {
            Ok(profile) => profile,
            Err(error) => return self.message = error,
        };
        self.busy = Some(Busy::QnapFolders);
        self.spawn(move || Event::QnapFolders(backup_engine::rsync::list_directories(&profile)));
    }

    pub fn create_volume(&mut self, job_id: String, root: String, name: String) {
        self.editor.creating_volume = true;
        self.editor.volume_status = None;
        self.spawn(move || {
            let result = volumes::create_job_volume(&root, &name);
            Event::VolumeCreated { job_id, name, result }
        });
    }

    pub fn reveal(&mut self, path: &str) {
        if !Path::new(path).exists() {
            self.message = "That folder is not available. Is the drive connected?".to_string();
            return;
        }
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        if let Err(error) = std::process::Command::new(opener).arg(path).spawn() {
            self.message = error.to_string();
        }
    }

    pub fn remove_job(&mut self, job_id: &str) {
        if self.run.as_ref().is_some_and(|run| run.job_id == job_id) || self.busy == Some(Busy::Run) {
            self.message = "Stop the running backup before removing a job.".to_string();
            return;
        }
        let Some(job) = self.jobs.iter().find(|job| job.id == job_id) else { return };
        let question = format!("Remove \"{}\"? Files it already backed up stay on the drive. Only the job and its history are removed.", job.name);
        let answer = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Remove job")
            .set_description(question)
            .set_buttons(rfd::MessageButtons::OkCancelCustom("Remove".into(), "Keep".into()))
            .show();
        let confirmed = matches!(answer, rfd::MessageDialogResult::Ok) || matches!(&answer, rfd::MessageDialogResult::Custom(label) if label == "Remove");
        if !confirmed {
            return;
        }
        let directory = self.data_dir.clone();
        let job_id = job_id.to_string();
        self.spawn(move || {
            let result = Manifest::open(&directory).and_then(|manifest| manifest.forget(&job_id));
            Event::JobForgotten { job_id, result }
        });
    }

    fn add_job(&mut self, job: Job, open_settings: bool, message: String) {
        self.selected = job.id.clone();
        self.jobs.push(job);
        self.view = View::Backup;
        self.tab = if open_settings { Tab::Settings } else { Tab::Status };
        self.message = message;
        self.mark_dirty();
    }

    /// A copy of the selected job that backs up to another drive.
    pub fn add_drive_copy(&mut self) {
        let Some(job) = self.selected_job().cloned() else {
            self.message = "Choose a configured job before adding a drive.".to_string();
            return;
        };
        let Some(folder) = pick_folder(None) else { return };
        let drive = folder.rsplit('/').find(|part| !part.is_empty()).unwrap_or("New drive").to_string();
        let copy = Job { id: model::new_id(&job.id), name: format!("{} on {drive}", job.name), destination_kind: "local".to_string(), destination_root: folder, last_run_at: None, ..job };
        self.add_job(copy, false, format!("{drive} added as a backup destination."));
    }

    pub fn create_phone_job(&mut self, device: Option<AndroidDevice>) {
        let preset = model::presets(&self.data_dir).into_iter().find(|preset| preset.id == "phone-camera-roll").unwrap_or_else(phone_preset_fallback);
        let phone_name = device.as_ref().map(|device| format!("{} {}", style::title_case(&device.manufacturer), device.model)).unwrap_or_else(|| "phone".to_string());
        let device_id = device.map(|device| device.id).unwrap_or_default();
        let first_volume = self.volumes.first().cloned();
        let job = Job {
            id: model::new_id("phone"),
            preset: Some("phone-camera-roll".to_string()),
            name: format!("Back up {phone_name}"),
            destination_root: first_volume.as_ref().map(|volume| volume.path.clone()).unwrap_or_default(),
            sources: preset.sources.iter().cloned().map(|source| Source { device_id: Some(device_id.clone()), ..source }).collect(),
            ..preset
        };
        let message = match first_volume {
            Some(volume) => format!("Phone backup job created on {}. Check the folders, then run it.", volume.name),
            None => "Phone backup job created. Choose the drive to back up to.".to_string(),
        };
        self.add_job(job, true, message);
    }

    pub fn create_qnap_job(&mut self) {
        let job = Job {
            id: model::new_id("qnap"),
            name: "QNAP backup job".to_string(),
            description: "A network backup job using QNAP rsync over SSH.".to_string(),
            destination_kind: "qnap-rsync".to_string(),
            destination_root: "/share/Backups".to_string(),
            mode: "safe-mirror".to_string(),
            verification: "metadata".to_string(),
            qnap: Some(Qnap { share: String::new(), ..Qnap::default() }),
            sources: vec![Source { id: "source".into(), label: "Choose a source folder".into(), category: "other".into(), ..Default::default() }],
            ..Default::default()
        };
        self.add_job(job, true, "QNAP job created. Enter the NAS connection details.".to_string());
    }

    /// Writes unsaved changes before the window closes.
    pub fn flush(&mut self) {
        if self.dirty_since.take().is_some() {
            let _ = model::save_jobs(&self.data_dir, &self.jobs);
        }
    }
}

fn phone_preset_fallback() -> Job {
    let source = |id: &str, label: &str, path: &str| Source { id: id.into(), label: label.into(), path: path.into(), category: "photos".into(), kind: Some("android".into()), device_id: Some(String::new()), ..Default::default() };
    Job {
        id: "phone-camera-roll".into(),
        preset: Some("phone-camera-roll".into()),
        name: "Phone Camera Roll".into(),
        description: "Backs up the photos, videos, screenshots and WhatsApp media of the connected Android phone to a drive.".into(),
        destination_kind: "local".into(),
        mode: "safe-mirror".into(),
        verification: "metadata".into(),
        sources: vec![source("camera", "Camera", "DCIM/Camera"), source("screenshots", "Screenshots", "Pictures/Screenshots")],
        ..Default::default()
    }
}

/// Native folder picker.
pub fn pick_folder(start_at: Option<&str>) -> Option<String> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(start) = start_at.filter(|start| !start.is_empty() && Path::new(start).exists()) {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_folder().map(|path| path.display().to_string())
}

pub fn load_icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(include_bytes!("../../src/assets/app-icon.png")).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture("app-icon", color, egui::TextureOptions::LINEAR))
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::views::show(self, ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.flush();
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use backup_engine::backup::BackupTotals;

    /// An app on a temporary data folder with sample drives, a dry run and a finished run.
    pub fn sample_app(ctx: &egui::Context, name: &str) -> App {
        let directory = std::env::temp_dir().join(format!("lbs-native-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let mut app = App::build(ctx.clone(), directory, false);
        let started = Instant::now();
        while !app.jobs_loaded && started.elapsed() < Duration::from_secs(5) {
            app.handle_events();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.jobs_loaded, "the bundled presets load on first launch");
        app.volumes = vec![
            MountedVolume { name: "T7 Shield".into(), path: "/Volumes/T7 Shield".into(), filesystem: "apfs".into(), capacity: Some(2_000_000_000_000), available: Some(812_000_000_000) },
            MountedVolume { name: "Archive".into(), path: "/Volumes/Archive".into(), filesystem: "exfat".into(), capacity: None, available: None },
        ];
        if let Some(job) = app.jobs.first_mut() {
            job.destination_root = "/Volumes/T7 Shield/Lightroom".into();
            job.last_run_at = Some(model::unix_now() - 7_200);
            job.sources = vec![Source { id: "photos".into(), label: "Photos".into(), path: "/Users/me/Pictures/Lightroom".into(), category: "photos".into(), ..Default::default() }];
        }
        let plan = BackupPlan {
            profile_id: "lightroom-optimized".into(),
            destination_root: "/Volumes/T7 Shield/Lightroom".into(),
            created_at: "1760000000".into(),
            entries: Vec::new(),
            entries_truncated: false,
            warnings: Vec::new(),
            totals: BackupTotals { scanned_files: 48_210, files_to_write: 312, files_skipped: 47_898, bytes_to_write: 6_400_000_000, ..Default::default() },
        };
        app.run_result = Some(BackupRunResult { plan: plan.clone(), completed_at: "1760000900".into(), copied_files: 312, archived_files: 4, failed_files: 0, cancelled: false, errors: Vec::new(), snapshot: None });
        app.plan = Some(plan);
        app
    }

    fn frame(ctx: &egui::Context, app: &mut App) {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1240.0, 860.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| crate::views::show(app, ui));
        output.textures_delta.clear();
    }

    #[test]
    fn every_screen_draws_without_errors() {
        let ctx = egui::Context::default();
        crate::style::apply(&ctx);
        let mut app = sample_app(&ctx, "screens");
        app.open_wizard(crate::wizard::Wizard::new(crate::wizard::Kind::Photos));
        for (view, tab) in [(View::Backup, Tab::Status), (View::Backup, Tab::Settings), (View::NewBackup, Tab::Status), (View::Drives, Tab::Status), (View::Phone, Tab::Status)] {
            app.view = view;
            app.tab = tab;
            for _ in 0..3 {
                frame(&ctx, &mut app);
            }
        }
        app.progress = Some(BackupProgress { run_id: "r".into(), event: "copying".into(), path: Some("/Users/me/Pictures/IMG_0001.CR3".into()), discovered_files: 48_210, evaluated_files: 48_210, completed_files: 120, total_files: 312, copied_bytes: 2_000_000_000, total_bytes: 6_400_000_000, elapsed_seconds: 42, bytes_per_second: 48_000_000 });
        app.run = Some(ActiveRun { id: "r".into(), job_id: app.jobs[0].id.clone(), control: RunControl::new(), paused: false });
        app.view = View::Backup;
        app.tab = Tab::Status;
        frame(&ctx, &mut app);
        assert!(app.dirty_since.is_none(), "drawing alone never changes a job");
    }

    /// Renders both pages to PNG files in the temp folder, to look at the window without opening it.
    /// Needs a GPU, so it only runs on request: `cargo test -p backup-station -- --ignored`.
    #[test]
    #[ignore]
    fn render_screens_to_png() {
        let out = std::env::temp_dir().join("lbs-native-screens");
        std::fs::create_dir_all(&out).unwrap();
        let cases = [
            (View::Backup, Tab::Status, "backup", false, 860.0),
            (View::Backup, Tab::Status, "backup-running", true, 860.0),
            (View::Backup, Tab::Settings, "backup-settings", false, 1900.0),
            (View::NewBackup, Tab::Status, "new-backup", false, 860.0),
            (View::Drives, Tab::Status, "drives", false, 860.0),
        ];
        for (view, tab, name, progress, height) in cases {
            let mut app = sample_app(&egui::Context::default(), name);
            app.view = view;
            app.tab = tab;
            if view == View::NewBackup {
                app.wizard = Some(crate::wizard::Wizard { kind: crate::wizard::Kind::Videos, source: "/Volumes/Archive".into(), destination: "/Volumes/T7 Shield".into(), error: None });
            }
            if progress {
                app.progress = Some(BackupProgress { run_id: "r".into(), event: "copying".into(), path: Some("/Users/me/Pictures/2025/IMG_0001.CR3".into()), discovered_files: 48_210, evaluated_files: 48_210, completed_files: 120, total_files: 312, copied_bytes: 2_000_000_000, total_bytes: 6_400_000_000, elapsed_seconds: 42, bytes_per_second: 48_000_000 });
                app.run = Some(ActiveRun { id: "r".into(), job_id: app.jobs[0].id.clone(), control: RunControl::new(), paused: false });
            }
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(1240.0, height))
                .with_pixels_per_point(1.0)
                .with_context_setup(crate::style::apply)
                .wgpu()
                .build_ui_state(|ui, app: &mut App| crate::views::show(app, ui), app);
            harness.run_steps(3);
            let image = harness.render().expect("renders");
            image.save(out.join(format!("{name}.png"))).unwrap();
        }
    }
}
