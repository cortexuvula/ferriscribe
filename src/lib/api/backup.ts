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
  destinationKind: 'agent' | 'folder' | 'local-only';
  destinationPresent: boolean;
  destinationMissing: boolean;
}

export interface DestinationProbe {
  writable: boolean;
  freeBytes: number | null;
  problem: string | null;
}

export interface AgentProbe {
  ok: boolean;
  problem: string | null;
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
  destPath: string | null,
): Promise<string> {
  return invoke('backup_install_schedule', { hour, minute, url, token, destPath });
}

export function testBackupDestination(destPath: string): Promise<DestinationProbe> {
  return invoke('backup_test_destination', { destPath });
}

export function testBackupAgent(url: string, token: string): Promise<AgentProbe> {
  return invoke('backup_test_agent', { url, token });
}

/** True when data is genuinely protected off this machine: the recovery
 *  key exists, an off-machine destination is configured, and the last
 *  restore drill passed recently. Local-only setups are NOT protected. */
export function isProtected(s: BackupStatus): boolean {
  return (
    s.wrappingKeyPresent && s.destinationKind !== 'local-only' && s.drillPassed && !s.stale
  );
}

/** Parse an <input type="time"> value ("HH:MM") into [hour, minute], or
 *  null when invalid. */
export function parseTimeOfDay(v: string): [number, number] | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(v.trim());
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  if (h > 23 || min > 59) return null;
  return [h, min];
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
