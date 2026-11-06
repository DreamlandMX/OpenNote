import type { Vault } from '../domain';
import type { ShellPort } from './port';

/** Whether startup should try the previous session's vault before drawing. */
export function shouldReopenLastVault(port: ShellPort, introSeen: boolean): boolean {
  return introSeen && port.reopenLastVault !== undefined;
}

/**
 * The previous session's vault, or `null` when the start screen should show.
 * A failure is not reported: the host has already forgotten a vault it could
 * not reopen, and the start screen is the way to pick another.
 */
export async function reopenLastVault(port: ShellPort): Promise<Vault | null> {
  if (port.reopenLastVault === undefined) {
    return null;
  }
  try {
    const outcome = await port.reopenLastVault();
    return outcome.ok ? outcome.value : null;
  } catch {
    return null;
  }
}
