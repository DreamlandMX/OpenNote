import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ServiceOutcome, SourceWatchEvent, SourceWatchStop } from '../editor/ports';
import { serviceErrorFromThrown } from './port';

type HostEvent = SourceWatchEvent & { watchId: string; vaultId: string };

export async function watchChanges(vaultId: string, watchId: string, noteIds: readonly string[] | null, onChange: (event: SourceWatchEvent) => void): Promise<ServiceOutcome<SourceWatchStop>> {
  let active = true;
  let detach: (() => void) | null = null;
  const stop = async (): Promise<void> => {
    if (!active) return;
    active = false;
    // Always detach locally, including after a vault switch or a failed host stop.
    await Promise.allSettled([
      Promise.resolve().then(() => detach?.()),
      Promise.resolve().then(() => invoke('stop_source_watch', { vaultId, watchId })),
    ]);
  };
  try {
    detach = await listen<HostEvent>(noteIds === null ? 'opennote://catalog-changed' : 'opennote://source-changed', event => {
      if (active && event.payload.vaultId === vaultId && event.payload.watchId === watchId) onChange(event.payload);
    });
    await (noteIds === null ? invoke('start_catalog_watch', { vaultId, watchId }) : invoke('start_source_watch', { vaultId, watchId, noteIds }));
    return { ok: true, value: stop };
  } catch (error) {
    await stop();
    return { ok: false, error: serviceErrorFromThrown(error) };
  }
}
