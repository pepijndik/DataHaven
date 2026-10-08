use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BATCH_SIZE: usize = 1000;
const BATCH_INTERVAL: Duration = Duration::from_millis(500);

/// What the manifest knows about a file at the time it was last backed up.
#[derive(Clone, Debug, PartialEq)]
pub struct ManifestRecord {
    pub size: u64,
    pub modified_ns: Option<i64>,
    pub hash: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RecordUpdate {
    pub profile_id: String,
    pub source_path: String,
    pub destination_path: String,
    pub size: u64,
    pub modified_ns: i64,
    pub hash: Option<String>,
    pub status: &'static str,
    pub recorded_at: String,
}

/// Records of one profile, keyed by source and destination path.
pub struct Snapshot(HashMap<(String, String), ManifestRecord>);

impl Snapshot {
    pub fn get(&self, source_path: &str, destination_path: &str) -> Option<&ManifestRecord> {
        self.0.get(&(source_path.to_string(), destination_path.to_string()))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&(String, String), &ManifestRecord)> {
        self.0.iter()
    }
}

#[derive(Clone)]
pub struct Manifest {
    path: PathBuf,
}

impl Manifest {
    pub fn open(app_data_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(app_data_dir).map_err(|error| error.to_string())?;
        let manifest = Self { path: app_data_dir.join("backup-manifest.sqlite3") };
        let connection = manifest.connection()?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS backup_files (
                profile_id TEXT NOT NULL,
                source_path TEXT NOT NULL,
                destination_path TEXT NOT NULL,
                size INTEGER NOT NULL,
                modified_seconds INTEGER NOT NULL,
                status TEXT NOT NULL,
                copied_at TEXT NOT NULL,
                PRIMARY KEY (profile_id, source_path, destination_path)
            );
            CREATE INDEX IF NOT EXISTS backup_files_destination_idx
            ON backup_files (destination_path);",
        ).map_err(|error| error.to_string())?;
        add_column_if_missing(&connection, "modified_ns", "INTEGER")?;
        add_column_if_missing(&connection, "hash", "TEXT")?;
        Ok(manifest)
    }

    pub fn snapshot(&self, profile_id: &str) -> Result<Snapshot, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT source_path, destination_path, size, modified_ns, hash FROM backup_files WHERE profile_id = ?1 AND status = 'copied'")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([profile_id], |row| {
                Ok((
                    (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                    ManifestRecord { size: row.get::<_, i64>(2)? as u64, modified_ns: row.get(3)?, hash: row.get(4)? },
                ))
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<HashMap<_, _>, _>>().map(Snapshot).map_err(|error| error.to_string())
    }

    /// Drops the history of a removed job. Its backup files are left alone.
    pub fn forget(&self, profile_id: &str) -> Result<(), String> {
        self.connection()?.execute("DELETE FROM backup_files WHERE profile_id = ?1", [profile_id]).map(|_| ()).map_err(|error| error.to_string())
    }

    /// Starts a background thread that writes updates in batched transactions.
    pub fn writer(&self) -> Result<ManifestWriter, String> {
        let connection = self.connection()?;
        let (sender, receiver) = mpsc::channel::<RecordUpdate>();
        let handle = std::thread::spawn(move || {
            let mut batch = Vec::with_capacity(BATCH_SIZE);
            let mut last_flush = Instant::now();
            loop {
                let closed = match receiver.recv_timeout(BATCH_INTERVAL) {
                    Ok(update) => { batch.push(update); false }
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => true,
                };
                if !batch.is_empty() && (closed || batch.len() >= BATCH_SIZE || last_flush.elapsed() >= BATCH_INTERVAL) {
                    if let Err(error) = write_batch(&connection, &batch) {
                        log::error!("Could not write manifest batch: {error}");
                    }
                    batch.clear();
                    last_flush = Instant::now();
                }
                if closed {
                    return;
                }
            }
        });
        Ok(ManifestWriter { sender: Some(sender), handle: Some(handle) })
    }

    fn connection(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.path).map_err(|error| error.to_string())?;
        connection.busy_timeout(Duration::from_secs(10)).map_err(|error| error.to_string())?;
        connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;").map_err(|error| error.to_string())?;
        Ok(connection)
    }
}

pub struct ManifestWriter {
    sender: Option<Sender<RecordUpdate>>,
    handle: Option<JoinHandle<()>>,
}

impl ManifestWriter {
    pub fn record(&self, update: RecordUpdate) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(update);
        }
    }

    /// Flushes everything still queued and waits for the writer thread.
    pub fn finish(mut self) {
        self.close();
    }

    fn close(&mut self) {
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ManifestWriter {
    fn drop(&mut self) {
        self.close();
    }
}

fn write_batch(connection: &Connection, batch: &[RecordUpdate]) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    {
        let mut statement = transaction.prepare_cached(
            "INSERT INTO backup_files
                (profile_id, source_path, destination_path, size, modified_seconds, modified_ns, hash, status, copied_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(profile_id, source_path, destination_path) DO UPDATE SET
                size = excluded.size,
                modified_seconds = excluded.modified_seconds,
                modified_ns = excluded.modified_ns,
                hash = COALESCE(excluded.hash, CASE WHEN backup_files.size = excluded.size AND backup_files.modified_ns = excluded.modified_ns THEN backup_files.hash END),
                status = excluded.status,
                copied_at = excluded.copied_at",
        )?;
        for update in batch {
            statement.execute(params![
                update.profile_id,
                update.source_path,
                update.destination_path,
                update.size as i64,
                update.modified_ns.div_euclid(1_000_000_000),
                update.modified_ns,
                update.hash,
                update.status,
                update.recorded_at,
            ])?;
        }
    }
    transaction.commit()
}

fn add_column_if_missing(connection: &Connection, column: &str, definition: &str) -> Result<(), String> {
    let exists = connection
        .prepare("SELECT 1 FROM pragma_table_info('backup_files') WHERE name = ?1")
        .and_then(|mut statement| statement.exists([column]))
        .map_err(|error| error.to_string())?;
    if !exists {
        connection
            .execute_batch(&format!("ALTER TABLE backup_files ADD COLUMN {column} {definition}"))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(source: &str, hash: Option<&str>, status: &'static str) -> RecordUpdate {
        RecordUpdate {
            profile_id: "photos".to_string(),
            source_path: source.to_string(),
            destination_path: format!("/destination{source}"),
            size: 42,
            modified_ns: 123_000_000_456,
            hash: hash.map(ToString::to_string),
            status,
            recorded_at: "456".to_string(),
        }
    }

    #[test]
    fn writer_batches_records_into_a_snapshot() {
        let directory = std::env::temp_dir().join(format!("lightroom-backup-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let manifest = Manifest::open(&directory).expect("manifest should open");

        let writer = manifest.writer().unwrap();
        writer.record(update("/a.dng", Some("abc"), "copied"));
        writer.record(update("/b.dng", None, "copied"));
        writer.record(update("/gone.dng", None, "source-missing"));
        writer.finish();

        // A later metadata-only update keeps the known hash while size and mtime still match.
        let writer = manifest.writer().unwrap();
        writer.record(update("/a.dng", None, "copied"));
        writer.finish();

        let snapshot = manifest.snapshot("photos").unwrap();
        assert_eq!(snapshot.iter().count(), 2);
        let record = snapshot.get("/a.dng", "/destination/a.dng").unwrap();
        assert_eq!(record, &ManifestRecord { size: 42, modified_ns: Some(123_000_000_456), hash: Some("abc".to_string()) });

        manifest.forget("photos").unwrap();
        assert_eq!(manifest.snapshot("photos").unwrap().iter().count(), 0);
        fs::remove_dir_all(directory).expect("test manifest should be removed");
    }
}
