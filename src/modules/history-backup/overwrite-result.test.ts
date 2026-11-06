import { describe, expect, it, vi } from 'vitest';
import { acceptOverwrite } from './overwrite-result';
import type { Overwritten } from './ports';

const result = (): Overwritten => ({
  result: {
    operationId: '00000000-0000-4000-8000-000000000001',
    snapshotId: '00000000-0000-4000-8000-000000000002',
    manifestHash: 'a'.repeat(64),
    target: '\\\\?\\E:\\Vaults\\Research',
    previousRoot: '\\\\?\\E:\\Vaults\\.opennote-overwrite-old-00000000-0000-4000-8000-000000000001',
    safetySnapshotId: '00000000-0000-4000-8000-000000000003',
    noteCount: 12,
    completedAt: '2026-10-19T22:00:00.000Z',
  },
  vault: {
    vaultId: 'vault',
    schemaVersion: 3,
    displayName: 'Research',
    syncEnabled: false,
    writable: true,
    findings: [],
  },
});

describe('overwrite completion handoff', () => {
  it('replaces the active session once and exposes the retained recovery path', () => {
    const handled = new Set<string>();
    const apply = vi.fn();
    const value = result();

    expect(acceptOverwrite(value, 'vault', handled, false, apply)).toBe('accepted');
    expect(acceptOverwrite(value, 'vault', handled, false, apply)).toBe('accepted');
    expect(apply).toHaveBeenCalledOnce();
    expect(apply).toHaveBeenCalledWith(value.vault, value.result.previousRoot);
  });

  it('waits for blocked editor work without consuming the completion', () => {
    const handled = new Set<string>();
    const apply = vi.fn();

    expect(acceptOverwrite(result(), 'vault', handled, true, apply)).toBe('wait');
    expect(handled.size).toBe(0);
    expect(apply).not.toHaveBeenCalled();
  });

  it('rejects another vault, missing rebind, or a non-writable modern vault', () => {
    const handled = new Set<string>();
    const apply = vi.fn();
    const missing = result();
    const legacy = result();
    const readOnly = result();
    const synced = result();
    missing.vault = null;
    legacy.vault!.schemaVersion = 1;
    readOnly.vault!.writable = false;
    synced.vault!.syncEnabled = true;

    for (const [value, active] of [[result(), 'another-vault'], [missing, 'vault'], [legacy, 'vault'], [readOnly, 'vault'], [synced, 'vault']] as const) {
      expect(acceptOverwrite(value, active, handled, false, apply)).toBe('rejected');
    }
    expect(apply).not.toHaveBeenCalled();
    expect(handled.size).toBe(0);
  });

  it('rejects malformed identity, hash, timestamp, count and recovery paths', () => {
    const mutations: ((value: Overwritten) => void)[] = [
      value => { value.result.operationId = 'not-a-uuid'; },
      value => { value.result.snapshotId = 'not-a-uuid'; },
      value => { value.result.safetySnapshotId = 'not-a-uuid'; },
      value => { value.result.manifestHash = 'abc'; },
      value => { value.result.completedAt = 'yesterday'; },
      value => { value.result.noteCount = -1; },
      value => { value.result.target = 'relative\\vault'; },
      value => { value.result.previousRoot = 'E:\\Vaults\\old'; },
      value => { value.result.previousRoot = '\\\\?\\D:\\Vaults\\.opennote-overwrite-old-00000000-0000-4000-8000-000000000001'; },
      value => { value.result.previousRoot = '\\\\?\\E:\\Vaults\\.opennote-overwrite-old-00000000-0000-4000-8000-000000000009'; },
    ];

    for (const mutate of mutations) {
      const handled = new Set<string>();
      const apply = vi.fn();
      const value = result();
      mutate(value);
      expect(acceptOverwrite(value, 'vault', handled, false, apply)).toBe('rejected');
      expect(apply).not.toHaveBeenCalled();
      expect(handled.size).toBe(0);
    }
  });
});
