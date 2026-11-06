import type { CatalogRefresh, NoteService, SourceWatchStop } from '../editor/ports';
import type { ServiceError } from '../domain';
import { serviceErrorFromThrown } from './port';

export function observeCatalog(service: Pick<NoteService, 'watchCatalogChanges' | 'refreshCatalog'>, callbacks: {
  refreshed(value: CatalogRefresh): void; watching(): void;
  watchFailed(error: ServiceError): void; scanFailed(error: ServiceError): void;
}): { stop(): void; refresh(): Promise<void> } {
  let active = true;
  let watchHealthy = true;
  let detach: SourceWatchStop | null = null;
  let flight: Promise<void> | null = null;
  let pending = false;
  let scanBlocked = false;
  const detachWatch = (): void => { const current = detach; detach = null; if (current) void current().catch(() => undefined); };
  const stop = (): void => { active = false; detachWatch(); };
  const refresh = (automatic = false): Promise<void> => {
    if (!active || (automatic && scanBlocked)) return Promise.resolve();
    if (!automatic) scanBlocked = false;
    pending = true;
    if (flight) return flight;
    flight = (async () => {
      do {
        pending = false;
        try {
          const outcome = await service.refreshCatalog();
          if (!active) break;
          if (outcome.ok) callbacks.refreshed(outcome.value);
          else { scanBlocked = true; pending = false; callbacks.scanFailed(outcome.error); }
        } catch (error) { if (active) { scanBlocked = true; pending = false; callbacks.scanFailed(serviceErrorFromThrown(error)); } }
      } while (active && pending);
    })().finally(() => { flight = null; if (active && pending) void refresh(true); });
    return flight;
  };
  try {
    void service.watchCatalogChanges(error => {
      if (!active) return;
      if (error) { watchHealthy = false; callbacks.watchFailed(error); detachWatch(); }
      else void refresh(true);
    }).then(outcome => {
      if (!outcome.ok) { watchHealthy = false; if (active) callbacks.watchFailed(outcome.error); return; }
      detach = outcome.value;
      if (!active || !watchHealthy) { detachWatch(); return; }
      callbacks.watching(); void refresh(true);
    }, error => { watchHealthy = false; if (active) callbacks.watchFailed(serviceErrorFromThrown(error)); });
  } catch (error) { watchHealthy = false; callbacks.watchFailed(serviceErrorFromThrown(error)); }
  return { stop, refresh };
}
