import { describe, expect, it, vi } from 'vitest';
import { acceptUpgrade } from './upgrade-result';
import type { Upgraded } from './ports';

const result = (): Upgraded => ({
  migration: { operationId: '00000000-0000-4000-8000-000000000001', manifestHash: 'a'.repeat(64), fromSchema: 1, toSchema: 3, noteCount: 3 },
  vault: { vaultId: 'vault', schemaVersion: 3, displayName: 'original name', syncEnabled: false, writable: true, findings: [] },
});

describe('upgrade completion handoff', () => {
  it('accepts schema two to three and preserves an older one to two receipt', () => {
    const newer = result(); newer.migration.fromSchema = 2;
    expect(acceptUpgrade(newer, 'vault', new Set(), false, vi.fn())).toBe('accepted');
    const legacy = result(); legacy.migration.toSchema = 2; legacy.vault!.schemaVersion = 2; legacy.vault!.writable = false;
    expect(acceptUpgrade(legacy, 'vault', new Set(), false, vi.fn())).toBe('accepted');
    const invalid = result(); invalid.migration.fromSchema = 3;
    expect(acceptUpgrade(invalid, 'vault', new Set(), false, vi.fn())).toBe('rejected');
  });
  it('reloads once across replayed terminal jobs and retains blocked editor state', () => {
    const handled = new Set<string>(); const apply = vi.fn(); const value = result();
    expect(acceptUpgrade(value, 'vault', handled, true, apply)).toBe('wait');
    expect(apply).not.toHaveBeenCalled();
    expect(handled.size).toBe(0);
    expect(acceptUpgrade(value, 'vault', handled, false, apply)).toBe('accepted');
    expect(acceptUpgrade(value, 'vault', handled, false, apply)).toBe('accepted');
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith(value.vault);
  });
  it('rejects missing rebind, another vault and mismatched format before replacing the session', () => {
    const apply = vi.fn(); const handled = new Set<string>();
    expect(acceptUpgrade({ ...result(), vault: null }, 'vault', handled, false, apply)).toBe('rejected');
    expect(acceptUpgrade(result(), 'another-vault', handled, false, apply)).toBe('rejected');
    const wrong = result(); wrong.vault!.schemaVersion = 1;
    expect(acceptUpgrade(wrong, 'vault', handled, false, apply)).toBe('rejected');
    expect(apply).not.toHaveBeenCalled(); expect(handled.size).toBe(0);
  });
});
