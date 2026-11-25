//! A list of past backup runs, newest first, kept in `run-history.json` next to the jobs.
//! It powers the History page and the "last backup" lines, and survives restarts.

use serde::{Deserialize, Serialize};
use std::path::Path;

const FILE: &str = "run-history.json";
/// Older runs are dropped so the file stays small and loads instantly.
pub const KEEP: usize = 500;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub job_id: String,
    pub job_name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub destination: String,
    /// Unix seconds.
    pub started_at: u64,
    #[serde(default)]
    pub duration_seconds: u64,
    #[serde(default)]
    pub copied_files: u64,
    #[serde(default)]
    pub copied_bytes: u64,
    #[serde(default)]
    pub skipped_files: u64,
    #[serde(default)]
    pub archived_files: u64,
    #[serde(default)]
    pub failed_files: u64,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub automatic: bool,
    /// The run could not start or stopped with an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Success,
    Warning,
    Failed,
    Stopped,
}

impl RunRecord {
    pub fn outcome(&self) -> Outcome {
        if self.error.is_some() {
            Outcome::Failed
        } else if self.cancelled {
            Outcome::Stopped
        } else if self.failed_files > 0 {
            Outcome::Warning
        } else {
            Outcome::Success
        }
    }
}

pub fn load(directory: &Path) -> Vec<RunRecord> {
    std::fs::read(directory.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes through a temporary file, so a crash never leaves half a history behind.
pub fn save(directory: &Path, records: &[RunRecord]) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(records).map_err(|error| error.to_string())?;
    let temporary = directory.join(format!("{FILE}.tmp"));
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, directory.join(FILE)).map_err(|error| error.to_string())
}

/// Adds a run at the front and drops the oldest past [`KEEP`].
pub fn push(records: &mut Vec<RunRecord>, record: RunRecord) {
    records.insert(0, record);
    records.truncate(KEEP);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_newest_first_and_round_trips() {
        let directory = std::env::temp_dir().join(format!("lbs-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        assert!(load(&directory).is_empty(), "no file means no history");
        let mut records = Vec::new();
        for index in 0..(KEEP as u64 + 3) {
            push(
                &mut records,
                RunRecord {
                    job_id: "a".into(),
                    job_name: "A".into(),
                    started_at: index,
                    ..Default::default()
                },
            );
        }
        assert_eq!(records.len(), KEEP);
        assert_eq!(records[0].started_at, KEEP as u64 + 2);
        save(&directory, &records).unwrap();
        assert_eq!(load(&directory), records);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn outcomes() {
        let base = RunRecord::default();
        assert_eq!(base.outcome(), Outcome::Success);
        assert_eq!(
            RunRecord {
                failed_files: 2,
                ..base.clone()
            }
            .outcome(),
            Outcome::Warning
        );
        assert_eq!(
            RunRecord {
                cancelled: true,
                ..base.clone()
            }
            .outcome(),
            Outcome::Stopped
        );
        assert_eq!(
            RunRecord {
                error: Some("x".into()),
                ..base
            }
            .outcome(),
            Outcome::Failed
        );
    }
}
