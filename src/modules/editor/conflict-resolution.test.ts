import { expect, it, vi } from 'vitest';
import { ConflictSession } from './conflict-resolution';
import type { NoteDocument } from '../domain';
import type { NoteService } from './ports';
const local: NoteDocument = { noteId: 'note', vaultId: 'vault', relativePath: 'note.md', source: 'my unsaved text', contentHash: 'old', localRevision: 1, readOnly: false };
const disk = { ...local, source: 'external text', contentHash: 'disk', localRevision: 2 };
const copy = { ...local, noteId: 'copy', relativePath: 'copy.md' };
const fail = { ok: false as const, error: { code: 'DISK_FULL' as const, messageKey: 'full', params: {}, retryable: true, operationId: null, retryAfterSeconds: null } };
function setup() {
  let id = 0;
  const io = { prepareConflict: vi.fn<NoteService['prepareConflict']>(async () => ({ ok: true as const, value: disk })), writeCheckpoint: vi.fn<NoteService['writeCheckpoint']>(async () => ({ ok: true as const, value: { draftId: 'draft', noteId: 'note', sourceHash: 'hash', savedAt: '', sequence: 1 } })), createCopy: vi.fn<NoteService['createCopy']>(async () => ({ ok: true as const, value: copy })) };
  return { io, session: new ConflictSession(io, local, () => `op-${++id}`) };
}
it('never discards the local version when its durable checkpoint fails', async () => {
  const { io, session } = setup();
  io.writeCheckpoint.mockResolvedValueOnce(fail);
  expect(await session.resolve(disk)).toEqual(fail);
  expect(io.createCopy).not.toHaveBeenCalled();
  expect(io.writeCheckpoint.mock.calls[0]?.[0]).toMatchObject({ source: local.source, baseRevision: 1, baseContentHash: 'old' });
});
it('invalidates a changed preview before copying or checkpointing', async () => {
  const { io, session } = setup();
  const newer = { ...disk, localRevision: 3, contentHash: 'newer' };
  io.prepareConflict.mockResolvedValueOnce({ ok: true, value: newer });
  expect(await session.resolve(disk, 'copy.md')).toMatchObject({ ok: true, value: { kind: 'changed', disk: newer } });
  expect(io.writeCheckpoint).not.toHaveBeenCalled();
  expect(io.createCopy).not.toHaveBeenCalled();
});
it('retries the same copy operation after an uncertain failure and resolves only after both versions are durable', async () => {
  const { io, session } = setup();
  io.createCopy.mockRejectedValueOnce(new Error('response lost'));
  expect(await session.resolve(disk, 'copy.md')).toMatchObject({ ok: false, error: { code: 'IO_DENIED' } });
  expect(await session.resolve(disk, 'another.md')).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
  expect(await session.resolve(disk, 'copy.md')).toMatchObject({ ok: true, value: { kind: 'resolved', copy, disk } });
  expect(io.createCopy.mock.calls[0]).toEqual(io.createCopy.mock.calls[1]);
});
it('retains an already-created copy when disk changes during the action', async () => {
  const { io, session } = setup();
  const newer = { ...disk, localRevision: 3, contentHash: 'newer' };
  io.prepareConflict.mockResolvedValueOnce({ ok: true, value: disk }).mockResolvedValueOnce({ ok: true, value: newer });
  expect(await session.resolve(disk, 'copy.md')).toMatchObject({ ok: true, value: { kind: 'changed', copy, disk: newer } });
  io.prepareConflict.mockResolvedValue({ ok: true, value: newer });
  expect(await session.resolve(newer, 'copy.md')).toMatchObject({ ok: true, value: { kind: 'resolved', copy, disk: newer } });
  expect(io.createCopy).toHaveBeenCalledTimes(1);
});
it('allows a different destination after an explicit collision without reusing that copy operation', async () => {
  const { io, session } = setup();
  io.createCopy.mockResolvedValueOnce({ ok: false, error: { ...fail.error, code: 'PATH_COLLISION' } });
  expect(await session.resolve(disk, 'occupied.md')).toMatchObject({ ok: false, error: { code: 'PATH_COLLISION' } });
  expect(session.pendingCopyPath).toBeNull();
  expect(await session.resolve(disk, 'copy.md')).toMatchObject({ ok: true, value: { kind: 'resolved' } });
  expect(io.createCopy.mock.calls[0]?.[0].operationId).not.toBe(io.createCopy.mock.calls[1]?.[0].operationId);
  expect(io.writeCheckpoint.mock.calls[0]).toEqual(io.writeCheckpoint.mock.calls[1]);
});
