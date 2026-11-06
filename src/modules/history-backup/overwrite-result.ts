import type { Vault } from '../domain';
import type { Overwritten } from './ports';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const RFC3339 = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/;
const ABSOLUTE_WINDOWS_PATH = /^(?:[a-z]:[\\/]|\\\\\?\\[a-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+)/i;

function windowsParent(value: string): string | null {
  const trimmed = value.replace(/[\\/]+$/, '');
  const separator = Math.max(trimmed.lastIndexOf('\\'), trimmed.lastIndexOf('/'));
  return separator >= 2 ? trimmed.slice(0, separator).replace(/[\\/]+$/, '').toLowerCase() : null;
}

function windowsLeaf(value: string): string {
  const trimmed = value.replace(/[\\/]+$/, '');
  return trimmed.slice(Math.max(trimmed.lastIndexOf('\\'), trimmed.lastIndexOf('/')) + 1);
}

export function acceptOverwrite(
  value: Overwritten,
  activeVaultId: string | null,
  handled: Set<string>,
  blocked: boolean,
  apply: (vault: Vault, previousRoot: string) => void,
): 'accepted' | 'wait' | 'rejected' {
  const { result, vault } = value;
  if (
    !activeVaultId
    || !vault
    || vault.vaultId !== activeVaultId
    || vault.schemaVersion !== 3
    || !vault.writable
    || vault.syncEnabled
    || !UUID.test(result.operationId)
    || !UUID.test(result.snapshotId)
    || !UUID.test(result.safetySnapshotId)
    || !SHA256.test(result.manifestHash)
    || !RFC3339.test(result.completedAt)
    || Number.isNaN(Date.parse(result.completedAt))
    || !Number.isSafeInteger(result.noteCount)
    || result.noteCount < 0
    || !ABSOLUTE_WINDOWS_PATH.test(result.target)
    || !ABSOLUTE_WINDOWS_PATH.test(result.previousRoot)
    || windowsParent(result.target) === null
    || windowsParent(result.target) !== windowsParent(result.previousRoot)
    || windowsLeaf(result.previousRoot) !== `.opennote-overwrite-old-${result.operationId}`
  ) {
    return 'rejected';
  }
  const key = `${activeVaultId}:${result.operationId}`;
  if (handled.has(key)) {
    return 'accepted';
  }
  if (blocked) {
    return 'wait';
  }
  handled.add(key);
  apply(vault, result.previousRoot);
  return 'accepted';
}
