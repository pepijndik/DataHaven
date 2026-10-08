/// <reference types="vite/client" />

import type { BackupApi } from "../shared/types";

declare global {
  interface Window {
    backupApp?: BackupApi;
  }
}
