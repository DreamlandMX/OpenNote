import type { Vault } from '../domain';
import type { Upgraded } from './ports';

export function acceptUpgrade(value: Upgraded, activeVaultId: string | null, handled: Set<string>, blocked: boolean, apply: (vault: Vault) => void): 'accepted' | 'wait' | 'rejected' {
  const { vault, migration } = value;
  if (!vault || !activeVaultId || vault.vaultId !== activeVaultId || vault.schemaVersion !== migration.toSchema || !((migration.fromSchema === 1 && [2, 3].includes(migration.toSchema)) || (migration.fromSchema === 2 && migration.toSchema === 3)) || !migration.operationId || !/^[0-9a-f]{64}$/.test(migration.manifestHash) || !Number.isSafeInteger(migration.noteCount) || migration.noteCount < 0) return 'rejected';
  const key = `${activeVaultId}:${migration.operationId}`;
  if (handled.has(key)) return 'accepted';
  if (blocked) return 'wait';
  handled.add(key);
  apply(vault);
  return 'accepted';
}
