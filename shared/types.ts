export type BackupMode = "safe-mirror" | "append-only";

export type VerificationMode = "metadata" | "important-hash" | "full-hash";

export type DestinationKind = "local" | "qnap-smb" | "qnap-rsync" | "android";

/** Which phone to use. An empty deviceId means whichever phone is connected. */
export type PhoneTarget = { deviceId: string; storageId?: number | null };

export type ProfileSource = {
  id: string;
  label: string;
  path: string;
  category: "catalog" | "photos" | "video" | "exports" | "other";
  /** "android" reads `path` (for example DCIM/Camera) from a phone. */
  kind?: "folder" | "android";
  deviceId?: string;
  storageId?: number | null;
};

export type BackupFilterAction = "include" | "exclude";

export type BackupFilter = {
  id: string;
  pattern: string;
  action: BackupFilterAction;
  note: string;
};

export type QnapSettings = {
  host: string;
  share: string;
  username: string;
  remoteRoot: string;
  sshPort: number;
  useHelper: boolean;
};

export type BackupProfile = {
  id: string;
  name: string;
  description: string;
  sources: ProfileSource[];
  destinationKind: DestinationKind;
  destinationRoot: string;
  mode: BackupMode;
  verification: VerificationMode;
  preserveMacMetadata: boolean;
  filters?: BackupFilter[];
  qnap?: QnapSettings;
  /** The phone used when destinationKind is "android". destinationRoot is then a folder on the phone. */
  phone?: PhoneTarget;
  /** Preset this job was created from. */
  preset?: string;
  /** Safety checks before scanning, for example "lightroom-closed". */
  guards?: string[];
  /** Files hashed when verification is "important-hash". */
  importantPatterns?: string[];
  /** Files copied at the same time. Low for spinning drives and video, higher for SSDs and photos. */
  parallelCopies?: number;
  /** Check every destination file instead of trusting the backup history. Slower. */
  checkDestination?: boolean;
  /** Remove older versions (archive or snapshots) older than this many days. Keeps everything when unset. */
  archiveKeepDays?: number;
  /** "per-source" writes to destination/<source id>, "mirror" copies one source straight into the destination like CCC. */
  layout?: "per-source" | "mirror";
  /** "snapshots" clones the whole backup after each run (APFS only). "archive" moves replaced files to _Archive. */
  versioning?: "archive" | "snapshots";
  autoRun?: AutoRun;
  /** Unix seconds of the last finished run. */
  lastRunAt?: number;
};

export type AutoRun = {
  /** Run as soon as the destination drive (or the phone) is connected. */
  whenConnected?: boolean;
  /** Also run every this many hours while the destination is connected. */
  everyHours?: number | null;
};

export type DestinationInfo = {
  filesystem?: string | null;
  snapshotsSupported: boolean;
  external: boolean;
  volumeName?: string | null;
  mountPoint?: string | null;
  isVolumeRoot: boolean;
  /** Carbon Copy Cloner snapshots exist on this volume. */
  cccBackup: boolean;
  canCreateVolume: boolean;
};

export type BackupSnapshot = { name: string; path: string; createdAt: number };

export type PlanAction = "copy" | "update" | "skip";

export type PlanEntry = {
  action: PlanAction;
  sourcePath: string;
  destinationPath: string;
  relativePath: string;
  size: number;
  reason: string;
};

export type BackupWarning = {
  level: "info" | "warning" | "danger";
  message: string;
  path?: string;
};

export type BackupPlan = {
  profileId: string;
  destinationRoot: string;
  createdAt: string;
  /** Files that will be written, capped for large plans. */
  entries: PlanEntry[];
  entriesTruncated: boolean;
  warnings: BackupWarning[];
  totals: {
    scannedFiles: number;
    filesToWrite: number;
    filesSkipped: number;
    bytesToWrite: number;
    bytesSkipped: number;
    filesExcluded: number;
    missingFromSource: number;
  };
};

export type BackupRunResult = BackupPlan & {
  completedAt: string;
  copiedFiles: number;
  archivedFiles: number;
  failedFiles: number;
  cancelled: boolean;
  errors: BackupWarning[];
  /** Snapshot made after this run. */
  snapshot?: string | null;
};

export type MountedVolume = {
  name: string;
  path: string;
  filesystem: string;
  capacity?: number;
  available?: number;
};

export type AndroidDevice = {
  id: string;
  manufacturer: string;
  model: string;
  serialNumber?: string;
  locationId: number;
  vendorId: number;
  productId: number;
};

export type AndroidStorage = {
  id: number;
  description: string;
  capacity: number;
  free: number;
  writable: boolean;
};

export type AndroidSendIssue = { file: string; message: string };

export type AndroidSendResult = {
  sent: number;
  skipped: number;
  failed: number;
  /** A different file with the same name is already on the phone and was left alone. */
  conflicts: number;
  bytes: number;
  destination: string;
  dryRun: boolean;
  cancelled: boolean;
  issues: AndroidSendIssue[];
};

export type AndroidSendRequest = {
  deviceId: string;
  storageId?: number | null;
  source: string;
  album?: string | null;
  /** Only count what would be sent. */
  dryRun?: boolean;
};

/** What "Send to phone" remembers per phone. */
export type PhoneSettings = {
  source?: string | null;
  album?: string | null;
  storageId?: number | null;
};

export type AndroidAlbum = { name: string; files: number };

export type AndroidProgress = {
  file: string;
  done: number;
  total: number;
  bytes: number;
  totalBytes: number;
};

export type HealthCheckLevel = "quick" | "medium" | "full";

export type VolumeHealth = {
  path: string;
  level: HealthCheckLevel;
  status: "healthy" | "warning" | "failed";
  filesChecked: number;
  bytesRead: number;
  errors: string[];
  durationMs: number;
  note: string;
};

export type QnapCommandSet = {
  smbUrl: string;
  macMountCommand: string;
  rsyncCommands: string[];
  helperInstallCommands: string[];
  helperManifestCommand: string;
  remoteDirectories?: string[];
};

export type BackupRunId = string;

export type BackupProgress = {
  runId: BackupRunId;
  event: string;
  path?: string;
  discoveredFiles: number;
  evaluatedFiles: number;
  completedFiles: number;
  totalFiles: number;
  copiedBytes: number;
  totalBytes: number;
  elapsedSeconds: number;
  bytesPerSecond: number;
};

export type BackupApi = {
  getDefaultProfiles: () => Promise<BackupProfile[]>;
  saveProfiles?: (profiles: BackupProfile[]) => Promise<void>;
  /** Clears the backup history of a removed job. Backed up files stay on the drive. */
  forgetJobHistory?: (profileId: string) => Promise<void>;
  /** Native yes/no dialog. */
  confirm?: (message: string, title: string) => Promise<boolean>;
  listPresets?: () => Promise<BackupProfile[]>;
  selectDirectory: (defaultPath?: string) => Promise<string | null>;
  detectVolumes: () => Promise<MountedVolume[]>;
  detectAndroidDevices?: () => Promise<AndroidDevice[]>;
  subscribeAndroidDevices?: (listener: (devices: AndroidDevice[]) => void) => Promise<() => void>;
  listAndroidStorages?: (deviceId: string) => Promise<AndroidStorage[]>;
  sendToAndroid?: (request: AndroidSendRequest) => Promise<AndroidSendResult>;
  cancelAndroidSend?: () => Promise<void>;
  /** Stops apps that hold the phone (the macOS camera service, Android File Transfer). */
  freeAndroidDevice?: () => Promise<void>;
  loadAndroidSettings?: (deviceId: string) => Promise<PhoneSettings>;
  saveAndroidSettings?: (deviceId: string, settings: PhoneSettings) => Promise<void>;
  androidDeviceName?: (deviceId: string) => Promise<string | null>;
  listAndroidAlbums?: (deviceId: string, storageId?: number | null) => Promise<AndroidAlbum[]>;
  createAndroidAlbum?: (deviceId: string, storageId: number | null | undefined, name: string) => Promise<void>;
  renameAndroidAlbum?: (deviceId: string, storageId: number | null | undefined, name: string, newName: string) => Promise<void>;
  subscribeAndroidProgress?: (listener: (progress: AndroidProgress) => void) => Promise<() => void>;
  checkVolumeHealth?: (path: string, level: HealthCheckLevel) => Promise<VolumeHealth>;
  inspectDestination?: (path: string) => Promise<DestinationInfo>;
  /** Adds an APFS volume for a job next to `path` and returns its mount point. */
  createJobVolume?: (path: string, name: string) => Promise<string>;
  listSnapshots?: (destinationRoot: string) => Promise<BackupSnapshot[]>;
  revealPath?: (path: string) => Promise<void>;
  planBackup: (profile: BackupProfile) => Promise<BackupPlan>;
  runBackup: (profile: BackupProfile, runId: BackupRunId) => Promise<BackupRunResult>;
  pauseBackup: (runId: BackupRunId) => Promise<void>;
  resumeBackup: (runId: BackupRunId) => Promise<void>;
  cancelBackup: (runId: BackupRunId) => Promise<void>;
  subscribeBackupProgress?: (listener: (progress: BackupProgress) => void) => Promise<() => void>;
  listQnapDirectories?: (profile: BackupProfile) => Promise<string[]>;
  generateQnapCommands: (profile: BackupProfile) => Promise<QnapCommandSet>;
};
