use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const BUNDLED_PRESETS: [&str; 4] = [
    include_str!("../presets/lightroom-optimized.json"),
    include_str!("../presets/media-archive.json"),
    include_str!("../presets/video-archive.json"),
    include_str!("../presets/phone-camera-roll.json"),
];

pub const DEFAULT_IMPORTANT_PATTERNS: [&str; 4] = ["*.lrcat", "*.xmp", "*.lrtemplate", "*.lrsmcol"];
pub const DEFAULT_PARALLEL_COPIES: usize = 4;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum BackupMode {
    #[default]
    SafeMirror,
    AppendOnly,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Verification {
    #[default]
    Metadata,
    ImportantHash,
    FullHash,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DestinationKind {
    #[default]
    Local,
    QnapSmb,
    QnapRsync,
    /// A folder on an Android phone (MTP). `destination_root` is the folder on the phone.
    Android,
}

/// Where files land inside the destination.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    /// `<destination>/<source id>/...`, so several sources can share one destination.
    #[default]
    PerSource,
    /// `<destination>/...`, an exact copy of one source, like Carbon Copy Cloner.
    Mirror,
}

/// How older versions of replaced files are kept in safe mirror mode.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Versioning {
    /// Replaced files move to `_Archive/<run>`. Works on every drive.
    #[default]
    Archive,
    /// After each run the whole backup is cloned to `_Snapshots/<run>`. APFS only, costs space only for what changes.
    Snapshots,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    #[default]
    Folder,
    /// A folder on an Android phone (MTP), for example `DCIM/Camera`.
    Android,
}

/// Which phone and storage to use. An empty `device_id` means whichever phone is connected.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneTarget {
    #[serde(default)]
    pub device_id: String,
    pub storage_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSource {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub kind: SourceKind,
    #[serde(default)]
    pub device_id: String,
    pub storage_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QnapSettings {
    pub host: String,
    pub share: String,
    pub username: String,
    pub remote_root: String,
    pub ssh_port: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFilter {
    pub pattern: String,
    pub action: String,
}

/// The engine's view of a backup job. The UI stores more fields (labels, notes),
/// which are kept untouched by the profile store and ignored here.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProfile {
    pub id: String,
    pub sources: Vec<ProfileSource>,
    pub destination_root: String,
    #[serde(default)]
    pub destination_kind: DestinationKind,
    #[serde(default)]
    pub mode: BackupMode,
    #[serde(default)]
    pub verification: Verification,
    #[serde(default)]
    pub preserve_mac_metadata: bool,
    #[serde(default)]
    pub filters: Vec<BackupFilter>,
    pub qnap: Option<QnapSettings>,
    /// The phone used when `destination_kind` is `android`.
    pub phone: Option<PhoneTarget>,
    /// Safety checks that run before scanning, for example `lightroom-closed`.
    #[serde(default)]
    pub guards: Vec<String>,
    /// Files hashed when verification is `important-hash`.
    #[serde(default)]
    pub important_patterns: Vec<String>,
    pub parallel_copies: Option<usize>,
    /// Stat every destination file instead of trusting the manifest. Slower, catches files removed from the backup by hand.
    #[serde(default)]
    pub check_destination: bool,
    /// Remove archived versions or snapshots older than this many days. Keeps everything when unset.
    pub archive_keep_days: Option<u32>,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub versioning: Versioning,
}

impl BackupProfile {
    pub fn guard_enabled(&self, guard: &str) -> bool {
        self.guards.iter().any(|value| value == guard)
            || (guard == "lightroom-closed" && self.sources.iter().any(|source| source.category == "catalog"))
    }

    /// The folder a source is copied into.
    pub fn destination_base(&self, source_id: &str) -> PathBuf {
        let root = PathBuf::from(&self.destination_root);
        match self.layout {
            Layout::Mirror => root,
            Layout::PerSource => root.join(source_id),
        }
    }

    /// Path inside the destination for a file of a source, with `/` separators.
    pub fn destination_relative(&self, source_id: &str, relative: &str) -> String {
        match self.layout {
            Layout::Mirror => relative.to_string(),
            Layout::PerSource => format!("{source_id}/{relative}"),
        }
    }

    pub fn check_layout(&self) -> Result<(), String> {
        if self.layout == Layout::Mirror && self.sources.len() != 1 {
            return Err("Mirror layout copies exactly one source into the destination. Remove the other sources or use the per source layout.".to_string());
        }
        Ok(())
    }

    pub fn uses_phone(&self) -> bool {
        self.destination_kind == DestinationKind::Android || self.sources.iter().any(|source| source.kind == SourceKind::Android)
    }

    pub fn parallel_copies(&self) -> usize {
        self.parallel_copies.unwrap_or(DEFAULT_PARALLEL_COPIES).clamp(1, 16)
    }

    pub fn important_patterns(&self) -> Vec<String> {
        if self.important_patterns.is_empty() {
            DEFAULT_IMPORTANT_PATTERNS.iter().map(ToString::to_string).collect()
        } else {
            self.important_patterns.clone()
        }
    }
}

/// Presets are profile templates stored as JSON. Bundled ones ship with the app,
/// and any `*.json` in `<app data>/presets` is added or overrides a bundled preset with the same id.
pub fn presets(app_data_dir: &Path) -> Vec<Value> {
    let mut presets: Vec<Value> = BUNDLED_PRESETS.iter().filter_map(|text| serde_json::from_str(text).ok()).collect();

    let Ok(directory) = fs::read_dir(app_data_dir.join("presets")) else { return presets };
    for entry in directory.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Some(preset) = fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()) else {
            log::warn!("Ignoring invalid preset {}", path.display());
            continue;
        };
        match presets.iter_mut().find(|existing| existing["id"] == preset["id"]) {
            Some(existing) => *existing = preset,
            None => presets.push(preset),
        }
    }
    presets
}

pub struct ProfileStore {
    path: PathBuf,
}

impl ProfileStore {
    pub fn new(app_data_dir: &Path) -> Self {
        Self { path: app_data_dir.join("profiles.json") }
    }

    /// Saved profiles, or the presets on first launch.
    pub fn load(&self, app_data_dir: &Path) -> Result<Vec<Value>, String> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| format!("profiles.json is invalid: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(presets(app_data_dir)),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn save(&self, profiles: &[Value]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let temporary = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(profiles).map_err(|error| error.to_string())?;
        fs::write(&temporary, text).map_err(|error| error.to_string())?;
        fs::rename(&temporary, &self.path).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_presets_parse_as_profiles() {
        let presets = presets(Path::new("/nonexistent"));
        assert_eq!(presets.len(), BUNDLED_PRESETS.len());
        for preset in presets {
            serde_json::from_value::<BackupProfile>(preset).expect("preset should be a valid profile");
        }
    }

    #[test]
    fn catalog_sources_enable_the_lightroom_guard() {
        let profile: BackupProfile = serde_json::from_value(serde_json::json!({
            "id": "job", "destinationRoot": "/backup",
            "sources": [{ "id": "lr", "path": "/lr", "category": "catalog" }]
        })).unwrap();
        assert!(profile.guard_enabled("lightroom-closed"));
        assert_eq!(profile.parallel_copies(), DEFAULT_PARALLEL_COPIES);
    }
}
