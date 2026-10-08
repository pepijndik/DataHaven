import { invoke } from "@tauri-apps/api/core";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import type { AndroidAlbum, AndroidDevice, AndroidProgress, AndroidSendResult, AndroidStorage, BackupSnapshot, DestinationInfo, PhoneSettings, BackupApi, BackupProfile, BackupProgress, BackupRunId, HealthCheckLevel, MountedVolume, VolumeHealth } from "../shared/types";

export const tauriApi: BackupApi = {
  getDefaultProfiles: () => invoke<BackupProfile[]>("load_profiles"),
  saveProfiles: (profiles) => invoke<void>("save_profiles", { profiles }),
  forgetJobHistory: (profileId) => invoke<void>("forget_job_history", { profileId }),
  confirm: (message, title) => ask(message, { title, kind: "warning", okLabel: "Remove", cancelLabel: "Keep" }),
  listPresets: () => invoke<BackupProfile[]>("list_presets"),
  selectDirectory: async (defaultPath) => {
    const selected = await open({ directory: true, multiple: false, defaultPath });
    return typeof selected === "string" ? selected : null;
  },
  detectVolumes: async () => invoke<MountedVolume[]>("detect_volumes"),
  detectAndroidDevices: async () => invoke<AndroidDevice[]>("detect_android_devices"),
  subscribeAndroidDevices: (listener) => listen<AndroidDevice[]>("android-devices", (event) => listener(event.payload)),
  listAndroidStorages: (deviceId) => invoke<AndroidStorage[]>("list_android_storages", { deviceId }),
  sendToAndroid: (request) => invoke<AndroidSendResult>("send_to_android", { request }),
  cancelAndroidSend: () => invoke<void>("cancel_android_send"),
  freeAndroidDevice: () => invoke<void>("free_android_device"),
  loadAndroidSettings: (deviceId) => invoke<PhoneSettings>("load_android_settings", { deviceId }),
  saveAndroidSettings: (deviceId, settings) => invoke<void>("save_android_settings", { deviceId, settings }),
  androidDeviceName: async (deviceId) => invoke<string | null>("android_device_name", { deviceId }),
  listAndroidAlbums: async (deviceId, storageId) => invoke<AndroidAlbum[]>("list_android_albums", { deviceId, storageId }),
  createAndroidAlbum: async (deviceId, storageId, name) => invoke<void>("create_android_album", { deviceId, storageId, name }),
  renameAndroidAlbum: async (deviceId, storageId, name, newName) =>
    invoke<void>("rename_android_album", { deviceId, storageId, name, newName }),
  subscribeAndroidProgress: (listener) => listen<AndroidProgress>("android-progress", (event) => listener(event.payload)),
  checkVolumeHealth: (path: string, level: HealthCheckLevel) => invoke<VolumeHealth>("check_volume_health", { path, level }),
  inspectDestination: (path) => invoke<DestinationInfo>("inspect_destination", { path }),
  createJobVolume: (path, name) => invoke<string>("create_job_volume", { path, name }),
  listSnapshots: (destinationRoot) => invoke<BackupSnapshot[]>("list_snapshots", { destinationRoot }),
  revealPath: (path) => invoke<void>("reveal_path", { path }),
  planBackup: (profile) => invoke("plan_backup", { profile }),
  runBackup: (profile, runId: BackupRunId) => invoke("run_backup", { profile, runId }),
  pauseBackup: (runId) => invoke("pause_backup", { runId }),
  resumeBackup: (runId) => invoke("resume_backup", { runId }),
  cancelBackup: (runId) => invoke("cancel_backup", { runId }),
  subscribeBackupProgress: async (listener) => {
    const unlisten = await listen<BackupProgress>("backup-progress", (event) => listener(event.payload));
    return unlisten;
  },
  listQnapDirectories: (profile) => invoke<string[]>("list_qnap_directories", { profile }),
  generateQnapCommands: async () => ({ smbUrl: "", macMountCommand: "", rsyncCommands: [], helperInstallCommands: [], helperManifestCommand: "" }),
};
