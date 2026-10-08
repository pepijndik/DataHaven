# Lightroom Backup Station

Lightroom Backup Station is a macOS and Linux Tauri desktop app for backing up Lightroom catalogs, original photos, and video archives to user configured drives and NAS destinations.

## What works now

1. A React and Tauri desktop UI with configurable backup jobs that are saved between launches.
2. A native Rust engine with a parallel scanner that skips excluded folders without entering them.
3. A SQLite backup history (WAL, batched writes) so unchanged files are skipped without touching the destination.
4. Resumable copies through `.lbpart` files that are flushed to disk before they replace the backup.
5. Safe mirror mode, which moves the previous destination file into `_Archive/<run>` before replacing it, with optional retention.
6. BLAKE3 verification for important files or for every file.
7. Lightroom catalog protection, plus presets for Lightroom, photo archives, and video archives.
8. Read only volume health checks at quick, medium, and full levels.
9. QNAP rsync over SSH as a destination type, with pause, cancel, filters, and archiving.
10. Android phones (MTP) as a backup source, for example the camera roll, screenshots, and WhatsApp media, and as a destination folder on the phone.
11. One click "Send to phone" that remembers the source folder, album, and storage per phone.

## Phones

A job source can be a folder on an Android phone (`"kind": "android"`, `"path": "DCIM/Camera"`). Leave `deviceId` empty to use whichever phone is connected. Phone files are copied one at a time over the single USB connection while local files keep copying in parallel. Files deleted on the phone stay in the backup and are reported. The bundled `phone-camera-roll` preset backs up the camera, screenshots, and WhatsApp media.

A job can also back up to a phone with `"destinationKind": "android"`. Then `destinationRoot` is a folder on the phone (for example `Backup`) and `phone.deviceId` picks the phone. Changed files are replaced on the phone, which keeps no archive.

On macOS the camera service (`ptpcamerad`) and Android File Transfer can hold the phone. The app shows a "Free the phone" button when that happens.

## Presets

Presets are JSON profile templates. The bundled ones live in `src-tauri/presets`. Drop extra `*.json` files into the app data folder under `presets` to add your own or override a bundled preset with the same `id`. Useful fields:

| Field | Meaning |
| --- | --- |
| `filters` | Include and exclude patterns. Patterns without `/` match a file or folder name anywhere, patterns with `/` match the path inside the source. Matching ignores case. |
| `verification` | `metadata`, `important-hash`, or `full-hash`. |
| `importantPatterns` | Files hashed in `important-hash` mode. |
| `parallelCopies` | Files copied at the same time. Use 1 or 2 for spinning drives and video, 4 to 8 for SSDs and photos. |
| `guards` | Safety checks before a run, currently `lightroom-closed`. |
| `checkDestination` | Stat every destination file instead of trusting the backup history. |
| `archiveKeepDays` | Remove archived versions older than this many days. |

## Commands

```bash
npm install
npm run tauri:dev
npm run tauri:build
npm run test
npm run typecheck
```

## Fast QNAP path

Use rsync over SSH when possible. QNAP Hybrid Backup Sync can enable rsync services. The app also supports mounted folders through the normal local destination flow.

The optional helper script remains available for NAS side manifest inspection.

## Working like Carbon Copy Cloner

1. **Mirror layout** (`"layout": "mirror"`) copies one source straight into the destination instead of `<destination>/<source id>`. Pointing a mirror job at an existing Carbon Copy Cloner backup continues it: files that are already there with the same size and date are recognised and not copied again. The app detects CCC volumes and offers this.
2. **Snapshots** (`"versioning": "snapshots"`, APFS drives only). After every run that changed something, the whole backup is cloned to `_Snapshots/<date>`. Clones share their data with the backup, so a snapshot costs space only for files that change later. Snapshots are normal folders: browse them in Finder and copy back what you need. `archiveKeepDays` removes old snapshots and always keeps the newest.
3. **A separate volume per job.** On an external APFS drive the app can add an APFS volume for a job, like CCC does. Volumes share the free space of the drive, nothing is reserved.
4. **Automatic runs.** `"autoRun": { "whenConnected": true, "everyHours": 24 }` starts a job when its drive or phone is connected (at most once per 15 minutes) and on a schedule while the app is open.

Real APFS snapshots, as CCC makes them, need an entitlement that Apple grants on request, so this app uses clones instead. They are not available on exFAT drives such as a T7 formatted for Windows, there the `_Archive` folder keeps older versions.

## Safety model

1. Normal runs compare size and modified time with the backup history, and fall back to the destination file when the history does not know it. Timestamps within 2 seconds count as equal, so FAT, exFAT, and SMB drives do not cause endless recopies.
2. Important files can also be hash checked before being skipped, and copies are verified after writing.
3. Existing destination files are moved into `_Archive` before replacement.
4. Source deletions are never propagated. They are reported and kept in the backup.
5. Lightroom catalog files are skipped while Lightroom Classic appears active.
6. A run refuses to start when the destination drive is not mounted or overlaps a source folder.
