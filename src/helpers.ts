import { tauriApi } from "./tauriApi";

export function getApi() {
  if (window.backupApp) {
    return window.backupApp;
  }

  const runtimeWindow = window as Window & { __TAURI_INTERNALS__?: unknown };
  return runtimeWindow.__TAURI_INTERNALS__ ? tauriApi : undefined;
}

/** Tauri commands reject with plain strings, other code with Error objects. */
export function errorText(error: unknown, fallback: string) {
  return typeof error === "string" ? error : error instanceof Error ? error.message : fallback;
}

export function titleCase(value: string) {
  return value.toLowerCase().replace(/\b\w/g, (letter) => letter.toUpperCase());
}

export function formatBytes(bytes: number) {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  const value = bytes / 1024 ** index;
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[index]}`;
}
