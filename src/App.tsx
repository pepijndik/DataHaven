import {
  Archive,
  ArrowUpRight,
  Check,
  ChevronRight,
  CircleAlert,
  Cloud,
  Database,
  FolderOpen,
  HardDrive,
  History,
  Loader2,
  Pause,
  Play,
  RefreshCw,
  Settings as SettingsIcon,
  ShieldCheck,
  Smartphone,
  SlidersHorizontal,
  Sparkles,
  Wifi,
  X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type {
  AndroidAlbum,
  AndroidDevice,
  AndroidProgress,
  AndroidSendResult,
  AndroidStorage,
  BackupPlan,
  BackupSnapshot,
  DestinationInfo,
  BackupProfile,
  BackupProgress,
  BackupRunResult,
  HealthCheckLevel,
  MountedVolume,
  PhoneSettings,
  ProfileSource,
  QnapCommandSet,
  VolumeHealth,
} from "../shared/types";
import { errorText, formatBytes, getApi, titleCase } from "./helpers";
import { JobEditor } from "./JobEditor";

type View = "overview" | "settings";
type Tone = "sun" | "leaf" | "sky" | "ink";

const fallbackProfiles: BackupProfile[] = [
  {
    id: "demo",
    name: "Add your first backup",
    description: "Connect a drive and choose a folder to begin.",
    sources: [],
    destinationKind: "local",
    destinationRoot: "",
    mode: "safe-mirror",
    verification: "metadata",
    preserveMacMetadata: true,
  },
];

const jobDetails: Record<
  string,
  { role: string; tone: Tone; icon: typeof HardDrive; summary: string }
> = {
  "lightroom-optimized": {
    role: "Optimized preset",
    tone: "sun",
    icon: Database,
    summary: "Catalogs, sidecars, originals, and safe exclusions",
  },
  "media-archive": {
    role: "Photo archive",
    tone: "leaf",
    icon: HardDrive,
    summary: "Any photo or dated archive folder",
  },
  "phone-camera-roll": {
    role: "Phone backup",
    tone: "sky",
    icon: Smartphone,
    summary: "Camera, screenshots and WhatsApp media from the phone",
  },
  "video-archive": {
    role: "Video archive",
    tone: "sky",
    icon: HardDrive,
    summary: "Large video files with few parallel streams",
  },
  "lightroom-catalogs-t7": {
    role: "Catalog safety",
    tone: "sun",
    icon: Database,
    summary: "Catalogs, previews, and sidecars",
  },
  "photos-t9": {
    role: "Working library",
    tone: "leaf",
    icon: HardDrive,
    summary: "Full photo library and originals",
  },
  "video-qnap": {
    role: "Archive copy",
    tone: "sky",
    icon: Cloud,
    summary: "Video archive on the QNAP NAS",
  },
};

const qnapProfile: BackupProfile = {
  id: "qnap-archive",
  name: "QNAP Archive",
  description: "Secondary view-only archive for photo and video protection.",
  sources: [
    {
      id: "archive",
      label: "Photo and video archive",
      path: "~/Pictures + ~/Movies",
      category: "other",
    },
  ],
  destinationKind: "qnap-rsync",
  destinationRoot: "QNAP / Media / Lightroom",
  mode: "append-only",
  verification: "metadata",
  preserveMacMetadata: false,
};

function detailFor(profile: BackupProfile) {
  return jobDetails[profile.id] ?? (profile.preset ? jobDetails[profile.preset] : undefined);
}

const phonePresetFallback: BackupProfile = {
  id: "phone-camera-roll",
  preset: "phone-camera-roll",
  name: "Phone Camera Roll",
  description: "Backs up the photos, videos, screenshots and WhatsApp media of the connected Android phone to a drive.",
  destinationKind: "local",
  destinationRoot: "",
  mode: "safe-mirror",
  verification: "metadata",
  preserveMacMetadata: false,
  sources: [
    { id: "camera", label: "Camera", path: "DCIM/Camera", category: "photos", kind: "android", deviceId: "" },
    { id: "screenshots", label: "Screenshots", path: "Pictures/Screenshots", category: "photos", kind: "android", deviceId: "" },
  ],
};

/** A job can run when its destination and every source are reachable right now. */
function jobAvailable(profile: BackupProfile, volumes: MountedVolume[], phones: AndroidDevice[]) {
  if (profile.id === "demo" || !profile.destinationRoot || profile.sources.length === 0) return false;
  const mounted = (path: string) =>
    !path.startsWith("/Volumes/") || volumes.some((volume) => path === volume.path || path.startsWith(`${volume.path}/`));
  const destinationReady =
    profile.destinationKind === "android"
      ? phones.length > 0
      : profile.destinationKind === "qnap-rsync" || mounted(profile.destinationRoot);
  return (
    destinationReady &&
    profile.sources.every((source) => (source.kind === "android" ? phones.length > 0 : Boolean(source.path) && mounted(source.path)))
  );
}


export default function App() {
  const [view, setView] = useState<View>("overview");
  const [profiles, setProfiles] = useState<BackupProfile[]>(fallbackProfiles);
  const [selectedId, setSelectedId] = useState("lightroom-optimized");
  const [volumes, setVolumes] = useState<MountedVolume[]>([]);
  const [androidDevices, setAndroidDevices] = useState<AndroidDevice[]>([]);
  const [plan, setPlan] = useState<BackupPlan | null>(null);
  const [runResult, setRunResult] = useState<BackupRunResult | null>(null);
  const [qnapCommands, setQnapCommands] = useState<QnapCommandSet | null>(null);
  const [qnapFolders, setQnapFolders] = useState<string[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [runId, setRunId] = useState<string | null>(null);
  const [progress, setProgress] = useState<BackupProgress | null>(null);
  const [health, setHealth] = useState<VolumeHealth | null>(null);
  const [message, setMessage] = useState("Ready for a backup");
  const profilesLoaded = useRef(false);
  const busyRef = useRef(false);
  const latest = useRef({ profiles, volumes, androidDevices, busy });
  latest.current = { profiles, volumes, androidDevices, busy };
  const wasAvailable = useRef<Record<string, boolean>>({});
  const selectedProfile = useMemo(
    () => profiles.find((profile) => profile.id === selectedId) ?? profiles[0],
    [profiles, selectedId],
  );

  useEffect(() => {
    void loadProfiles();
  }, []);

  // Automatic runs: when a job's drive (or phone) is connected, and on a schedule. One run at a time.
  useEffect(() => {
    function tick() {
      const { profiles, volumes, androidDevices, busy } = latest.current;
      if (!profilesLoaded.current || busy !== null || busyRef.current) return;
      const now = Math.floor(Date.now() / 1000);
      for (const profile of profiles) {
        const autoRun = profile.autoRun;
        if (!autoRun?.whenConnected && !autoRun?.everyHours) continue;
        const available = jobAvailable(profile, volumes, androidDevices);
        const appeared = available && !wasAvailable.current[profile.id];
        wasAvailable.current[profile.id] = available;
        if (!available) continue;
        const sinceLastRun = now - (profile.lastRunAt ?? 0);
        // A drive that is plugged in and out quickly should not start a backup every time.
        const connectedDue = autoRun.whenConnected && appeared && sinceLastRun > 15 * 60;
        const scheduleDue = autoRun.everyHours ? sinceLastRun >= autoRun.everyHours * 3600 : false;
        if (connectedDue || scheduleDue) {
          void runBackup(profile, true);
          return;
        }
      }
    }
    tick();
    const timer = window.setInterval(tick, 60_000);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [volumes, androidDevices, busy]);

  useEffect(() => {
    const api = getApi();
    if (!profilesLoaded.current || !api?.saveProfiles) return;
    const timer = window.setTimeout(() => {
      api
        .saveProfiles?.(profiles.filter((profile) => profile.id !== "demo"))
        .catch((error) =>
          setMessage(
            errorText(error, "Could not save backup jobs."),
          ),
        );
    }, 400);
    return () => window.clearTimeout(timer);
  }, [profiles]);

  useEffect(() => {
    void detectVolumes(true);
    void detectAndroidDevices(true);
    const timer = window.setInterval(() => void detectVolumes(true), 4000);
    // Phones are pushed by the backend when plugged in or removed, so they are not polled.
    let unsubscribe: (() => void) | undefined;
    let disposed = false;
    void getApi()
      ?.subscribeAndroidDevices?.(setAndroidDevices)
      .then((cleanup) => (disposed ? cleanup() : (unsubscribe = cleanup)));
    return () => {
      disposed = true;
      window.clearInterval(timer);
      unsubscribe?.();
    };
  }, []);

  useEffect(() => {
    const api = getApi();
    if (!api?.subscribeBackupProgress) return;
    let unsubscribe: (() => void) | undefined;
    void api
      .subscribeBackupProgress((nextProgress) => {
        if (!runId || nextProgress.runId !== runId) return;
        if (nextProgress.event === "paused")
          return setMessage("Backup paused. Partial copies resume where they stopped.");
        if (nextProgress.event === "resumed") return setMessage("Backup resumed.");
        if (nextProgress.event === "cancelled")
          return setMessage("Stopping. Partial copies resume on the next run.");
        setProgress(nextProgress);
      })
      .then((cleanup) => {
        unsubscribe = cleanup;
      });
    return () => unsubscribe?.();
  }, [runId]);

  async function loadProfiles() {
    const api = getApi();
    if (!api) {
      setMessage("Preview mode. Connect the desktop app for file access.");
      return;
    }
    setBusy("profiles");
    try {
      const defaults = await api.getDefaultProfiles();
      setProfiles(defaults.length ? defaults : fallbackProfiles);
      profilesLoaded.current = true;
      setSelectedId(
        defaults.find((profile) => profile.id === "lightroom-optimized")?.id ??
          defaults[0]?.id ??
          "demo",
      );
      setMessage("Your backup jobs are ready.");
    } catch (error) {
      setMessage(
        errorText(error, "Could not load backup jobs."),
      );
    } finally {
      setBusy(null);
    }
  }

  async function detectVolumes(silent = false) {
    const api = getApi();
    if (!api) {
      setMessage("Volume detection is available in the desktop app.");
      return;
    }
    if (!silent) setBusy("volumes");
    try {
      setVolumes(await api.detectVolumes());
      if (!silent) setMessage("Storage connections refreshed.");
    } finally {
      if (!silent) setBusy(null);
    }
  }

  async function detectAndroidDevices(silent = false) {
    const api = getApi();
    if (!api?.detectAndroidDevices) return;
    try {
      const devices = await api.detectAndroidDevices();
      setAndroidDevices(devices);
      if (!silent && devices.length > 0) {
        setMessage(`${devices.length} Android device${devices.length === 1 ? "" : "s"} connected.`);
      }
    } catch (error) {
      if (!silent) setMessage(errorText(error, "Could not detect Android devices."));
    }
  }

  async function checkHealth(path: string, level: HealthCheckLevel) {
    const api = getApi();
    if (!api?.checkVolumeHealth) {
      setMessage("Health checks are available in the Tauri desktop app.");
      return;
    }
    setBusy(`health-${level}`);
    try {
      const result = await api.checkVolumeHealth(path, level);
      setHealth(result);
      setMessage(
        `${level[0].toUpperCase()}${level.slice(1)} health check finished.`,
      );
    } catch (error) {
      setMessage(
        errorText(error, "Health check failed."),
      );
    } finally {
      setBusy(null);
    }
  }

  async function chooseDestination() {
    const api = getApi();
    if (!api || !selectedProfile) return;
    const directory = await api.selectDirectory();
    if (directory) {
      updateProfile({
        ...selectedProfile,
        destinationKind: "local",
        destinationRoot: directory,
      });
      setMessage("Destination updated.");
    }
  }

  async function addDrive() {
    const api = getApi();
    if (!api || !selectedProfile || selectedProfile.id === "demo") {
      setMessage("Choose a configured job before adding a drive.");
      return;
    }
    const directory = await api.selectDirectory();
    if (!directory) return;
    const driveName =
      directory.split("/").filter(Boolean).at(-1) ?? "New drive";
    const newProfile: BackupProfile = {
      ...selectedProfile,
      id: `${selectedProfile.id}-${Date.now()}`,
      name: `${selectedProfile.name} on ${driveName}`,
      destinationKind: "local",
      destinationRoot: directory,
    };
    setProfiles((current) => [...current, newProfile]);
    setSelectedId(newProfile.id);
    setMessage(`${driveName} added as a backup destination.`);
  }

  function createJobForVolume(volume: MountedVolume) {
    const newProfile: BackupProfile = {
      id: `volume-${Date.now()}`,
      name: `Backup to ${volume.name}`,
      description: `New backup job targeting the newly detected ${volume.name} volume.`,
      destinationKind: "local",
      destinationRoot: volume.path,
      mode: "safe-mirror",
      verification: "metadata",
      preserveMacMetadata: true,
      sources: [
        {
          id: "source",
          label: "Choose a source folder",
          path: "",
          category: "other",
        },
      ],
    };
    setProfiles((current) => [...current, newProfile]);
    setSelectedId(newProfile.id);
    setView("settings");
    setMessage(`${volume.name} is ready to configure as a backup job.`);
  }

  async function createPhoneBackupJob(device?: AndroidDevice) {
    const presets = await getApi()?.listPresets?.().catch(() => []);
    const preset = presets?.find((item) => item.id === "phone-camera-roll") ?? phonePresetFallback;
    const phoneName = device ? `${titleCase(device.manufacturer)} ${device.model}` : "phone";
    const newProfile: BackupProfile = {
      ...preset,
      id: `phone-${Date.now()}`,
      preset: "phone-camera-roll",
      name: `Back up ${phoneName}`,
      destinationRoot: volumes[0]?.path ?? "",
      sources: preset.sources.map((source) => ({ ...source, deviceId: device?.id ?? "" })),
    };
    setProfiles((current) => [...current, newProfile]);
    setSelectedId(newProfile.id);
    setView("settings");
    setMessage(
      newProfile.destinationRoot
        ? `Phone backup job created on ${volumes[0]?.name}. Check the folders, then run it.`
        : "Phone backup job created. Choose the drive to back up to.",
    );
  }

  function createQnapJob() {
    const newProfile: BackupProfile = {
      id: `qnap-${Date.now()}`,
      name: "QNAP backup job",
      description: "A network backup job using QNAP rsync over SSH.",
      destinationKind: "qnap-rsync",
      destinationRoot: "/share/Backups",
      mode: "safe-mirror",
      verification: "metadata",
      preserveMacMetadata: false,
      qnap: {
        host: "",
        share: "",
        username: "",
        remoteRoot: "/share/Backups",
        sshPort: 22,
        useHelper: true,
      },
      sources: [
        {
          id: "source",
          label: "Choose a source folder",
          path: "",
          category: "other",
        },
      ],
    };
    setProfiles((current) => [...current, newProfile]);
    setSelectedId(newProfile.id);
    setView("settings");
    setMessage("QNAP job created. Enter the NAS connection details.");
  }

  async function makePlan() {
    const api = getApi();
    if (!api || !selectedProfile) {
      setMessage("Dry runs are available in the desktop app.");
      return;
    }
    setBusy("plan");
    setRunResult(null);
    try {
      const nextPlan = await api.planBackup(selectedProfile);
      setPlan(nextPlan);
      setMessage(
        `${nextPlan.totals.filesToWrite} files are ready to be written.`,
      );
    } catch (error) {
      setMessage(errorText(error, "Dry run failed."));
    } finally {
      setBusy(null);
    }
  }

  async function runBackup(profile = selectedProfile, automatic = false) {
    const api = getApi();
    if (!api || !profile || profile.id === "demo") {
      setMessage("Choose a configured job first.");
      return;
    }
    const nextRunId = crypto.randomUUID();
    setRunId(nextRunId);
    setProgress(null);
    setBusy("run");
    busyRef.current = true;
    if (automatic) {
      setSelectedId(profile.id);
      setMessage(`Automatic backup started: ${profile.name}.`);
    }
    try {
      const result = await api.runBackup(profile, nextRunId);
      setPlan(result);
      setRunResult(result);
      if (!result.cancelled) {
        const lastRunAt = Math.floor(Date.now() / 1000);
        setProfiles((current) => current.map((item) => (item.id === profile.id ? { ...item, lastRunAt } : item)));
      }
      const versions = result.snapshot ? ` Snapshot ${result.snapshot} saved.` : "";
      setMessage(
        result.failedFiles === 0
          ? `${result.copiedFiles} files backed up successfully.${versions}`
          : `${result.failedFiles} files need attention.${versions}`,
      );
    } catch (error) {
      setMessage(errorText(error, "Backup failed."));
    } finally {
      busyRef.current = false;
      setBusy(null);
      setRunId(null);
    }
  }

  async function pauseBackup() {
    const api = getApi();
    if (!api || !runId) return;
    await api.pauseBackup(runId);
    setBusy("paused");
  }

  async function resumeBackup() {
    const api = getApi();
    if (!api || !runId) return;
    await api.resumeBackup(runId);
    setBusy("run");
  }

  async function cancelBackup() {
    const api = getApi();
    if (!api || !runId) return;
    await api.cancelBackup(runId);
  }

  async function generateQnap() {
    const api = getApi();
    if (!api || !selectedProfile) return;
    setBusy("qnap");
    try {
      setQnapCommands(await api.generateQnapCommands(selectedProfile));
      setMessage("QNAP connection commands are ready.");
    } finally {
      setBusy(null);
    }
  }

  async function loadQnapFolders() {
    const api = getApi();
    if (!api?.listQnapDirectories || !selectedProfile) {
      setMessage(
        "QNAP folder discovery is available in the Tauri desktop app.",
      );
      return;
    }
    setBusy("qnap-folders");
    try {
      setQnapFolders(await api.listQnapDirectories(selectedProfile));
      setMessage("QNAP folders loaded.");
    } catch (error) {
      setMessage(
        errorText(error, "Could not load QNAP folders."),
      );
    } finally {
      setBusy(null);
    }
  }

  async function removeProfile(profile: BackupProfile) {
    const api = getApi();
    if (!api || busy === "run" || busy === "paused") {
      setMessage("Stop the running backup before removing a job.");
      return;
    }
    const message = `Remove "${profile.name}"? Files it already backed up stay on the drive. Only the job and its history are removed.`;
    const confirmed = api.confirm ? await api.confirm(message, "Remove job") : window.confirm(message);
    if (!confirmed) return;
    try {
      await api.forgetJobHistory?.(profile.id);
    } catch (error) {
      setMessage(errorText(error, "Could not remove the job."));
      return;
    }
    const remaining = profiles.filter((item) => item.id !== profile.id);
    setProfiles(remaining.length ? remaining : fallbackProfiles);
    setSelectedId(remaining[0]?.id ?? "demo");
    setPlan(null);
    setRunResult(null);
    setMessage(`${profile.name} removed.`);
  }

  function updateProfile(nextProfile: BackupProfile) {
    setProfiles((current) =>
      current.map((profile) =>
        profile.id === nextProfile.id ? nextProfile : profile,
      ),
    );
  }
  const isBusy = busy !== null;

  return (
    <main className="appShell">
      <aside className="sidebar">
        <div className="brandBlock">
          <div className="brandMark">
            <Sparkles size={21} />
          </div>
          <div>
            <p className="brandKicker">Private archive</p>
            <h1>
              Lightroom
              <br />
              Backup
            </h1>
          </div>
        </div>
        <nav className="primaryNav" aria-label="Main navigation">
          <button
            className={view === "overview" ? "navItem active" : "navItem"}
            type="button"
            onClick={() => setView("overview")}
          >
            <HardDrive size={18} /> Overview
          </button>
          <button
            className={view === "settings" ? "navItem active" : "navItem"}
            type="button"
            onClick={() => setView("settings")}
          >
            <SettingsIcon size={18} /> Settings
          </button>
        </nav>
        {runId ? (
          <div className="runControls">
            <span>
              {busy === "paused"
                ? "Paused"
                : progress?.path
                  ? progress.path.split("/").at(-1)
                  : "Backup running"}
            </span>
            <div>
              <button
                type="button"
                onClick={busy === "paused" ? resumeBackup : pauseBackup}
              >
                {busy === "paused" ? <Play size={13} /> : <Pause size={13} />}{" "}
                {busy === "paused" ? "Resume" : "Pause"}
              </button>
              <button type="button" onClick={cancelBackup}>
                <X size={13} /> Stop
              </button>
            </div>
          </div>
        ) : null}
        <div className="sidebarDivider" />
        <div className="sidebarLabel">Storage map</div>
        <div className="storageMiniList">
          {volumes.length > 0 ? (
            volumes
              .slice(0, 5)
              .map((volume) => (
                <StorageMini
                  key={volume.path}
                  name={volume.name}
                  detail={volume.filesystem}
                  tone="leaf"
                  connected
                />
              ))
          ) : (
            <p className="storageEmpty">No drives mounted</p>
          )}
        </div>
        <div className="sidebarBottom">
          <div className="statusMark">
            <Check size={14} />
          </div>
          <div>
            <strong>{message}</strong>
            <span>Backup station</span>
          </div>
        </div>
      </aside>
      <section className="workspace">
        {view === "overview" ? (
          <Overview
            profiles={profiles}
            selectedProfile={selectedProfile}
            selectedId={selectedId}
            setSelectedId={setSelectedId}
            plan={plan}
            runResult={runResult}
            progress={progress}
            volumes={volumes}
            androidDevices={androidDevices}
            setMessage={setMessage}
            busy={busy}
            isBusy={isBusy}
            onRun={() => void runBackup()}
            onPlan={makePlan}
            onRefresh={detectVolumes}
            onCreateJob={createJobForVolume}
            onBackupPhone={createPhoneBackupJob}
            onSettings={() => setView("settings")}
          />
        ) : (
          <SettingsPage
            profiles={profiles}
            selectedProfile={selectedProfile}
            selectedId={selectedId}
            setSelectedId={setSelectedId}
            volumes={volumes}
            androidDevices={androidDevices}
            busy={busy}
            health={health}
            qnapCommands={qnapCommands}
            qnapFolders={qnapFolders}
            onChange={updateProfile}
            onRemove={removeProfile}
            onChooseDestination={chooseDestination}
            onAddDrive={addDrive}
            onAddQnap={createQnapJob}
            onAddPhone={() => void createPhoneBackupJob(androidDevices[0])}
            onGenerateQnap={generateQnap}
            onLoadQnapFolders={loadQnapFolders}
            onHealthCheck={checkHealth}
            onRefresh={detectVolumes}
            onBack={() => setView("overview")}
          />
        )}
      </section>
    </main>
  );
}

function Overview({
  profiles,
  selectedProfile,
  selectedId,
  setSelectedId,
  plan,
  runResult,
  progress,
  volumes,
  androidDevices,
  setMessage,
  busy,
  isBusy,
  onRun,
  onPlan,
  onRefresh,
  onCreateJob,
  onBackupPhone,
  onSettings,
}: {
  profiles: BackupProfile[];
  selectedProfile?: BackupProfile;
  selectedId: string;
  setSelectedId: (id: string) => void;
  plan: BackupPlan | null;
  runResult: BackupRunResult | null;
  progress: BackupProgress | null;
  volumes: MountedVolume[];
  androidDevices: AndroidDevice[];
  setMessage: (message: string) => void;
  busy: string | null;
  isBusy: boolean;
  onBackupPhone: (device?: AndroidDevice) => void;
  onRun: () => void;
  onPlan: () => void;
  onRefresh: () => void;
  onCreateJob: (volume: MountedVolume) => void;
  onSettings: () => void;
}) {
  return (
    <>
      {progress ? <TransferProgress progress={progress} /> : null}
      <header className="pageHeader">
        <div>
          <p className="eyebrow">Wednesday, 08 October</p>
          <h2>
            Keep the library
            <br />
            <em>quietly safe.</em>
          </h2>
          <p className="lede">
            One place to protect your Lightroom catalog, originals, and video
            archive.
          </p>
        </div>
        <div className="headerActions">
          <button
            className="iconButton"
            title="Refresh connected storage"
            type="button"
            onClick={onRefresh}
            disabled={isBusy}
          >
            {busy === "volumes" ? (
              <Loader2 className="spin" size={18} />
            ) : (
              <RefreshCw size={18} />
            )}
          </button>
          <button
            className="secondaryButton"
            type="button"
            onClick={onSettings}
          >
            <SlidersHorizontal size={17} /> Settings
          </button>
          <button
            className="primaryButton"
            type="button"
            onClick={onRun}
            disabled={
              isBusy || !selectedProfile || selectedProfile.id === "demo"
            }
          >
            {busy === "run" ? (
              <Loader2 className="spin" size={18} />
            ) : (
              <Play size={17} fill="currentColor" />
            )}{" "}
            Back up now
          </button>
        </div>
      </header>
      <section className="heroStatus">
        <div className="heroStatusIcon">
          <ShieldCheck size={25} />
        </div>
        <div>
          <span className="eyebrow">Current state</span>
          <h3>
            {runResult?.failedFiles
              ? "Needs your attention"
              : "Everything is in order"}
          </h3>
        </div>
        <div className="heroStatusMeta">
          <strong>
            {runResult
              ? `${runResult.copiedFiles} files written`
              : "No backup run yet"}
          </strong>
          <span>
            {runResult
              ? `Completed ${formatDate(runResult.completedAt)}`
              : "Preview changes before the first run"}
          </span>
        </div>
        <button
          className="textButton"
          type="button"
          onClick={onPlan}
          disabled={isBusy}
        >
          Preview changes <ArrowUpRight size={16} />
        </button>
      </section>
      <div className="sectionHeading">
        <div>
          <p className="eyebrow">Backup jobs</p>
          <h3>Four quiet safety nets</h3>
        </div>
        <span className="sectionNote">Write only what changed</span>
      </div>
      <section className="jobGrid">
        {profiles
          .filter((profile) => profile.id !== "demo")
          .map((profile, index) => (
            <JobCard
              key={profile.id}
              profile={profile}
              selected={selectedId === profile.id}
              order={index + 1}
              onSelect={() => setSelectedId(profile.id)}
            />
          ))}
      </section>
      <section className="lowerGrid">
        <div className="surface activitySurface">
          <div className="surfaceHeader">
            <div>
              <p className="eyebrow">Latest activity</p>
              <h3>
                {plan ? "The next run is ready" : "Your first run is waiting"}
              </h3>
            </div>
            <History size={19} />
          </div>
          {plan ? (
            <PlanSummary plan={plan} runResult={runResult} />
          ) : (
            <EmptyActivity onPlan={onPlan} />
          )}
        </div>
        <div className="surface storageSurface">
          <div className="surfaceHeader">
            <div>
              <p className="eyebrow">Connected now</p>
              <h3>{volumes.length} storage locations</h3>
            </div>
            <Wifi size={19} />
          </div>
          <div className="storageRows">
            {volumes.slice(0, 4).map((volume) => (
              <div className="storageRow" key={volume.path}>
                <span className="driveDot connected" />
                <div>
                  <strong>{volume.name}</strong>
                  <small>{volume.path}</small>
                </div>
                <span className="storageFree">
                  {volume.available
                    ? `${formatBytes(volume.available)} free`
                    : volume.filesystem}
                </span>
                <button
                  className="smallTextButton"
                  type="button"
                  onClick={() => onCreateJob(volume)}
                >
                  Add job
                </button>
              </div>
            ))}
          </div>
          {volumes.length === 0 ? (
            <p className="mutedText">
              No drives are mounted. Connect one or add a backup destination.
            </p>
          ) : null}
          <button
            className="smallTextButton"
            type="button"
            onClick={onSettings}
          >
            Manage storage <ChevronRight size={15} />
          </button>
        </div>
        <AndroidPanel devices={androidDevices} volumes={volumes} setMessage={setMessage} onBackupPhone={onBackupPhone} />
      </section>
    </>
  );
}



/** Start of the backend message when another app holds the phone. */
const PHONE_BUSY = "The phone is in use by another app";

function sendSummary(result: AndroidSendResult) {
  if (result.dryRun) {
    return result.sent === 0
      ? `Nothing new. ${result.skipped} file${result.skipped === 1 ? " is" : "s are"} already on the phone.`
      : `${result.sent} new file${result.sent === 1 ? "" : "s"} to send (${formatBytes(result.bytes)}), ${result.skipped} already on the phone.`;
  }
  const parts = [`${result.sent} sent (${formatBytes(result.bytes)})`, `${result.skipped} already on the phone`];
  if (result.failed) parts.push(`${result.failed} failed`);
  if (result.conflicts) parts.push(`${result.conflicts} skipped because of a name clash`);
  return `${result.cancelled ? "Stopped. " : ""}${parts.join(", ")}.`;
}

function AndroidPanel({
  devices,
  volumes,
  setMessage,
  onBackupPhone,
}: {
  devices: AndroidDevice[];
  volumes: MountedVolume[];
  setMessage: (message: string) => void;
  onBackupPhone: (device?: AndroidDevice) => void;
}) {
  const [deviceId, setDeviceId] = useState<string | undefined>(devices[0]?.id);
  const device = devices.find((item) => item.id === deviceId);
  const [phoneName, setPhoneName] = useState<string | null>(null);
  const [settings, setSettings] = useState<PhoneSettings>({});
  const [storages, setStorages] = useState<AndroidStorage[]>([]);
  const [albums, setAlbums] = useState<AndroidAlbum[]>([]);
  const [setup, setSetup] = useState(false);
  const [albumInput, setAlbumInput] = useState<"new" | "rename" | null>(null);
  const [sending, setSending] = useState(false);
  const [checking, setChecking] = useState(false);
  const [progress, setProgress] = useState<AndroidProgress | null>(null);
  const [result, setResult] = useState<AndroidSendResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const albumLabel = settings.album || "Fotografie";
  const writableStorages = storages.filter((storage) => storage.writable);

  useEffect(() => {
    if (!devices.some((item) => item.id === deviceId)) setDeviceId(devices[0]?.id);
  }, [devices, deviceId]);

  function report(reason: unknown, fallback: string) {
    const text = errorText(reason, fallback);
    setError(text);
    setMessage(text);
  }

  async function loadPhone(storageId = settings.storageId) {
    const api = getApi();
    if (!api || !deviceId) return;
    try {
      setStorages((await api.listAndroidStorages?.(deviceId)) ?? []);
      setAlbums((await api.listAndroidAlbums?.(deviceId, storageId)) ?? []);
      setError(null);
    } catch (reason) {
      report(reason, "Could not read the phone.");
    }
  }

  useEffect(() => {
    setPhoneName(null);
    setResult(null);
    setError(null);
    setAlbums([]);
    setStorages([]);
    if (!deviceId) return;
    let cancelled = false;
    // Every call opens the USB connection, so they run one after the other.
    void (async () => {
      const api = getApi();
      const saved = (await api?.loadAndroidSettings?.(deviceId).catch(() => null)) ?? {};
      if (cancelled) return;
      setSettings(saved);
      setSetup(!saved.source);
      const name = await api?.androidDeviceName?.(deviceId).catch(() => null);
      if (cancelled) return;
      setPhoneName(name ?? null);
      await loadPhone(saved.storageId);
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceId]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void getApi()
      ?.subscribeAndroidProgress?.(setProgress)
      .then((cleanup) => (disposed ? cleanup() : (unlisten = cleanup)));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  function remember(next: PhoneSettings) {
    setSettings(next);
    setResult(null);
    if (!deviceId) return;
    getApi()
      ?.saveAndroidSettings?.(deviceId, next)
      .catch((reason) => report(reason, "Could not save the phone settings."));
  }

  async function pick(startAt?: string) {
    const picked = await getApi()?.selectDirectory(startAt ?? settings.source ?? undefined);
    if (picked) remember({ ...settings, source: picked });
  }

  async function saveAlbum(value: string) {
    const api = getApi();
    const name = value.trim();
    const mode = albumInput;
    setAlbumInput(null);
    if (!api || !deviceId || !name || !mode) return;
    try {
      if (mode === "new") await api.createAndroidAlbum?.(deviceId, settings.storageId, name);
      else if (settings.album && name !== settings.album)
        await api.renameAndroidAlbum?.(deviceId, settings.storageId, settings.album, name);
      remember({ ...settings, album: name });
      await loadPhone();
    } catch (reason) {
      report(reason, "Album change failed.");
    }
  }

  async function send(dryRun = false) {
    const api = getApi();
    if (!api?.sendToAndroid || !deviceId || !settings.source) return;
    if (dryRun) setChecking(true);
    else setSending(true);
    setResult(null);
    setError(null);
    try {
      const sent = await api.sendToAndroid({
        deviceId,
        storageId: settings.storageId,
        source: settings.source,
        album: settings.album,
        dryRun,
      });
      setResult(sent);
      setMessage(sendSummary(sent));
      if (!dryRun) void loadPhone();
    } catch (reason) {
      report(reason, "Sending to the phone failed.");
    } finally {
      setSending(false);
      setChecking(false);
      setProgress(null);
    }
  }

  async function freePhone() {
    await getApi()?.freeAndroidDevice?.();
    setError(null);
    await loadPhone();
  }

  if (!device) {
    return (
      <div className="surface androidSurface">
        <div className="surfaceHeader">
          <div>
            <p className="eyebrow">Phone</p>
            <h3>No phone connected</h3>
          </div>
          <Cloud size={19} />
        </div>
        <p className="mutedText">Plug in your Android phone, unlock it and choose "File transfer" in the USB notification.</p>
      </div>
    );
  }

  const percent =
    progress && progress.totalBytes > 0
      ? Math.min(100, Math.round((progress.bytes / progress.totalBytes) * 100))
      : progress && progress.total > 0
        ? Math.round((progress.done / progress.total) * 100)
        : 0;
  const albumOptions =
    settings.album && !albums.some((item) => item.name.toLowerCase() === settings.album?.toLowerCase())
      ? [...albums, { name: settings.album, files: 0 }]
      : albums;

  return (
    <div className="surface androidSurface">
      <div className="surfaceHeader">
        <div>
          <p className="eyebrow">Send to phone</p>
          <h3>{phoneName ?? titleCase(device.manufacturer)}</h3>
        </div>
        <span className="driveDot connected" />
      </div>
      {devices.length > 1 ? (
        <select value={deviceId} disabled={sending} onChange={(event) => setDeviceId(event.target.value)}>
          {devices.map((item) => (
            <option key={item.id} value={item.id}>
              {titleCase(item.manufacturer)} {item.model}
            </option>
          ))}
        </select>
      ) : null}

      {setup ? (
        <>
          <p className="mutedText">1. Where are the photos and videos?</p>
          <div className="androidSources">
            {volumes.map((volume) => (
              <button key={volume.path} className="smallTextButton" type="button" onClick={() => void pick(volume.path)}>
                {volume.name}
              </button>
            ))}
            <button className="smallTextButton" type="button" onClick={() => void pick()}>
              This Mac
            </button>
          </div>
          {settings.source ? <small className="androidPath">{settings.source}</small> : null}
          <p className="mutedText">2. Which album on the phone? (DCIM/Fotografie)</p>
          {albumInput ? (
            <input
              autoFocus
              placeholder={albumInput === "new" ? "New album name" : "Album name"}
              defaultValue={albumInput === "rename" ? (settings.album ?? "") : ""}
              onBlur={(event) => void saveAlbum(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") event.currentTarget.blur();
                if (event.key === "Escape") setAlbumInput(null);
              }}
            />
          ) : (
            <div className="androidSources">
              <select
                value={settings.album ?? ""}
                onChange={(event) => remember({ ...settings, album: event.target.value || null })}
              >
                <option value="">Fotografie (no album)</option>
                {albumOptions.map((item) => (
                  <option key={item.name} value={item.name}>
                    {item.name} ({item.files})
                  </option>
                ))}
              </select>
              <button className="smallTextButton" type="button" onClick={() => setAlbumInput("new")}>
                New album
              </button>
              {settings.album ? (
                <button className="smallTextButton" type="button" onClick={() => setAlbumInput("rename")}>
                  Rename
                </button>
              ) : null}
            </div>
          )}
          {writableStorages.length > 1 ? (
            <>
              <p className="mutedText">3. Phone storage</p>
              <select
                value={settings.storageId ?? writableStorages[0].id}
                onChange={(event) => {
                  const storageId = Number(event.target.value);
                  remember({ ...settings, storageId });
                  void loadPhone(storageId);
                }}
              >
                {writableStorages.map((storage) => (
                  <option key={storage.id} value={storage.id}>
                    {storage.description || "Storage"} ({formatBytes(storage.free)} free)
                  </option>
                ))}
              </select>
            </>
          ) : null}
          <button
            className="primaryButton"
            type="button"
            disabled={!settings.source}
            onClick={() => setSetup(false)}
          >
            <Check size={16} /> Done
          </button>
        </>
      ) : (
        <>
          <button
            className="primaryButton androidSend"
            type="button"
            disabled={sending || checking}
            onClick={() => void send()}
          >
            {sending ? <Loader2 size={18} className="spin" /> : <ArrowUpRight size={18} />}
            {sending ? `Sending to ${albumLabel} ${percent}%` : `Send to ${albumLabel}`}
          </button>
          <small className="androidPath">From {settings.source}</small>
          {sending ? (
            <>
              <div className="transferTrack">
                <span style={{ width: `${percent}%` }} />
              </div>
              {progress?.file ? (
                <small className="androidPath">
                  {progress.done + 1} of {progress.total}: {progress.file}
                </small>
              ) : null}
              <button className="smallTextButton" type="button" onClick={() => void getApi()?.cancelAndroidSend?.()}>
                <X size={14} /> Stop
              </button>
            </>
          ) : (
            <div className="androidSources">
              <button className="smallTextButton" type="button" disabled={checking} onClick={() => void send(true)}>
                {checking ? "Checking" : "What's new?"}
              </button>
              <button className="smallTextButton" type="button" onClick={() => setSetup(true)}>
                Change
              </button>
            </div>
          )}
        </>
      )}

      {error ? (
        <div className="androidSources">
          <p className="mutedText">{error}</p>
          {error.startsWith(PHONE_BUSY) ? (
            <button className="smallTextButton" type="button" onClick={() => void freePhone()}>
              Free the phone
            </button>
          ) : null}
        </div>
      ) : null}
      {result ? <p className="mutedText">{sendSummary(result)}</p> : null}
      {result?.issues.length ? (
        <ul className="androidIssues">
          {result.issues.slice(0, 5).map((issue) => (
            <li key={issue.file}>
              <strong>{issue.file}</strong> {issue.message}
            </li>
          ))}
          {result.issues.length > 5 ? <li>and {result.issues.length - 5} more</li> : null}
        </ul>
      ) : null}
      {sending ? null : (
        <button className="smallTextButton" type="button" onClick={() => onBackupPhone(device)}>
          <Smartphone size={14} /> Back up this phone to a drive
        </button>
      )}
    </div>
  );
}

function JobCard({
  profile,
  selected,
  order,
  onSelect,
}: {
  profile: BackupProfile;
  selected: boolean;
  order: number;
  onSelect: () => void;
}) {
  const detail = detailFor(profile) ?? {
    role: "Archive copy",
    tone: "ink" as Tone,
    icon: Archive,
    summary: profile.description,
  };
  const Icon = detail.icon;
  return (
    <button
      className={`jobCard ${selected ? "selected" : ""}`}
      type="button"
      onClick={onSelect}
    >
      <div className={`jobIcon ${detail.tone}`}>
        <Icon size={20} />
      </div>
      <div className="jobNumber">0{order}</div>
      <div className="jobCopy">
        <span className="jobRole">{detail.role}</span>
        <strong>{profile.name}</strong>
        <small>{detail.summary}</small>
      </div>
      <div className="jobDestination">
        <span>Destination</span>
        <code>{shortPath(profile.destinationRoot) || "Not connected"}</code>
      </div>
      <ChevronRight className="jobArrow" size={17} />
    </button>
  );
}

function TransferProgress({ progress }: { progress: BackupProgress }) {
  const percent =
    progress.totalBytes > 0
      ? Math.min(
          100,
          Math.round((progress.copiedBytes / progress.totalBytes) * 100),
        )
      : progress.totalFiles > 0
        ? Math.round((progress.completedFiles / progress.totalFiles) * 100)
        : 0;
  return (
    <section className="transferPanel">
      <div className="transferTop">
        <div>
          <p className="transferKicker">
            {progress.event === "scanning"
              ? "Scanning source folders"
              : "Copying files"}
          </p>
          <strong>
            {progress.path ? progress.path : "Preparing transfer"}
          </strong>
        </div>
        <span className="transferPercent">{percent}%</span>
      </div>
      <div className="transferGraph">
        <span className="graphLine graphOne" />
        <span className="graphLine graphTwo" />
        <span className="graphLine graphThree" />
      </div>
      <div className="transferStats">
        <div>
          <span>Discovered</span>
          <strong>
            {progress.discoveredFiles.toLocaleString()} <small>files</small>
          </strong>
        </div>
        <div>
          <span>Evaluated</span>
          <strong>
            {progress.evaluatedFiles.toLocaleString()} <small>files</small>
          </strong>
        </div>
        <div>
          <span>Copied</span>
          <strong>
            {progress.completedFiles.toLocaleString()}{" "}
            <small>({formatBytes(progress.copiedBytes)})</small>
          </strong>
        </div>
        <div>
          <span>Elapsed</span>
          <strong>{formatDuration(progress.elapsedSeconds)}</strong>
        </div>
        <div>
          <span>Throughput</span>
          <strong>
            {formatBytes(progress.bytesPerSecond)}
            <small>/s</small>
          </strong>
        </div>
      </div>
      <div className="transferTrack">
        <span style={{ width: `${percent}%` }} />
      </div>
      <div className="transferFooter">
        <span>{progress.totalFiles.toLocaleString()} files in this run</span>
        <span>{formatBytes(progress.totalBytes)} planned</span>
      </div>
    </section>
  );
}

function PlanSummary({
  plan,
  runResult,
}: {
  plan: BackupPlan;
  runResult: BackupRunResult | null;
}) {
  return (
    <div className="planSummary">
      <div className="planNumbers">
        <div>
          <strong>{formatBytes(plan.totals.bytesToWrite)}</strong>
          <span>to write</span>
        </div>
        <div>
          <strong>{plan.totals.filesSkipped}</strong>
          <span>unchanged files</span>
        </div>
        <div>
          <strong>{runResult?.failedFiles ?? 0}</strong>
          <span>failures</span>
        </div>
      </div>
      <div className="progressTrack">
        <span
          style={{
            width: `${plan.totals.scannedFiles ? Math.max(7, Math.round((plan.totals.filesSkipped / plan.totals.scannedFiles) * 100)) : 7}%`,
          }}
        />
      </div>
      <p>
        {plan.warnings[0]?.message ?? "The planner found no safety warnings."}
      </p>
    </div>
  );
}

function EmptyActivity({ onPlan }: { onPlan: () => void }) {
  return (
    <div className="emptyActivity">
      <div className="emptyIcon">
        <Sparkles size={19} />
      </div>
      <p>
        Preview the changes before copying anything. Your originals stay
        untouched.
      </p>
      <button className="smallTextButton" type="button" onClick={onPlan}>
        Preview first run <ArrowUpRight size={15} />
      </button>
    </div>
  );
}

function SettingsPage({
  profiles,
  selectedProfile,
  selectedId,
  setSelectedId,
  volumes,
  busy,
  health,
  qnapCommands,
  qnapFolders,
  onChange,
  onRemove,
  onChooseDestination,
  onAddDrive,
  onAddQnap,
  onAddPhone,
  androidDevices,
  onGenerateQnap,
  onLoadQnapFolders,
  onHealthCheck,
  onRefresh,
  onBack,
}: {
  profiles: BackupProfile[];
  selectedProfile?: BackupProfile;
  selectedId: string;
  setSelectedId: (id: string) => void;
  volumes: MountedVolume[];
  busy: string | null;
  health: VolumeHealth | null;
  qnapCommands: QnapCommandSet | null;
  qnapFolders: string[];
  onChange: (profile: BackupProfile) => void;
  onRemove: (profile: BackupProfile) => void;
  onChooseDestination: () => void;
  onAddDrive: () => void;
  onAddQnap: () => void;
  onAddPhone: () => void;
  androidDevices: AndroidDevice[];
  onGenerateQnap: () => void;
  onLoadQnapFolders: () => void;
  onHealthCheck: (path: string, level: HealthCheckLevel) => void;
  onRefresh: () => void;
  onBack: () => void;
}) {
  const healthPath = volumes[0]?.path;
  return (
    <>
      <header className="pageHeader compactHeader">
        <div>
          <p className="eyebrow">Configuration</p>
          <h2>Settings</h2>
          <p className="lede">
            Set these once. Daily backup decisions stay on the Overview.
          </p>
        </div>
        <button className="secondaryButton" type="button" onClick={onBack}>
          <ChevronRight className="rotateBack" size={17} /> Back to overview
        </button>
      </header>
      <div className="settingsLayout">
        <aside className="settingsJobs">
          <p className="eyebrow">Jobs</p>
          {profiles
            .filter((profile) => profile.id !== "demo")
            .map((profile) => (
              <button
                className={
                  selectedId === profile.id
                    ? "settingsJob active"
                    : "settingsJob"
                }
                type="button"
                key={profile.id}
                onClick={() => setSelectedId(profile.id)}
              >
                <span>{detailFor(profile)?.role ?? "Backup"}</span>
                <strong>{profile.name}</strong>
                <ChevronRight size={15} />
              </button>
            ))}
          <div className="settingsActions">
            <p className="eyebrow">New job</p>
            <button className="ghostButton" type="button" onClick={onAddDrive}>
              <FolderOpen size={14} /> Copy of this job to another drive
            </button>
            <button className="ghostButton" type="button" onClick={onAddPhone}>
              <Smartphone size={14} /> Phone backup
            </button>
            <button className="ghostButton" type="button" onClick={onAddQnap}>
              <Wifi size={14} /> QNAP over SSH
            </button>
          </div>

        </aside>
        <div className="settingsContent">
          {selectedProfile && selectedProfile.id !== "demo" ? (
            <JobEditor
              key={selectedProfile.id}
              profile={selectedProfile}
              volumes={volumes}
              androidDevices={androidDevices}
              qnapCommands={qnapCommands}
              qnapFolders={qnapFolders}
              onChange={onChange}
              onRemove={() => onRemove(selectedProfile)}
              onGenerateQnap={onGenerateQnap}
              onLoadQnapFolders={onLoadQnapFolders}
            />
          ) : null}
          <section className="surface advancedSettings">
            <div className="surfaceHeader">
              <div>
                <p className="eyebrow">Storage health</p>
                <h3>Connected locations</h3>
              </div>
              <button
                className="iconButton"
                title="Refresh connected storage"
                type="button"
                onClick={onRefresh}
                disabled={busy !== null}
              >
                <RefreshCw size={17} />
              </button>
            </div>
            {volumes.length > 0 ? (
              volumes.map((volume) => (
                <div className="healthVolume" key={volume.path}>
                  <div className="storageRow">
                    <span className="driveDot connected" />
                    <div>
                      <strong>{volume.name}</strong>
                      <small>{volume.path}</small>
                    </div>
                    <span className="storageFree">
                      {volume.available
                        ? `${formatBytes(volume.available)} free`
                        : volume.filesystem}
                    </span>
                  </div>
                  <div className="healthActions">
                    <button
                      className="smallTextButton"
                      type="button"
                      onClick={() => onHealthCheck(volume.path, "quick")}
                      disabled={busy !== null}
                    >
                      Quick
                    </button>
                    <button
                      className="smallTextButton"
                      type="button"
                      onClick={() => onHealthCheck(volume.path, "medium")}
                      disabled={busy !== null}
                    >
                      Medium
                    </button>
                    <button
                      className="smallTextButton"
                      type="button"
                      onClick={() => onHealthCheck(volume.path, "full")}
                      disabled={busy !== null}
                    >
                      Full read
                    </button>
                  </div>
                </div>
              ))
            ) : (
              <p className="mutedText">
                No drives are mounted. Connect a drive before running a health
                check.
              </p>
            )}
            {health ? (
              <div className={`healthResult ${health.status}`}>
                <strong>
                  {health.status === "healthy"
                    ? "Health check passed"
                    : "Health check needs attention"}
                </strong>
                <span>
                  {health.filesChecked} files checked,{" "}
                  {formatBytes(health.bytesRead)} read in {health.durationMs}{" "}
                  ms.
                </span>
                <small>{health.note}</small>
                {health.errors.map((error) => (
                  <code key={error}>{error}</code>
                ))}
              </div>
            ) : null}
          </section>
        </div>
      </div>
    </>
  );
}

function StorageMini({
  name,
  detail,
  tone,
  connected,
}: {
  name: string;
  detail: string;
  tone: Tone;
  connected: boolean;
}) {
  return (
    <div className="storageMini">
      <span className={`miniDot ${tone} ${connected ? "isConnected" : ""}`} />
      <div>
        <strong>{name}</strong>
        <small>{detail}</small>
      </div>
      <span className="miniState">{connected ? "Ready" : "Offline"}</span>
    </div>
  );
}
function isVolumeConnected(volumes: MountedVolume[], names: string[]) {
  return volumes.some((volume) =>
    names.some(
      (name) =>
        volume.name.toLowerCase() === name.toLowerCase() ||
        volume.path.toLowerCase().includes(name.toLowerCase()),
    ),
  );
}
function shortPath(value: string) {
  return value.replace(/^\/Volumes\//, "");
}
function formatDate(value: string) {
  return new Date(value).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}
function formatDuration(seconds: number) {
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = seconds % 60;
  return `${minutes}m ${remainingSeconds.toString().padStart(2, "0")}s`;
}
