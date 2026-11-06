import { serviceErrorFromThrown } from './port';
import type { CatalogRefresh, NoteService } from '../editor/ports';
import type { NoteDocument, NoteSummary, ServiceError } from '../domain';
import type { SaveController } from '../editor/save-controller';
export interface CatalogSyncOptions {
  service: Pick<NoteService, 'readNote'>; controller: SaveController; allowed(): boolean;
  currentSummary(noteId: string): NoteSummary | null;
  document(value: NoteDocument): void; metadata(value: NoteSummary): void; failed(error: ServiceError, noteId: string): void;
}
export function createCatalogSync(options: CatalogSyncOptions): { apply(value: CatalogRefresh): Promise<void>; dispose(): void } {
  let active = true;
  let latest: CatalogRefresh | null = null;
  let desired = new Map<string, string>();
  const handled = new Map<string, string>();
  const pending = new Set<string>();
  const tokenOf = (summary: NoteSummary, unavailable: boolean) => JSON.stringify([summary.relativePath, summary.contentHash, summary.localRevision, summary.readOnly, unavailable]);
  const apply = async (value: CatalogRefresh): Promise<void> => {
    if (!active || value.deferred) return;
    latest = value;
    const unavailable = new Set(value.unavailableNoteIds);
    desired = new Map(value.notes.map(summary => [summary.noteId, tokenOf(summary, unavailable.has(summary.noteId))]));
    if (!options.allowed()) return;
    for (const summary of value.notes) {
      if (!active || !options.allowed()) return;
      const id = summary.noteId;
      const missing = unavailable.has(id);
      const token = tokenOf(summary, missing);
      if (desired.get(id) !== token || handled.get(id) === token || pending.has(id)) continue;
      const before = options.controller.snapshot(id);
      const shown = options.currentSummary(id);
      if (!before || before.inFlight || (!shown && !before.dirty && before.status === 'clean')) continue;
      if (before.revision > summary.localRevision) { handled.set(id, token); continue; }
      const baselineChanged = before.revision !== summary.localRevision || before.contentHash !== summary.contentHash;
      const metadataChanged = shown !== null && (shown.relativePath !== summary.relativePath || shown.readOnly !== summary.readOnly);
      if (missing) {
        if (before.dirty && !options.controller.reportExternalConflict(id)) continue;
        options.metadata({ ...summary, readOnly: true }); handled.set(id, token); continue;
      }
      if (!baselineChanged && !metadataChanged) continue;
      if (before.dirty || before.status === 'failed' || before.status === 'conflict') {
        if ((baselineChanged || summary.readOnly) && !options.controller.reportExternalConflict(id)) continue;
        options.metadata(summary); handled.set(id, token); continue;
      }
      pending.add(id);
      let retry = false;
      try {
        const outcome = await options.service.readNote(id);
        if (!active || !options.allowed() || desired.get(id) !== token) continue;
        const now = options.controller.snapshot(id);
        if (!now || now.inFlight) continue;
        if (now.dirty || now.status === 'failed' || now.status === 'conflict') {
          if (options.controller.reportExternalConflict(id)) { options.metadata(summary); handled.set(id, token); }
          continue;
        }
        if (now.revision !== before.revision || now.contentHash !== before.contentHash) { retry = true; continue; }
        handled.set(id, token);
        if (outcome.ok) {
          options.controller.adoptExternalContent(id, outcome.value);
          options.document(outcome.value);
        } else {
          options.metadata({ ...summary, readOnly: true }); options.failed(outcome.error, id);
        }
      } catch (error) {
        if (active) { handled.set(id, token); options.failed(serviceErrorFromThrown(error), id); }
      } finally {
        pending.delete(id);
        if (active && options.allowed() && latest && (retry || desired.get(id) !== token)) void apply(latest);
      }
    }
  };
  return { apply, dispose: () => { active = false; } };
}
