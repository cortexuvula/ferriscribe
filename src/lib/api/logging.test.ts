import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { frontendLog, getLogPath, getRecentLogs, log } from './logging';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  // Keep test output clean — log.* mirrors to console.
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.spyOn(console, 'info').mockImplementation(() => {});
  vi.spyOn(console, 'debug').mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('logging api', () => {
  it('frontendLog null-coalesces context when omitted', async () => {
    await frontendLog('info', 'hello');
    expect(invokeMock).toHaveBeenCalledWith('frontend_log', {
      level: 'info',
      message: 'hello',
      context: null,
    });
  });

  it('frontendLog forwards context object when provided', async () => {
    await frontendLog('error', 'boom', { component: 'X', count: 3 });
    expect(invokeMock).toHaveBeenCalledWith('frontend_log', {
      level: 'error',
      message: 'boom',
      context: { component: 'X', count: 3 },
    });
  });

  it('getLogPath invokes get_log_path with no args', async () => {
    await getLogPath();
    expect(invokeMock).toHaveBeenCalledWith('get_log_path');
  });

  it('getRecentLogs defaults lines to 200 when omitted', async () => {
    await getRecentLogs();
    expect(invokeMock).toHaveBeenCalledWith('get_recent_logs', { lines: 200 });
    invokeMock.mockReset();
    await getRecentLogs(50);
    expect(invokeMock).toHaveBeenCalledWith('get_recent_logs', { lines: 50 });
  });

  it('log.error / .warn / .info / .debug each forward to frontend_log with the right level', async () => {
    log.error('e');
    log.warn('w');
    log.info('i');
    log.debug('d');
    // frontendLog is fire-and-forget under log.*, so let microtasks flush.
    await new Promise((r) => setTimeout(r, 0));
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'frontend_log', { level: 'error', message: 'e', context: null });
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'frontend_log', { level: 'warn',  message: 'w', context: null });
    expect(invokeMock).toHaveBeenNthCalledWith(3, 'frontend_log', { level: 'info',  message: 'i', context: null });
    expect(invokeMock).toHaveBeenNthCalledWith(4, 'frontend_log', { level: 'debug', message: 'd', context: null });
  });

  it('log.* swallows backend rejections silently (no unhandled rejection)', async () => {
    invokeMock.mockRejectedValue(new Error('backend gone'));
    expect(() => log.error('e')).not.toThrow();
    await new Promise((r) => setTimeout(r, 0));
    // Reaching here without an unhandled rejection is the assertion.
  });
});
