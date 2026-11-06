import { describe, expect, it } from 'vitest';
import type { ServiceError, Vault } from '../domain';
import { disconnectedShellPort, type ShellCallResult, type ShellPort } from './port';
import { reopenLastVault, shouldReopenLastVault } from './last-vault';

const vault = { vaultId: 'v1', displayName: 'Notes', writable: true } as unknown as Vault;

const failure: ServiceError = {
  code: 'UNSUPPORTED_SCHEMA',
  messageKey: '',
  params: {},
  retryable: false,
  operationId: null,
  retryAfterSeconds: null,
};

function portReopening(reopen: () => Promise<ShellCallResult<Vault | null>>): ShellPort {
  return { ...disconnectedShellPort, kind: 'tauri', reopenLastVault: reopen };
}

describe('reopening the previous vault at startup', () => {
  it('is tried only after the intro and only when the host supports it', () => {
    const port = portReopening(async () => ({ ok: true, value: null }));
    expect(shouldReopenLastVault(port, true)).toBe(true);
    expect(shouldReopenLastVault(port, false)).toBe(false);
    expect(shouldReopenLastVault(disconnectedShellPort, true)).toBe(false);
  });

  it('returns the reopened vault', async () => {
    await expect(reopenLastVault(portReopening(async () => ({ ok: true, value: vault })))).resolves.toBe(vault);
  });

  it('falls back to the start screen when there is nothing to reopen or it fails', async () => {
    await expect(reopenLastVault(portReopening(async () => ({ ok: true, value: null })))).resolves.toBeNull();
    await expect(reopenLastVault(portReopening(async () => ({ ok: false, error: failure })))).resolves.toBeNull();
    await expect(reopenLastVault(portReopening(() => Promise.reject(new Error('ipc'))))).resolves.toBeNull();
    await expect(reopenLastVault(disconnectedShellPort)).resolves.toBeNull();
  });
});
