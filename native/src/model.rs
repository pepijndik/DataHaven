//! Backup jobs as the window edits them. The JSON shape matches `profiles.json`, which the Tauri
//! window reads and writes too, so both windows share the same jobs. Unknown fields are kept.

use backup_engine::profile::{BackupProfile, ProfileStore};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Same identifier as the Tauri app, so jobs, history and phone settings are shared.
pub const APP_IDENTIFIER: &str = "com.lightroombackup.station";

/// The app data folder, the same one Tauri uses (`~/Library/Application Support/<id>` on macOS).
pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support").join(APP_IDENTIFIER);
    }
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|path| path.is_absolute()).unwrap_or_else(|| home.join(".local/share"));
    base.join(APP_IDENTIFIER)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_id: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Source {
    pub fn is_phone(&self) -> bool {
        self.kind.as_deref() == Some("android")
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Filter {
    #[serde(default)]
    pub id: String,
    pub pattern: String,
    #[serde(default = "exclude")]
    pub action: String,
    #[serde(default)]
    pub note: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn exclude() -> String {
    "exclude".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Qnap {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub share: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub remote_root: String,
    #[serde(default = "ssh_port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub use_helper: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn ssh_port() -> u16 {
    22
}

impl Default for Qnap {
    fn default() -> Self {
        Self { host: String::new(), share: "Backups".to_string(), username: String::new(), remote_root: "/share/Backups".to_string(), ssh_port: 22, use_helper: true, extra: Map::new() }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PhoneTarget {
    #[serde(default)]
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_id: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutoRun {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_connected: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_hours: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default = "local")]
    pub destination_kind: String,
    #[serde(default)]
    pub destination_root: String,
    #[serde(default = "safe_mirror")]
    pub mode: String,
    #[serde(default = "metadata")]
    pub verification: String,
    #[serde(default)]
    pub preserve_mac_metadata: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<Filter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qnap: Option<Qnap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<PhoneTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guards: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub important_patterns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_copies: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_destination: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_keep_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versioning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_run: Option<AutoRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn local() -> String {
    "local".to_string()
}
fn safe_mirror() -> String {
    "safe-mirror".to_string()
}
fn metadata() -> String {
    "metadata".to_string()
}

impl Job {
    /// The engine's typed view of this job.
    pub fn engine_profile(&self) -> Result<BackupProfile, String> {
        let value = serde_json::to_value(self).map_err(|error| error.to_string())?;
        serde_json::from_value(value).map_err(|error| format!("This job has an invalid setting: {error}"))
    }

    /// A key that changes whenever anything that affects a backup changes.
    pub fn fingerprint(&self) -> String {
        self.engine_profile().ok().and_then(|profile| serde_json::to_string(&profile).ok()).unwrap_or_default()
    }

    pub fn is_configured(&self) -> bool {
        !self.destination_root.is_empty() && self.sources.iter().any(|source| !source.path.is_empty())
    }

    pub fn guard(&self, guard: &str) -> bool {
        self.guards.as_ref().is_some_and(|guards| guards.iter().any(|value| value == guard))
    }

    pub fn set_guard(&mut self, guard: &str, on: bool) {
        let guards = self.guards.get_or_insert_with(Vec::new);
        guards.retain(|value| value != guard);
        if on {
            guards.push(guard.to_string());
        }
    }

    pub fn layout(&self) -> &str {
        self.layout.as_deref().unwrap_or("per-source")
    }

    /// A unique source id based on `base`.
    pub fn unique_source_id(&self, base: &str) -> String {
        let mut id = base.to_string();
        let mut counter = 2;
        while self.sources.iter().any(|source| source.id == id) {
            id = format!("{base}-{counter}");
            counter += 1;
        }
        id
    }
}

pub fn load_jobs(directory: &Path) -> Result<Vec<Job>, String> {
    ProfileStore::new(directory)
        .load(directory)?
        .into_iter()
        .map(|value| serde_json::from_value::<Job>(value).map_err(|error| format!("profiles.json has an invalid job: {error}")))
        .collect()
}

pub fn save_jobs(directory: &Path, jobs: &[Job]) -> Result<(), String> {
    let values = jobs.iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?;
    ProfileStore::new(directory).save(&values)
}

pub fn presets(directory: &Path) -> Vec<Job> {
    backup_engine::profile::presets(directory).into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect()
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

/// A short unique id for new jobs and runs.
pub fn new_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_nanos()).unwrap_or(0);
    format!("{prefix}-{}", nanos / 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_round_trip_without_losing_fields() {
        let text = r#"{"id":"a","name":"A","description":"d","sources":[{"id":"s","label":"S","path":"/x","category":"photos","custom":1}],
            "destinationKind":"local","destinationRoot":"/Volumes/T7","mode":"safe-mirror","verification":"metadata",
            "preserveMacMetadata":true,"autoRun":{"whenConnected":true,"everyHours":24},"lastRunAt":5,"unknownField":"kept"}"#;
        let job: Job = serde_json::from_str(text).unwrap();
        let back = serde_json::to_value(&job).unwrap();
        assert_eq!(back["unknownField"], "kept");
        assert_eq!(back["sources"][0]["custom"], 1);
        assert_eq!(back["autoRun"]["everyHours"], 24);
        assert_eq!(serde_json::from_value::<Job>(back).unwrap(), job);
    }

    #[test]
    fn bundled_presets_become_engine_profiles() {
        let presets = presets(Path::new("/nonexistent"));
        assert!(presets.len() >= 4);
        for preset in presets {
            preset.engine_profile().unwrap();
        }
    }

    #[test]
    fn unique_source_ids() {
        let mut job = Job { id: "j".into(), ..Default::default() };
        job.sources.push(Source { id: "source".into(), ..Default::default() });
        assert_eq!(job.unique_source_id("source"), "source-2");
        assert_eq!(job.unique_source_id("camera"), "camera");
    }
}
