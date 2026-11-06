import { expect, it, vi } from 'vitest';
import { hasDraftCheckpoint, loadDraftComparison, sameDiskVersion } from './draft-recovery';
import type { NoteDraft, NoteService } from './ports';
import type { NoteDocument, ServiceError } from '../domain';

const missing: ServiceError = { code: 'OBJECT_NOT_FOUND', messageKey: 'missing', params: {}, retryable: false, operationId: null, retryAfterSeconds: null };
const draft: NoteDraft = { draftId: 'd', noteId: 'n', relativePath: 'n.md', source: '<script>draft</script>', sourceHash: 'h', baseRevision: 1, baseContentHash: 'h1', savedAt: 'now', sequence: 1 };

it('retains a verified draft when the disk note is missing, without writing anything', async () => {
  const service = { readDraft: vi.fn(async () => ({ ok: true as const, value: draft })), readNote: vi.fn(async () => ({ ok: false as const, error: missing })) };
  const result = await loadDraftComparison(service as Pick<NoteService, 'readDraft' | 'readNote'>, 'd');
  expect(result).toEqual({ ok: true, value: { draft, disk: { ok: false, error: missing } } });
  expect(service.readNote).toHaveBeenCalledWith('n');
});

it('does not treat an unreadable disk as a missing or empty file', () => {
  expect(sameDiskVersion({ ok: false, error: missing }, { ok: false, error: { ...missing, code: 'IO_DENIED' } })).toBe(false);
  const value: NoteDocument = { noteId: 'n', vaultId: 'v', relativePath: 'n.md', source: 'disk', localRevision: 1, contentHash: 'h1', readOnly: false };
  expect(sameDiskVersion({ ok: true, value }, { ok: true, value: { ...value, localRevision: 2, contentHash: 'h2' } })).toBe(false);
});

it('offers draft recovery only after a successful listing with a checkpoint', () => {
  const summary = {
    noteId: 'n',
    draftId: 'd',
    savedAt: 'now',
    sequence: 1,
    sourceHash: 'h',
    relativePath: 'n.md',
    baseRevision: 1,
    baseContentHash: 'h1',
  };
  expect(hasDraftCheckpoint({ ok: true, value: { items: [summary], nextBeforeSequence: null } })).toBe(true);
  expect(hasDraftCheckpoint({ ok: true, value: { items: [], nextBeforeSequence: null } })).toBe(false);
  expect(hasDraftCheckpoint({ ok: false, error: missing })).toBe(false);
});
