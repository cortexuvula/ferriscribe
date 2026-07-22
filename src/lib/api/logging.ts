import { invoke } from '@tauri-apps/api/core';

export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace';

/**
 * Send a log entry from the frontend to the backend logging system.
 * This persists the log to the rolling log file alongside Rust-side logs.
 */
export async function frontendLog(
  level: LogLevel,
  message: string,
  context?: Record<string, unknown>,
): Promise<void> {
  return invoke('frontend_log', { level, message, context: context ?? null });
}

/** Return the absolute path to the log directory. */
export async function getLogPath(): Promise<string> {
  return invoke('get_log_path');
}

/** Return the last N lines from the most recent log file. */
export async function getRecentLogs(lines?: number): Promise<string> {
  return invoke('get_recent_logs', { lines: lines ?? 200 });
}

/**
 * Global logger that writes to both console and the backend log file.
 *
 * Usage:
 *   import { log } from '$lib/api/logging';
 *   log.error('Transcription failed', { component: 'RecordTab', recordingId });
 *   log.info('Recording started');
 */
export const log = {
  error(message: string, context?: Record<string, unknown>) {
    console.error(`[FerriScribe] ${message}`, context ?? '');
    frontendLog('error', message, context).catch(() => {});
  },
  warn(message: string, context?: Record<string, unknown>) {
    console.warn(`[FerriScribe] ${message}`, context ?? '');
    frontendLog('warn', message, context).catch(() => {});
  },
  info(message: string, context?: Record<string, unknown>) {
    console.warn(`[FerriScribe] ${message}`, context ?? '');
    frontendLog('info', message, context).catch(() => {});
  },
  debug(message: string, context?: Record<string, unknown>) {
    console.warn(`[FerriScribe] ${message}`, context ?? '');
    frontendLog('debug', message, context).catch(() => {});
  },
};
