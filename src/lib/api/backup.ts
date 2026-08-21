import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** Mirrors BackupStatus in src-tauri/src/commands/backup.rs (camelCase). */
export interface BackupStatus {
  everRan: boolean;
  lastRunAt: string | null;
  snapshotId: string | null;
  drillPassed: boolean;
  stale: boolean;
  failure: string | null;
  pushedTo: string | null;
  wrappingKeyPresent: boolean;
  scheduleInstalled: boolean;
  toolCopyOk: boolean;
  scheduleSupported: boolean;
}

export interface EscrowArtifacts {
  sheetPath: string;
  usbPath: string;
}

export function getBackupStatus(): Promise<BackupStatus> {
  return invoke('backup_status');
}

export function escrowInit(outDir: string): Promise<EscrowArtifacts> {
  return invoke('backup_escrow_init', { outDir });
}

export function escrowVerify(file: string): Promise<string> {
  return invoke('backup_escrow_verify', { file });
}

export function installBackupSchedule(
  hour: number,
  minute: number,
  url: string | null,
  token: string | null,
): Promise<string> {
  return invoke('backup_install_schedule', { hour, minute, url, token });
}

export function uninstallBackupSchedule(): Promise<string> {
  return invoke('backup_uninstall_schedule');
}

/** Runs the full backup job; resolves true/false. Progress arrives via
 *  `backup-job` events (subscribe with onBackupJobEvent). */
export function runBackupNow(): Promise<boolean> {
  return invoke('backup_run_now');
}

export interface BackupJobEvent {
  kind: 'step' | 'ok' | 'fail';
  line: string;
}

export async function onBackupJobEvent(cb: (e: BackupJobEvent) => void): Promise<UnlistenFn> {
  return listen<BackupJobEvent>('backup-job', (event) => cb(event.payload));
}
