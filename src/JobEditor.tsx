import {
  Check,
  ChevronDown,
  Clock,
  Cloud,
  FolderOpen,
  HardDrive,
  History,
  Plus,
  RefreshCw,
  Smartphone,
  Trash2,
  Wifi,
  X,
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import type {
  AndroidDevice,
  BackupFilter,
  BackupProfile,
  BackupSnapshot,
  DestinationInfo,
  MountedVolume,
  ProfileSource,
  QnapCommandSet,
} from "../shared/types";
import { errorText, formatBytes, getApi, titleCase } from "./helpers";

const phoneFolderSuggestions = [
  { id: "camera", label: "Camera", path: "DCIM/Camera" },
  { id: "screenshots", label: "Screenshots", path: "Pictures/Screenshots" },
  { id: "whatsapp", label: "WhatsApp media", path: "Android/media/com.whatsapp/WhatsApp/Media" },
  { id: "downloads", label: "Downloads", path: "Download" },
];

type Option<T extends string | number> = { value: T; label: string; disabled?: boolean; hint?: string };

function Segmented<T extends string | number>({ value, options, onChange, label }: { value: T; options: Option<T>[]; onChange: (value: T) => void; label: string }) {
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={String(option.value)}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          className={option.value === value ? "active" : ""}
          disabled={option.disabled}
          title={option.hint}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

function Toggle({ checked, onChange, label, detail }: { checked: boolean; onChange: (checked: boolean) => void; label: string; detail?: string }) {
  return (
    <label className="toggleRow">
      <span>
        <strong>{label}</strong>
        {detail ? <small>{detail}</small> : null}
      </span>
      <input type="checkbox" role="switch" checked={checked} onChange={(event) => onChange(event.target.checked)} />
    </label>
  );
}

function Card({ title, icon, aside, children }: { title: string; icon: ReactNode; aside?: ReactNode; children: ReactNode }) {
  return (
    <section className="editorCard">
      <header>
        <span className="editorIcon">{icon}</span>
        <h3>{title}</h3>
        {aside}
      </header>
      {children}
    </section>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="editorField">
      <span className="editorLabel">{label}</span>
      {children}
      {hint ? <small className="editorHint">{hint}</small> : null}
    </div>
  );
}

function volumeFor(path: string, volumes: MountedVolume[]) {
  return volumes.find((volume) => path === volume.path || path.startsWith(`${volume.path}/`));
}

/** A folder shown as drive + path, with a picker. */
function PathRow({ path, volumes, placeholder, onPick, onRemove }: { path: string; volumes: MountedVolume[]; placeholder: string; onPick: (path: string) => void; onRemove?: () => void }) {
  const volume = volumeFor(path, volumes);
  const inside = volume ? path.slice(volume.path.length).replace(/^\//, "") : path;
  const offline = path.startsWith("/Volumes/") && !volume;

  async function pick(startAt?: string) {
    const picked = await getApi()?.selectDirectory(startAt ?? (path || undefined));
    if (picked) onPick(picked);
  }

  return (
    <div className="pathRow">
      <span className={`pathIcon ${offline ? "offline" : ""}`}>{volume || offline ? <HardDrive size={16} /> : <FolderOpen size={16} />}</span>
      {path ? (
        <span className="pathText">
          <strong>{volume?.name ?? (offline ? path.split("/")[2] : "This Mac")}</strong>
          <small>{offline ? "Drive not connected" : inside ? inside : "Whole drive"}</small>
        </span>
      ) : (
        <span className="pathText empty">{placeholder}</span>
      )}
      <div className="pathActions">
        <button className="ghostButton" type="button" onClick={() => void pick()}>
          {path ? "Change" : "Choose"}
        </button>
        {onRemove ? (
          <button className="iconGhost" type="button" title="Remove" onClick={onRemove}>
            <X size={15} />
          </button>
        ) : null}
      </div>
      {!path && volumes.length > 0 ? (
        <div className="driveChips">
          {volumes.map((volume) => (
            <button key={volume.path} type="button" className="chip" onClick={() => void pick(volume.path)}>
              <HardDrive size={13} /> {volume.name}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function PhoneSelect({ value, devices, onChange }: { value: string; devices: AndroidDevice[]; onChange: (deviceId: string) => void }) {
  const known = value === "" || devices.some((device) => device.id === value);
  return (
    <select className="editorSelect" value={value} onChange={(event) => onChange(event.target.value)}>
      <option value="">Whichever phone is connected</option>
      {devices.map((device) => (
        <option key={device.id} value={device.id}>
          {titleCase(device.manufacturer)} {device.model}
        </option>
      ))}
      {known ? null : <option value={value}>Saved phone (not connected)</option>}
    </select>
  );
}

function SourceList({ profile, volumes, androidDevices, onChange }: { profile: BackupProfile; volumes: MountedVolume[]; androidDevices: AndroidDevice[]; onChange: (profile: BackupProfile) => void }) {
  const sources = profile.sources;
  const update = (index: number, next: Partial<ProfileSource>) =>
    onChange({ ...profile, sources: sources.map((source, position) => (position === index ? { ...source, ...next } : source)) });
  const remove = (index: number) => onChange({ ...profile, sources: sources.filter((_, position) => position !== index) });
  const uniqueId = (base: string) => {
    let id = base;
    for (let counter = 2; sources.some((source) => source.id === id); counter += 1) id = `${base}-${counter}`;
    return id;
  };
  const phoneDevice = sources.find((source) => source.kind === "android")?.deviceId ?? "";
  const unusedPhoneFolders = phoneFolderSuggestions.filter((item) => !sources.some((source) => source.kind === "android" && source.path === item.path));

  return (
    <div className="sourceList">
      {sources.map((source, index) =>
        source.kind === "android" ? (
          <div className="pathRow phoneRow" key={source.id}>
            <span className="pathIcon">
              <Smartphone size={16} />
            </span>
            <div className="phoneFields">
              <input className="editorInput" value={source.path} placeholder="DCIM/Camera" onChange={(event) => update(index, { path: event.target.value })} />
              <PhoneSelect value={source.deviceId ?? ""} devices={androidDevices} onChange={(deviceId) => update(index, { deviceId })} />
            </div>
            <div className="pathActions">
              {sources.length > 1 ? (
                <button className="iconGhost" type="button" title="Remove" onClick={() => remove(index)}>
                  <X size={15} />
                </button>
              ) : null}
            </div>
          </div>
        ) : (
          <PathRow
            key={source.id}
            path={source.path}
            volumes={volumes}
            placeholder="Choose a folder or a whole drive"
            onPick={(path) => update(index, { path, label: path.split("/").filter(Boolean).at(-1) ?? "Folder" })}
            onRemove={sources.length > 1 ? () => remove(index) : undefined}
          />
        ),
      )}
      <div className="addRow">
        <button
          className="ghostButton"
          type="button"
          onClick={() => onChange({ ...profile, sources: [...sources, { id: uniqueId("source"), label: "Folder", path: "", category: "other", kind: "folder" }] })}
        >
          <Plus size={14} /> Folder
        </button>
        {unusedPhoneFolders.length > 0 ? (
          <label className="ghostSelect">
            <Smartphone size={14} /> Phone folder
            <ChevronDown size={13} />
            <select
              value=""
              aria-label="Add a phone folder"
              onChange={(event) => {
                const item = phoneFolderSuggestions.find((suggestion) => suggestion.path === event.target.value);
                if (!item) return;
                onChange({
                  ...profile,
                  sources: [...sources, { id: uniqueId(item.id), label: item.label, path: item.path, category: "photos", kind: "android", deviceId: phoneDevice }],
                });
              }}
            >
              <option value="" disabled>
                Add a phone folder
              </option>
              {unusedPhoneFolders.map((item) => (
                <option key={item.path} value={item.path}>
                  {item.label}
                </option>
              ))}
            </select>
          </label>
        ) : null}
      </div>
    </div>
  );
}

function FilterList({ filters, onChange }: { filters: BackupFilter[]; onChange: (filters: BackupFilter[]) => void }) {
  const [pattern, setPattern] = useState("");
  const add = () => {
    const value = pattern.trim();
    if (!value || filters.some((filter) => filter.pattern === value)) return;
    onChange([...filters, { id: `exclude-${Date.now()}`, pattern: value, action: "exclude", note: "" }]);
    setPattern("");
  };
  return (
    <div className="filterList">
      <div className="chipRow">
        {filters.length === 0 ? <small className="editorHint">Nothing is skipped.</small> : null}
        {filters.map((filter) => (
          <span key={filter.id} className={`filterChip ${filter.action}`} title={filter.note || undefined}>
            {filter.action === "include" ? "Only " : ""}
            <code>{filter.pattern}</code>
            <button type="button" title="Remove" onClick={() => onChange(filters.filter((item) => item.id !== filter.id))}>
              <X size={12} />
            </button>
          </span>
        ))}
      </div>
      <div className="inlineAdd">
        <input
          className="editorInput"
          value={pattern}
          placeholder="Skip files or folders, e.g. *.tmp or **/Cache/**"
          onChange={(event) => setPattern(event.target.value)}
          onKeyDown={(event) => event.key === "Enter" && add()}
        />
        <button className="ghostButton" type="button" disabled={!pattern.trim()} onClick={add}>
          <Plus size={14} /> Skip
        </button>
      </div>
    </div>
  );
}

function useDestination(profile: BackupProfile) {
  const [info, setInfo] = useState<DestinationInfo | null>(null);
  const [snapshots, setSnapshots] = useState<BackupSnapshot[]>([]);
  const local = profile.destinationKind === "local" || profile.destinationKind === "qnap-smb";
  const root = profile.destinationRoot;
  useEffect(() => {
    setInfo(null);
    setSnapshots([]);
    const api = getApi();
    if (!api || !local || !root) return;
    let cancelled = false;
    void api.inspectDestination?.(root).then((next) => !cancelled && setInfo(next)).catch(() => undefined);
    void api.listSnapshots?.(root).then((next) => !cancelled && setSnapshots(next)).catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [root, local]);
  return { info, snapshots, local };
}

export function JobEditor({
  profile,
  volumes,
  androidDevices,
  qnapCommands,
  qnapFolders,
  onChange,
  onRemove,
  onGenerateQnap,
  onLoadQnapFolders,
}: {
  profile: BackupProfile;
  volumes: MountedVolume[];
  androidDevices: AndroidDevice[];
  qnapCommands: QnapCommandSet | null;
  qnapFolders: string[];
  onChange: (profile: BackupProfile) => void;
  onRemove: () => void;
  onGenerateQnap: () => void;
  onLoadQnapFolders: () => void;
}) {
  const { info, snapshots, local } = useDestination(profile);
  const [volumeName, setVolumeName] = useState<string | null>(null);
  const [volumeStatus, setVolumeStatus] = useState<string | null>(null);
  const [creatingVolume, setCreatingVolume] = useState(false);
  useEffect(() => {
    setVolumeName(null);
    setVolumeStatus(null);
  }, [profile.id]);

  const singleSource = profile.sources.length === 1;
  const destinationVolume = volumeFor(profile.destinationRoot, volumes);
  const snapshotsPossible = local && (info ? info.snapshotsSupported : true);
  const versions = profile.mode === "append-only" ? "off" : profile.versioning === "snapshots" && snapshotsPossible ? "snapshots" : "archive";
  const autoRun = profile.autoRun ?? {};
  const lightroomGuard = profile.guards?.includes("lightroom-closed") ?? false;
  const configured = Boolean(profile.destinationRoot) && profile.sources.some((source) => source.path);

  async function createVolume(name: string) {
    const api = getApi();
    if (!api?.createJobVolume) return;
    setCreatingVolume(true);
    setVolumeStatus(null);
    try {
      const mount = await api.createJobVolume(profile.destinationRoot, name);
      onChange({ ...profile, destinationRoot: mount, layout: singleSource ? "mirror" : profile.layout });
      setVolumeName(null);
      setVolumeStatus(`Volume ${name} is ready. It shares the free space of the drive.`);
    } catch (error) {
      setVolumeStatus(errorText(error, "Could not make the volume."));
    } finally {
      setCreatingVolume(false);
    }
  }

  return (
    <div className="jobEditor">
      <div className="editorTitle">
        <input
          className="titleInput"
          value={profile.name}
          aria-label="Job name"
          onChange={(event) => onChange({ ...profile, name: event.target.value })}
        />
        <span className={`statusChip ${configured ? "good" : "warn"}`}>{configured ? "Ready" : "Needs setup"}</span>
      </div>

      <Card title="Back up" icon={<FolderOpen size={16} />}>
        <SourceList profile={profile} volumes={volumes} androidDevices={androidDevices} onChange={onChange} />
      </Card>

      <Card title="To" icon={<HardDrive size={16} />}>
        <Segmented
          label="Destination type"
          value={profile.destinationKind === "qnap-smb" ? "local" : profile.destinationKind}
          options={[
            { value: "local", label: "Drive or folder" },
            { value: "android", label: "Phone" },
            ...(profile.destinationKind === "qnap-rsync" ? [{ value: "qnap-rsync" as const, label: "QNAP" }] : []),
          ]}
          onChange={(destinationKind) =>
            onChange({
              ...profile,
              destinationKind,
              destinationRoot: destinationKind === "android" ? "Backup" : profile.destinationKind === "android" ? "" : profile.destinationRoot,
              phone: destinationKind === "android" ? (profile.phone ?? { deviceId: "" }) : profile.phone,
            })
          }
        />
        {profile.destinationKind === "android" ? (
          <div className="pathRow phoneRow">
            <span className="pathIcon">
              <Smartphone size={16} />
            </span>
            <div className="phoneFields">
              <input className="editorInput" value={profile.destinationRoot} placeholder="Backup" onChange={(event) => onChange({ ...profile, destinationRoot: event.target.value })} />
              <PhoneSelect value={profile.phone?.deviceId ?? ""} devices={androidDevices} onChange={(deviceId) => onChange({ ...profile, phone: { ...profile.phone, deviceId } })} />
            </div>
          </div>
        ) : profile.destinationKind === "qnap-rsync" ? null : (
          <PathRow
            path={profile.destinationRoot}
            volumes={volumes}
            placeholder="Choose where the backup goes"
            onPick={(path) => onChange({ ...profile, destinationRoot: path })}
          />
        )}
        {local && info ? (
          <div className="badgeRow">
            {info.filesystem ? <span className="badge">{info.filesystem.toUpperCase()}</span> : null}
            {destinationVolume?.available ? <span className="badge">{formatBytes(destinationVolume.available)} free</span> : null}
            {info.isVolumeRoot ? <span className="badge">Whole volume</span> : null}
            {info.cccBackup ? <span className="badge accent">Carbon Copy Cloner backup</span> : null}
          </div>
        ) : null}
        {info?.cccBackup && profile.layout !== "mirror" ? (
          <div className="banner">
            <p>This volume holds a Carbon Copy Cloner backup. Continue it as an exact mirror: files already there are not copied again.</p>
            <button className="primaryButton" type="button" disabled={!singleSource} onClick={() => onChange({ ...profile, layout: "mirror" })}>
              <Check size={15} /> Continue CCC backup
            </button>
          </div>
        ) : null}
        {profile.destinationKind !== "android" ? (
          <Field
            label="Layout"
            hint={profile.layout === "mirror" ? "The destination becomes an exact copy of the source." : "Each source gets its own folder in the destination."}
          >
            <Segmented
              label="Layout"
              value={profile.layout ?? "per-source"}
              options={[
                { value: "per-source", label: "Folder per source" },
                { value: "mirror", label: "Exact mirror", disabled: !singleSource, hint: singleSource ? undefined : "A mirror copies exactly one source" },
              ]}
              onChange={(layout) => onChange({ ...profile, layout })}
            />
          </Field>
        ) : null}
        {info?.canCreateVolume && !info.isVolumeRoot ? (
          volumeName === null ? (
            <button className="linkButton" type="button" onClick={() => setVolumeName(profile.name.replace(/[/:]/g, " ").slice(0, 60))}>
              <Plus size={14} /> Give this job its own volume on {info.volumeName ?? "this drive"}
            </button>
          ) : (
            <div className="inlineAdd">
              <input className="editorInput" value={volumeName} aria-label="Volume name" onChange={(event) => setVolumeName(event.target.value)} />
              <button className="primaryButton" type="button" disabled={creatingVolume || !volumeName.trim()} onClick={() => void createVolume(volumeName)}>
                {creatingVolume ? "Making" : "Create volume"}
              </button>
              <button className="iconGhost" type="button" title="Cancel" onClick={() => setVolumeName(null)}>
                <X size={15} />
              </button>
            </div>
          )
        ) : null}
        {volumeStatus ? <small className="editorHint">{volumeStatus}</small> : null}
      </Card>

      <Card title="Older versions" icon={<History size={16} />}>
        <Segmented
          label="Older versions"
          value={versions}
          options={[
            { value: "snapshots", label: "Snapshots", disabled: !snapshotsPossible, hint: snapshotsPossible ? undefined : "Needs an APFS drive" },
            { value: "archive", label: "Archive folder" },
            { value: "off", label: "Don't keep" },
          ]}
          onChange={(next) =>
            onChange({
              ...profile,
              mode: next === "off" ? "append-only" : "safe-mirror",
              versioning: next === "snapshots" ? "snapshots" : "archive",
            })
          }
        />
        <small className="editorHint">
          {versions === "snapshots"
            ? "After every run the backup is cloned to _Snapshots. Only changes take extra space."
            : versions === "archive"
              ? "Files that change are moved to _Archive before the new version is written."
              : "Changed files are overwritten. Deleted source files still stay in the backup."}
        </small>
        {versions !== "off" ? (
          <Field label="Keep for">
            <Segmented
              label="Keep older versions for"
              value={profile.archiveKeepDays ?? 0}
              options={[
                { value: 30, label: "30 days" },
                { value: 90, label: "90 days" },
                { value: 365, label: "1 year" },
                { value: 0, label: "Forever" },
              ]}
              onChange={(days) => onChange({ ...profile, archiveKeepDays: days > 0 ? days : undefined })}
            />
          </Field>
        ) : null}
        {snapshots.length > 0 ? (
          <ul className="snapshotRows">
            {snapshots.slice(0, 5).map((snapshot) => (
              <li key={snapshot.name}>
                <span>{new Date(snapshot.createdAt * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</span>
                <button className="ghostButton" type="button" onClick={() => void getApi()?.revealPath?.(snapshot.path)}>
                  Show in Finder
                </button>
              </li>
            ))}
            {snapshots.length > 5 ? <li className="editorHint">and {snapshots.length - 5} older</li> : null}
          </ul>
        ) : null}
      </Card>

      <Card title="When" icon={<Clock size={16} />}>
        <Toggle
          label="Run when connected"
          detail="Starts as soon as the drive (or phone) is plugged in."
          checked={Boolean(autoRun.whenConnected)}
          onChange={(whenConnected) => onChange({ ...profile, autoRun: { ...autoRun, whenConnected } })}
        />
        <Field label="Repeat while connected">
          <Segmented
            label="Repeat"
            value={autoRun.everyHours ?? 0}
            options={[
              { value: 0, label: "Off" },
              { value: 1, label: "Hourly" },
              { value: 6, label: "Every 6 h" },
              { value: 24, label: "Daily" },
              { value: 168, label: "Weekly" },
            ]}
            onChange={(hours) => onChange({ ...profile, autoRun: { ...autoRun, everyHours: hours > 0 ? hours : null } })}
          />
        </Field>
      </Card>

      {profile.destinationKind === "qnap-rsync" ? (
        <QnapCard profile={profile} commands={qnapCommands} folders={qnapFolders} onChange={onChange} onGenerate={onGenerateQnap} onLoadFolders={onLoadQnapFolders} />
      ) : null}

      <details className="editorCard advanced">
        <summary>
          <span className="editorIcon">
            <ChevronDown size={16} />
          </span>
          <h3>Advanced</h3>
        </summary>
        <Field label="Skip">
          <FilterList filters={profile.filters ?? []} onChange={(filters) => onChange({ ...profile, filters })} />
        </Field>
        <Field label="Checking files" hint="Hashing reads the whole file, so it is slower but catches silent changes.">
          <Segmented
            label="Change detection"
            value={profile.verification}
            options={[
              { value: "metadata", label: "Size and date" },
              { value: "important-hash", label: "Hash catalogs" },
              { value: "full-hash", label: "Hash everything" },
            ]}
            onChange={(verification) => onChange({ ...profile, verification })}
          />
        </Field>
        <Field label="Files at the same time" hint="1 or 2 for hard disks and video, 4 to 8 for SSDs.">
          <Segmented
            label="Parallel copies"
            value={profile.parallelCopies ?? 4}
            options={[1, 2, 4, 8].map((value) => ({ value, label: String(value) }))}
            onChange={(parallelCopies) => onChange({ ...profile, parallelCopies })}
          />
        </Field>
        <Toggle
          label="Skip live Lightroom catalogs"
          detail="Catalog files are left out while Lightroom Classic is open."
          checked={lightroomGuard}
          onChange={(on) =>
            onChange({
              ...profile,
              guards: on ? [...(profile.guards ?? []), "lightroom-closed"] : (profile.guards ?? []).filter((guard) => guard !== "lightroom-closed"),
            })
          }
        />
        <Toggle
          label="Check every file on the destination"
          detail="Slower. Notices files removed from the backup by hand."
          checked={Boolean(profile.checkDestination)}
          onChange={(checkDestination) => onChange({ ...profile, checkDestination })}
        />
        <Toggle
          label="Keep Finder tags and labels"
          checked={profile.preserveMacMetadata}
          onChange={(preserveMacMetadata) => onChange({ ...profile, preserveMacMetadata })}
        />
      </details>

      <div className="editorFooter">
        <button className="dangerLink" type="button" onClick={onRemove}>
          <Trash2 size={14} /> Remove this job
        </button>
        <small>Backed up files stay on the drive.</small>
      </div>
    </div>
  );
}

function QnapCard({
  profile,
  commands,
  folders,
  onChange,
  onGenerate,
  onLoadFolders,
}: {
  profile: BackupProfile;
  commands: QnapCommandSet | null;
  folders: string[];
  onChange: (profile: BackupProfile) => void;
  onGenerate: () => void;
  onLoadFolders: () => void;
}) {
  const qnap = profile.qnap ?? { host: "", share: "Backups", username: "", remoteRoot: "/share/Backups", sshPort: 22, useHelper: true };
  const set = (next: Partial<typeof qnap>) => onChange({ ...profile, qnap: { ...qnap, ...next } });
  return (
    <Card title="QNAP connection" icon={<Cloud size={16} />}>
      <div className="editorGrid">
        <Field label="Host">
          <input className="editorInput" value={qnap.host} placeholder="qnap.local" onChange={(event) => set({ host: event.target.value })} />
        </Field>
        <Field label="Username">
          <input className="editorInput" value={qnap.username} onChange={(event) => set({ username: event.target.value })} />
        </Field>
        <Field label="Share">
          <input className="editorInput" value={qnap.share} onChange={(event) => set({ share: event.target.value })} />
        </Field>
        <Field label="SSH port">
          <input className="editorInput" type="number" value={qnap.sshPort} onChange={(event) => set({ sshPort: Number(event.target.value) || 22 })} />
        </Field>
      </div>
      <Field label="Folder on the NAS">
        {folders.length > 0 ? (
          <select className="editorSelect" value={profile.destinationRoot} onChange={(event) => onChange({ ...profile, destinationRoot: event.target.value })}>
            <option value="">Choose a folder</option>
            {folders.map((folder) => (
              <option key={folder} value={folder}>
                {folder}
              </option>
            ))}
          </select>
        ) : (
          <input className="editorInput" value={profile.destinationRoot} placeholder="/share/Backups" onChange={(event) => onChange({ ...profile, destinationRoot: event.target.value })} />
        )}
      </Field>
      <div className="addRow">
        <button className="ghostButton" type="button" onClick={onLoadFolders}>
          <RefreshCw size={14} /> Load folders
        </button>
        <button className="ghostButton" type="button" onClick={onGenerate}>
          <Wifi size={14} /> Connection commands
        </button>
      </div>
      {commands ? (
        <div className="commandPreview">
          {[commands.macMountCommand, ...commands.rsyncCommands].filter(Boolean).map((command) => (
            <code key={command}>{command}</code>
          ))}
        </div>
      ) : null}
    </Card>
  );
}
