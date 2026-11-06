import { describe, expect, it } from 'vitest';
import type { LatexEnvironmentStatus } from '../latex-workspace/ports';
import { environmentView, formatBytes, reasonKey } from './latex-environment-view';

function status(overrides: Partial<LatexEnvironmentStatus> = {}): LatexEnvironmentStatus {
  return { environment: { kind: 'notPrepared' }, preparing: null, last: null, ...overrides };
}

describe('latex environment view', () => {
  it('shows sizes in readable decimal units', () => {
    expect(formatBytes(21_060_223, 'en')).toBe('21.1 MB');
    expect(formatBytes(512, 'en')).toBe('512 B');
    expect(formatBytes(268_435_456, 'en')).toBe('268.4 MB');
  });

  it('treats an invalid environment exactly like an unprepared one', () => {
    expect(environmentView(null)).toBe('loading');
    expect(environmentView(status())).toBe('notReady');
    expect(environmentView(status({ environment: { kind: 'invalid', reasonKey: 'latexEnv.invalid.files' } }))).toBe('notReady');
    expect(environmentView(status({ environment: { kind: 'ready', totalBytes: 1, fileCount: 1 } }))).toBe('ready');
  });

  it('shows progress while preparing, whatever the stored environment', () => {
    const preparing = status({
      environment: { kind: 'ready', totalBytes: 1, fileCount: 1 },
      preparing: { operationId: 'op', progress: { stage: 'downloading', bytes: 0, totalBytes: 10, step: 0, steps: 0 } },
    });
    expect(environmentView(preparing)).toBe('preparing');
  });

  it('never shows a raw or unknown host key', () => {
    expect(reasonKey('latexEnv.reason.timedOut')).toBe('latexEnv.reason.timedOut');
    expect(reasonKey('error.somethingNew')).toBe('latexEnv.reason.unknown');
    expect(reasonKey(null)).toBe('latexEnv.reason.unknown');
  });
});
