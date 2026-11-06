import { describe, expect, it, vi } from 'vitest';
import type { NoteDocument, ServiceError } from '../domain';
import { SaveController } from '../editor';
import type { SaveNoteRequest, ServiceOutcome, PropertyWrite } from '../editor';
import { writeDatabaseCell, type DatabaseCellWriteIo } from './database-write';

const SELECT_DONE = { kind: 'select', text: 'Done' } as const;
const request = { noteId: 'n1', sourceRevision: 1, sourceHash: 'hash-1', readOnly: false, name: 'status', value: SELECT_DONE };
function documentOf(source = 'old', localRevision = 1): NoteDocument {
  return { noteId: 'n1', vaultId: 'v1', relativePath: 'notes/example.md', source,
    localRevision, contentHash: `hash-${localRevision}`, readOnly: false };
}
function serviceError(code: ServiceError['code']): ServiceError {
  return { code, messageKey: 'opennote.service.error', params: {}, retryable: false,
    operationId: null, retryAfterSeconds: null };
}
function setup(open = true) {
  const save = vi.fn(async (_request: SaveNoteRequest): Promise<ServiceOutcome<{
    noteId: string; localRevision: number; contentHash: string;
  }>> => ({ ok: true, value: { noteId: 'n1', localRevision: 2, contentHash: 'hash-2' } }));
  let operation = 0;
  const controller = new SaveController({ saveNote: save }, { uuid: () => `op-${++operation}` });
  if (open) controller.registerDocument(documentOf());
  const readNote = vi.fn(async (): Promise<ServiceOutcome<NoteDocument>> => ({ ok: true, value: documentOf() }));
  const applyPropertyEdit = vi.fn(async (source: string): Promise<ServiceOutcome<PropertyWrite>> => ({
    ok: true, value: { source: `${source}\nstatus:Done`, previous: { kind: 'unset' }, changed: true },
  }));
  const io: DatabaseCellWriteIo = { controller, canWrite: () => true, onSourceApplied: vi.fn(),
    service: { readNote, applyPropertyEdit } };
  return { io, save, readNote, applyPropertyEdit };
}

describe('database cell writeback', () => {
  it.each([true, false])('saves through SaveController with revision checks (registered=%s)', async open => {
    const { io, save, readNote } = setup(open);
    expect(await writeDatabaseCell(io, request)).toEqual({ ok: true, value: { changed: true } });
    expect(save).toHaveBeenCalledExactlyOnceWith({ noteId: 'n1', source: 'old\nstatus:Done', expectedRevision: 1, operationId: 'op-1' });
    expect(readNote).toHaveBeenCalledTimes(open ? 0 : 1);
    expect(io.onSourceApplied).toHaveBeenCalledWith('n1', 'old\nstatus:Done');
    expect(io.controller.snapshot('n1')).toMatchObject({ status: 'clean', revision: 2 });
  });

  it('preserves pre-existing unsaved words when applying a property edit', async () => {
    const { io, save, applyPropertyEdit } = setup();
    io.controller.setBufferText('n1', 'old\nunsaved words');
    expect((await writeDatabaseCell(io, request)).ok).toBe(true);
    expect(applyPropertyEdit).toHaveBeenCalledWith('old\nunsaved words', 'status', SELECT_DONE);
    expect(save.mock.calls[0]?.[0].source).toBe('old\nunsaved words\nstatus:Done');
  });

  it('does nothing when the property is unchanged', async () => {
    const { io, save, applyPropertyEdit } = setup(false);
    applyPropertyEdit.mockResolvedValue({ ok: true, value: { source: 'old', previous: SELECT_DONE, changed: false } });
    expect(await writeDatabaseCell(io, request)).toEqual({ ok: true, value: { changed: false } });
    expect(save).not.toHaveBeenCalled();
    expect(io.controller.snapshot('n1')).toBeNull();
  });

  it.each([true, false])('refuses a stale projected revision or hash (registered=%s)', async open => {
    const { io, save, applyPropertyEdit } = setup(open);
    expect((await writeDatabaseCell(io, { ...request, sourceRevision: 0 })).ok).toBe(false);
    expect((await writeDatabaseCell(io, { ...request, sourceHash: 'old-hash' })).ok).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(applyPropertyEdit).not.toHaveBeenCalled();
  });

  it('refuses a read-only projected row even when its editor is registered', async () => {
    const { io, save, applyPropertyEdit } = setup();
    expect((await writeDatabaseCell(io, { ...request, readOnly: true })).ok).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(applyPropertyEdit).not.toHaveBeenCalled();
  });

  it('retains a save conflict instead of overwriting a concurrent change', async () => {
    const { io, save } = setup(false);
    const error = serviceError('VERSION_CONFLICT');
    save.mockResolvedValueOnce({ ok: false, error });
    expect(await writeDatabaseCell(io, request)).toEqual({ ok: false, error });
    expect(io.controller.snapshot('n1')).toMatchObject({ status: 'conflict', failedOperation: { operationId: 'op-1' } });
    expect(io.controller.getBufferText('n1')).toBe('old\nstatus:Done');
  });

  it('does not replace input received during an asynchronous property rewrite', async () => {
    const { io, save, applyPropertyEdit } = setup();
    applyPropertyEdit.mockImplementation(async () => {
      io.controller.setBufferText('n1', 'new draft');
      return { ok: true, value: { source: 'edited', previous: { kind: 'unset' }, changed: true } };
    });
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    expect(io.controller.getBufferText('n1')).toBe('new draft');
    expect(save).not.toHaveBeenCalled();
    expect(io.onSourceApplied).not.toHaveBeenCalled();
  });

  it('retains a failed request with an unknown durable result and retries the original operation', async () => {
    const { io, save, applyPropertyEdit } = setup();
    save.mockResolvedValueOnce({ ok: false, error: serviceError('IO_DENIED') });
    io.controller.setBufferText('n1', 'possibly committed');
    await io.controller.requestSave('n1');
    const before = io.controller.snapshot('n1')?.failedOperation;
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    expect(io.controller.snapshot('n1')?.failedOperation).toEqual(before);
    expect(save).toHaveBeenCalledTimes(1);
    expect(applyPropertyEdit).not.toHaveBeenCalled();
    await io.controller.retryFailed('n1');
    expect(save.mock.calls[1]).toEqual(save.mock.calls[0]);
  });

  it('retains an unopened note write failure in the normal retry path', async () => {
    const { io, save } = setup(false);
    save.mockResolvedValueOnce({ ok: false, error: serviceError('IO_DENIED') });
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    const failed = io.controller.snapshot('n1')?.failedOperation;
    expect(failed).toMatchObject({ operationId: 'op-1', source: 'old\nstatus:Done', expectedRevision: 1 });
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    expect(save).toHaveBeenCalledTimes(1);
    await io.controller.retryFailed('n1');
    expect(save.mock.calls[1]).toEqual(save.mock.calls[0]);
  });

  it('explicit retry replays the original property save without a duplicate edit', async () => {
    const { io, save, applyPropertyEdit } = setup(false);
    save.mockResolvedValueOnce({ ok: false, error: serviceError('IO_DENIED') });
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    applyPropertyEdit.mockResolvedValue({ ok: true, value: {
      source: 'old\nstatus:Done', previous: SELECT_DONE, changed: false,
    } });
    expect(await writeDatabaseCell(io, { ...request, retryFailed: true })).toEqual({ ok: true, value: { changed: true } });
    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[1]).toEqual(save.mock.calls[0]);
    expect(io.controller.snapshot('n1')?.status).toBe('clean');
  });

  it('explicit retry resolves an older normal save before applying a new property', async () => {
    const { io, save, applyPropertyEdit } = setup();
    save.mockResolvedValueOnce({ ok: false, error: serviceError('IO_DENIED') });
    io.controller.setBufferText('n1', 'normal draft');
    await io.controller.requestSave('n1');
    expect((await writeDatabaseCell(io, { ...request, retryFailed: true })).ok).toBe(true);
    expect(save).toHaveBeenCalledTimes(3);
    expect(save.mock.calls[1]).toEqual(save.mock.calls[0]);
    expect(save.mock.calls[2]?.[0]).toMatchObject({ source: 'normal draft\nstatus:Done', expectedRevision: 2, operationId: 'op-2' });
    expect(applyPropertyEdit).toHaveBeenCalledWith('normal draft', 'status', SELECT_DONE);
  });

  it('does not bypass a note registered with a new draft during the rewrite', async () => {
    const { io, save, applyPropertyEdit } = setup(false);
    applyPropertyEdit.mockImplementation(async () => {
      io.controller.registerDocument(documentOf());
      io.controller.setBufferText('n1', 'new draft');
      return { ok: true, value: { source: 'edited', previous: { kind: 'unset' }, changed: true } };
    });
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(io.controller.getBufferText('n1')).toBe('new draft');
  });

  it.each(['readOnly', 'identity'] as const)('refuses an unopened note with changed %s', async change => {
    const { io, save, readNote, applyPropertyEdit } = setup(false);
    const doc = documentOf();
    if (change === 'readOnly') doc.readOnly = true;
    else doc.noteId = 'other';
    readNote.mockResolvedValue({ ok: true, value: doc });
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(applyPropertyEdit).not.toHaveBeenCalled();
  });

  it.each(['composition', 'conflict', 'workspace'] as const)('refuses initial %s', async state => {
    const { io, save, applyPropertyEdit } = setup();
    if (state === 'composition') io.controller.compositionStarted('n1');
    if (state === 'conflict') io.controller.reportExternalConflict('n1');
    if (state === 'workspace') io.canWrite = () => false;
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(applyPropertyEdit).not.toHaveBeenCalled();
  });

  it.each(['composition', 'revision', 'workspace'] as const)('discards a late result after %s changed', async state => {
    const { io, save, applyPropertyEdit } = setup();
    applyPropertyEdit.mockImplementation(async () => {
      if (state === 'composition') io.controller.compositionStarted('n1');
      if (state === 'revision') io.controller.registerDocument(documentOf('old', 3));
      if (state === 'workspace') io.canWrite = () => false;
      return { ok: true, value: { source: 'edited', previous: { kind: 'unset' }, changed: true } };
    });
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(io.controller.getBufferText('n1')).toBe('old');
  });

  it('refuses a second property mutation while a normal save is in flight', async () => {
    const { io, save, applyPropertyEdit } = setup();
    let finish!: (result: ServiceOutcome<{ noteId: string; localRevision: number; contentHash: string }>) => void;
    save.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    io.controller.setBufferText('n1', 'saving draft');
    const saving = io.controller.requestSave('n1');
    expect((await writeDatabaseCell(io, request)).ok).toBe(false);
    expect(applyPropertyEdit).not.toHaveBeenCalled();
    finish({ ok: true, value: { noteId: 'n1', localRevision: 2, contentHash: 'hash-2' } });
    await saving;
  });

  it('propagates read and pure rewrite failures without changing buffers', async () => {
    const { io, save, readNote, applyPropertyEdit } = setup(false);
    const error = serviceError('IO_DENIED');
    readNote.mockResolvedValueOnce({ ok: false, error });
    expect(await writeDatabaseCell(io, request)).toEqual({ ok: false, error });
    expect(applyPropertyEdit).not.toHaveBeenCalled();
    applyPropertyEdit.mockResolvedValue({ ok: false, error });
    expect(await writeDatabaseCell(io, request)).toEqual({ ok: false, error });
    expect(save).not.toHaveBeenCalled();
    expect(io.controller.snapshot('n1')).toBeNull();
  });
});
