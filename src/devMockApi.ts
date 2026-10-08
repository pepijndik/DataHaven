import type { BackupApi, BackupProfile } from "../shared/types";

/**
 * A fake backend for designing the UI in a normal browser: open the Vite dev server with `?mock`.
 * Only loaded in development builds, never shipped.
 */
const profiles: BackupProfile[] = [
  {
    id: "t7-shield-mirror",
    name: "T7 Shield backup",
    description: "Exact copy of the T7 Shield on the 4 TB HDD.",
    destinationKind: "local",
    destinationRoot: "/Volumes/Reservekopie van T7 Shield",
    mode: "safe-mirror",
    verification: "important-hash",
    preserveMacMetadata: true,
    layout: "mirror",
    versioning: "snapshots",
    archiveKeepDays: 90,
    parallelCopies: 2,
    guards: ["lightroom-closed"],
    filters: [
      { id: "previews", pattern: "**/*Previews.lrdata/**", action: "exclude", note: "Lightroom previews can be rebuilt." },
      { id: "recycle", pattern: "$RECYCLE.BIN", action: "exclude", note: "Windows recycle bin." },
    ],
    sources: [{ id: "t7", label: "T7 Shield", path: "/Volumes/T7 Shield", category: "other", kind: "folder" }],
  },
  {
    id: "phone-1",
    preset: "phone-camera-roll",
    name: "Back up Samsung Galaxy S24",
    description: "Camera roll to the T7.",
    destinationKind: "local",
    destinationRoot: "/Volumes/T7 Shield/Phone",
    mode: "safe-mirror",
    verification: "metadata",
    preserveMacMetadata: false,
    sources: [
      { id: "camera", label: "Camera", path: "DCIM/Camera", category: "photos", kind: "android", deviceId: "" },
      { id: "screenshots", label: "Screenshots", path: "Pictures/Screenshots", category: "photos", kind: "android", deviceId: "" },
    ],
  },
];

const delay = <T,>(value: T) => new Promise<T>((resolve) => setTimeout(() => resolve(value), 120));

export const devMockApi: BackupApi = {
  getDefaultProfiles: () => delay(profiles),
  saveProfiles: () => delay(undefined),
  listPresets: () => delay([]),
  forgetJobHistory: () => delay(undefined),
  confirm: async (message) => window.confirm(message),
  selectDirectory: async (defaultPath) => window.prompt("Folder", defaultPath ?? "/Volumes/T7 Shield/Exports"),
  detectVolumes: () =>
    delay([
      { name: "Backup", path: "/Volumes/Backup", filesystem: "apfs", capacity: 4e12, available: 3.5e12 },
      { name: "Reservekopie van T7 Shield", path: "/Volumes/Reservekopie van T7 Shield", filesystem: "apfs", capacity: 4e12, available: 3.5e12 },
      { name: "T7 Shield", path: "/Volumes/T7 Shield", filesystem: "exfat", capacity: 2e12, available: 1.34e12 },
    ]),
  detectAndroidDevices: () => delay([{ id: "R5CX", manufacturer: "SAMSUNG", model: "Galaxy S24", locationId: 1, vendorId: 1, productId: 1 }]),
  inspectDestination: async (path) =>
    delay(
      path.startsWith("/Volumes/T7")
        ? { filesystem: "exfat", snapshotsSupported: false, external: true, volumeName: "T7 Shield", isVolumeRoot: false, cccBackup: false, canCreateVolume: false }
        : { filesystem: "apfs", snapshotsSupported: true, external: true, volumeName: path.split("/")[2], isVolumeRoot: true, cccBackup: path.includes("Reservekopie"), canCreateVolume: true },
    ),
  listSnapshots: (root) =>
    delay(
      root.includes("Reservekopie")
        ? [
            { name: "2026-10-08_141210", path: `${root}/_Snapshots/2026-10-08_141210`, createdAt: 1791468730 },
            { name: "2026-10-01_090003", path: `${root}/_Snapshots/2026-10-01_090003`, createdAt: 1790845203 },
          ]
        : [],
    ),
  revealPath: () => delay(undefined),
  createJobVolume: (_path, name) => delay(`/Volumes/${name}`),
  planBackup: () => Promise.reject("The mock backend does not plan backups."),
  runBackup: () => Promise.reject("The mock backend does not run backups."),
  pauseBackup: () => delay(undefined),
  resumeBackup: () => delay(undefined),
  cancelBackup: () => delay(undefined),
  generateQnapCommands: async () => ({ smbUrl: "", macMountCommand: "", rsyncCommands: [], helperInstallCommands: [], helperManifestCommand: "" }),
};
