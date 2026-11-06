import type { NoteService, SourceWatchStop } from '../editor/ports';
import type { ServiceError } from '../domain';
import { serviceErrorFromThrown } from './port';

export function observeSources(service: Pick<NoteService, 'watchSourceChanges'>, noteIds: readonly string[], callbacks: { ready(): void; changed(): void; failed(error: ServiceError): void }): () => void {
  let active = true;
  let detach: SourceWatchStop | null = null;
  const wanted = new Set(noteIds);
  const stop = (): void => {
    active = false;
    const current = detach; detach = null;
    if (current) void current().catch(() => undefined);
  };
  const failed = (error: ServiceError): void => {
    if (!active) return;
    callbacks.failed(error);
    stop();
  };
  try {
    void service.watchSourceChanges(noteIds, event => {
      if (!active) return;
      if (event.error) failed(event.error);
      else if (event.noteIds.some(id => wanted.has(id))) callbacks.changed();
    }).then(outcome => {
      if (!outcome.ok) { failed(outcome.error); return; }
      detach = outcome.value;
      if (!active) { stop(); return; }
      // Refresh after arming the native observer to close the setup race.
      callbacks.ready();
    }, error => failed(serviceErrorFromThrown(error)));
  } catch (error) { failed(serviceErrorFromThrown(error)); }
  return stop;
}
