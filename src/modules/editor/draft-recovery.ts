import type { NoteDocument } from '../domain';
import type { DraftPage, NoteDraft, NoteService, ServiceOutcome } from './ports';

export interface DraftComparison {
  draft: NoteDraft;
  disk: ServiceOutcome<NoteDocument>;
}

/**
 * A failed or empty list never counts as "no drafts to recover": only a
 * successful listing with an item permits the startup notice.
 */
export function hasDraftCheckpoint(result: ServiceOutcome<DraftPage>): boolean {
  return result.ok && result.value.items.length > 0;
}

export async function loadDraftComparison(service: Pick<NoteService, 'readDraft' | 'readNote'>, draftId: string): Promise<ServiceOutcome<DraftComparison>> {
  const draft = await service.readDraft(draftId);
  if (!draft.ok) return draft;
  const disk = await service.readNote(draft.value.noteId);
  return { ok: true, value: { draft: draft.value, disk } };
}

export function sameDiskVersion(left: ServiceOutcome<NoteDocument>, right: ServiceOutcome<NoteDocument>): boolean {
  if (!left.ok || !right.ok) return !left.ok && !right.ok && left.error.code === 'OBJECT_NOT_FOUND' && right.error.code === 'OBJECT_NOT_FOUND';
  return left.value.noteId === right.value.noteId && left.value.contentHash === right.value.contentHash && left.value.localRevision === right.value.localRevision;
}
