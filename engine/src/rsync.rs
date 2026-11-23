use crate::backup::{archive_run_name, BackupWarning, ProgressTracker, RunControl};
use crate::profile::{BackupMode, BackupProfile, QnapSettings};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::Duration;

pub struct RsyncOutcome {
    pub transferred_files: u64,
    pub errors: Vec<BackupWarning>,
}

/// Runs rsync over SSH for every source. Cancel kills rsync (`--partial` keeps the progress),
/// pause stops the process with SIGSTOP until resumed.
pub fn run(profile: &BackupProfile, control: &RunControl, tracker: &ProgressTracker, skip_live_catalogs: bool) -> Result<RsyncOutcome, String> {
    let qnap = profile.qnap.as_ref().ok_or("QNAP settings are missing")?;
    if qnap.host.trim().is_empty() || qnap.username.trim().is_empty() {
        return Err("Enter a QNAP host and username first".to_string());
    }
    let remote_root = remote_root(profile, qnap);
    let archive_run = archive_run_name();
    let mut outcome = RsyncOutcome { transferred_files: 0, errors: Vec::new() };

    for source in &profile.sources {
        if control.is_cancelled() {
            break;
        }
        let mirror = profile.layout == crate::profile::Layout::Mirror;
        let remote_directory = if mirror { remote_root.clone() } else { format!("{remote_root}/{}/{}", profile.id, source.id) };
        let mut command = Command::new("rsync");
        command
            .args(["-a", "--partial", "--protect-args", "--itemize-changes", "--human-readable", "-e"])
            .arg(format!("ssh -o BatchMode=yes -o ConnectTimeout=10 -p {}", qnap.ssh_port));
        for argument in filter_arguments(profile, skip_live_catalogs) {
            command.arg(argument);
        }
        if profile.mode == BackupMode::SafeMirror {
            let archive = if mirror { format!("{remote_root}/_Archive/{archive_run}") } else { format!("{remote_root}/_Archive/{archive_run}/{}", source.id) };
            command.arg("--backup").arg(format!("--backup-dir={archive}"));
            if mirror {
                command.arg("--exclude=/_Archive");
            }
        }
        command
            .arg(format!("{}/", source.path.trim_end_matches('/')))
            .arg(format!("{}@{}:{remote_directory}/", qnap.username, qnap.host))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|error| format!("Could not start rsync: {error}"))?;
        let stdout = child.stdout.take().expect("rsync stdout is piped");
        let stderr = child.stderr.take().expect("rsync stderr is piped");
        let status = std::thread::scope(|scope| {
            scope.spawn(|| {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    // Itemized lines look like ">f+++++++++ 2024/IMG_0001.CR3".
                    if let Some(path) = line.strip_prefix(">f").and_then(|rest| rest.split_once(' ')).map(|(_, path)| path) {
                        tracker.completed_files.fetch_add(1, Ordering::Relaxed);
                        tracker.set_current(path);
                    }
                }
            });
            let errors = scope.spawn(|| BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<_>>());
            let status = supervise(&mut child, control);
            (status, errors.join().unwrap_or_default())
        });

        match status {
            (Ok(status), _) if status.success() => {}
            (Ok(_), _) if control.is_cancelled() => break,
            (Ok(status), stderr) => outcome.errors.push(BackupWarning::danger(
                format!("rsync failed ({status}): {}", stderr.last().map(String::as_str).unwrap_or("no error output")),
                Some(std::path::Path::new(&source.path)),
            )),
            (Err(error), _) => return Err(format!("rsync stopped unexpectedly: {error}")),
        }
    }
    outcome.transferred_files = tracker.completed_files.load(Ordering::Relaxed);
    Ok(outcome)
}

fn supervise(child: &mut std::process::Child, control: &RunControl) -> std::io::Result<std::process::ExitStatus> {
    let mut stopped = false;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if control.is_cancelled() {
            if stopped { signal(child.id(), "CONT"); }
            let _ = child.kill();
            return child.wait();
        }
        if control.is_paused() != stopped {
            stopped = !stopped;
            signal(child.id(), if stopped { "STOP" } else { "CONT" });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn signal(pid: u32, signal: &str) {
    let _ = Command::new("kill").arg(format!("-{signal}")).arg(pid.to_string()).status();
}

fn filter_arguments(profile: &BackupProfile, skip_live_catalogs: bool) -> Vec<String> {
    let mut arguments: Vec<String> = [".DS_Store", "._*", ".Spotlight-V100", ".Trashes", ".fseventsd", "*.lbpart"]
        .iter()
        .map(|pattern| format!("--exclude={pattern}"))
        .collect();
    if skip_live_catalogs {
        arguments.extend(["*.lrcat", "*.lrcat-wal", "*.lrcat-shm", "*.lrcat-journal"].iter().map(|pattern| format!("--exclude={pattern}")));
    }
    arguments.extend(profile.filters.iter().filter(|filter| filter.action == "exclude").map(|filter| format!("--exclude={}", filter.pattern)));

    let includes: Vec<&str> = profile.filters.iter().filter(|filter| filter.action == "include").map(|filter| filter.pattern.as_str()).collect();
    if !includes.is_empty() {
        arguments.push("--include=*/".to_string());
        arguments.extend(includes.iter().map(|pattern| format!("--include={pattern}")));
        arguments.push("--exclude=*".to_string());
        arguments.push("--prune-empty-dirs".to_string());
    }
    arguments
}

fn remote_root(profile: &BackupProfile, qnap: &QnapSettings) -> String {
    let root = if profile.destination_root.starts_with("/share/") {
        profile.destination_root.clone()
    } else if qnap.remote_root.is_empty() {
        format!("/share/{}", qnap.share)
    } else {
        qnap.remote_root.clone()
    };
    root.trim_end_matches('/').to_string()
}

pub fn list_directories(profile: &BackupProfile) -> Result<Vec<String>, String> {
    let qnap = profile.qnap.as_ref().ok_or("QNAP settings are missing")?;
    if qnap.host.trim().is_empty() || qnap.username.trim().is_empty() {
        return Err("Enter a QNAP host and username first".to_string());
    }
    let root = if qnap.remote_root.is_empty() { format!("/share/{}", qnap.share) } else { qnap.remote_root.clone() };
    let command = format!("find {} -mindepth 1 -maxdepth 2 -type d -print", shell_quote(&root));
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-p"])
        .arg(qnap.ssh_port.to_string())
        .arg(format!("{}@{}", qnap.username, qnap.host))
        .arg(command)
        .output()
        .map_err(|error| format!("Could not start SSH: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).lines().map(str::trim).filter(|path| !path.is_empty()).map(ToString::to_string).collect())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_filters_become_an_rsync_allow_list() {
        let profile: BackupProfile = serde_json::from_value(serde_json::json!({
            "id": "job", "destinationRoot": "/share/Backups", "sources": [],
            "filters": [{ "pattern": "*.mov", "action": "include" }, { "pattern": "**/Cache/**", "action": "exclude" }]
        })).unwrap();
        let arguments = filter_arguments(&profile, true);
        assert!(arguments.contains(&"--exclude=*.lrcat".to_string()));
        assert!(arguments.contains(&"--exclude=**/Cache/**".to_string()));
        let tail: Vec<&str> = arguments.iter().rev().take(4).map(String::as_str).collect();
        assert_eq!(tail, vec!["--prune-empty-dirs", "--exclude=*", "--include=*.mov", "--include=*/"]);
    }
}
