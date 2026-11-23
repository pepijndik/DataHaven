//! The backup engine of Lightroom Backup Station: scanning, planning, copying, verification,
//! phones, NAS and volume tools. It has no window code, so the native egui window and the
//! Tauri window share exactly the same engine.

pub mod android;
pub mod backup;
pub mod filters;
pub mod manifest;
pub mod phone;
pub mod profile;
pub mod rsync;
pub mod runtime;
pub mod scan;
pub mod snapshots;
pub mod transfer;
pub mod volumes;
