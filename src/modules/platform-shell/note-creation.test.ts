import { describe, expect, it, vi } from 'vitest';
import type { NoteDocument, ServiceError } from '../domain';
import type { CreateNoteRequest, ServiceOutcome } from '../editor';
import { folderOf, NoteCreationController, titleOf, titleToPath, untitledPath } from './note-creation';

const doc: NoteDocument = { noteId: 'created', vaultId: 'v1', relativePath: 'Template.md', source: 'expanded body', localRevision: 0, contentHash: 'hash', readOnly: false };
const success: ServiceOutcome<NoteDocument> = { ok: true, value: doc };
function failed(code: ServiceError['code']): ServiceOutcome<NoteDocument> {
  return { ok: false, error: { code, messageKey: 'opennote.service.error', params: {}, retryable: true, operationId: null, retryAfterSeconds: null } };
}
function setup() {
  const createNote = vi.fn(async (_request: CreateNoteRequest): Promise<ServiceOutcome<NoteDocument>> => success);
  let sequence = 0;
  const io = { createNote, canCreate: () => true, newOperationId: () => `op-${++sequence}` };
  return { controller: new NoteCreationController(io), io, createNote };
}

describe('retained note creation', () => {
  it('retains the exact template request and joins duplicate clicks while working', async () => {
    const { controller, createNote } = setup();
    let finish!: (result: ServiceOutcome<NoteDocument>) => void;
    createNote.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    const first = controller.create('Template.md', 'expanded body');
    expect(controller.snapshot()).toMatchObject({ working: true, request: { relativePath: 'Template.md', source: 'expanded body', operationId: 'op-1' } });
    expect(controller.create('Template.md', 'expanded body')).toBe(first);
    expect(controller.dismissRejected()).toBe(false);
    await Promise.resolve();
    expect(createNote).toHaveBeenCalledTimes(1);
    finish(success);
    expect(await first).toEqual(success);
    expect(controller.snapshot()).toEqual({ request: null, working: false, error: null, uncertain: false });
  });

  it('replays an acknowledged-lost commit without a second note or changed operation ID', async () => {
    let committed = 0;
    const requests: CreateNoteRequest[] = [];
    const controller = new NoteCreationController({ canCreate: () => true, newOperationId: () => 'original-id', createNote: async request => {
      requests.push({ ...request });
      if (committed === 0) { committed++; return failed('IO_DENIED'); }
      return success;
    } });
    expect((await controller.create('Template.md', 'expanded body')).ok).toBe(false);
    expect(controller.snapshot()).toMatchObject({ working: false, uncertain: true, request: { source: 'expanded body' } });
    expect(await controller.retry()).toEqual(success);
    expect(requests).toHaveLength(2);
    expect(requests[1]).toEqual(requests[0]);
    expect(committed).toBe(1);
  });

  it('never replaces an uncertain request with a new path or blank template body', async () => {
    const { controller, createNote } = setup();
    createNote.mockResolvedValueOnce(failed('IO_DENIED'));
    await controller.create('Template.md', 'expanded body');
    expect((await controller.create('Other.md', '')).ok).toBe(false);
    expect((await controller.create('Template.md', '')).ok).toBe(false);
    expect(controller.dismissRejected()).toBe(false);
    expect(controller.snapshot().request).toEqual({ relativePath: 'Template.md', source: 'expanded body', operationId: 'op-1' });
    expect(createNote).toHaveBeenCalledTimes(1);
  });

  it.each(['INVALID_INPUT', 'PATH_COLLISION', 'UNSUPPORTED_SCHEMA'] as const)('allows a corrected fresh request after definite %s rejection', async code => {
    const { controller, createNote } = setup();
    createNote.mockResolvedValueOnce(failed(code));
    await controller.create('bad.md', 'expanded body');
    expect(controller.snapshot().uncertain).toBe(false);
    await controller.create('fixed.md', 'expanded body');
    expect(createNote.mock.calls[1]?.[0]).toEqual({ relativePath: 'fixed.md', source: 'expanded body', operationId: 'op-2' });
  });

  it('does not downgrade an earlier unknown result after a later validation refusal', async () => {
    const { controller, createNote } = setup();
    createNote.mockResolvedValueOnce(failed('IO_DENIED')).mockResolvedValueOnce(failed('INVALID_INPUT'));
    await controller.create('Template.md', 'expanded body');
    await controller.retry();
    expect(controller.snapshot().uncertain).toBe(true);
    expect(controller.dismissRejected()).toBe(false);
    expect((await controller.create('new.md', '')).ok).toBe(false);
    expect(createNote.mock.calls[1]).toEqual(createNote.mock.calls[0]);
  });

  it('retains a thrown failure and refuses writes after permission or lifetime loss', async () => {
    const { controller, createNote, io } = setup();
    createNote.mockRejectedValueOnce(Error('lost reply'));
    expect(await controller.create('Template.md', 'expanded body')).toMatchObject({ ok: false, error: { code: 'IO_DENIED' } });
    io.canCreate = () => false;
    expect((await controller.retry()).ok).toBe(false);
    expect(createNote).toHaveBeenCalledTimes(1);
    expect(controller.snapshot().request?.operationId).toBe('op-1');
  });

  it('can dismiss a known rejection without another write and preserves subscribers across retries', async () => {
    const { controller, createNote } = setup();
    const published = vi.fn();
    const unsubscribe = controller.subscribe(published);
    createNote.mockResolvedValueOnce(failed('PATH_COLLISION'));
    await controller.create('Template.md', 'expanded body');
    expect(controller.dismissRejected()).toBe(true);
    expect(controller.snapshot().request).toBeNull();
    unsubscribe();
    const before = published.mock.calls.length;
    await controller.create('New.md', 'new body');
    expect(published).toHaveBeenCalledTimes(before);
  });
});

describe('untitled path', () => {
  it('numbers past taken names, case-insensitively, in the given folder', () => {
    expect(untitledPath([], '', 'Untitled')).toBe('Untitled.md');
    expect(untitledPath(['untitled.md', 'Untitled 1.md'], '', 'Untitled')).toBe('Untitled 2.md');
    expect(untitledPath(['Untitled.md'], 'journal', 'Untitled')).toBe('journal/Untitled.md');
    expect(untitledPath(['journal/未命名.md'], 'journal', '未命名')).toBe('journal/未命名 1.md');
  });
  it('reads the folder and title of a path', () => {
    expect(folderOf('a/b/c.md')).toBe('a/b');
    expect(folderOf('c.md')).toBe('');
    expect(titleOf('a/My Note.MD')).toBe('My Note');
  });
});

describe('title to path', () => {
  it('keeps the folder and replaces characters a file name cannot hold', () => {
    expect(titleToPath('  Weekly review ', 'journal')).toBe('journal/Weekly review.md');
    expect(titleToPath('a:b?', '')).toBe('a-b-.md');
    expect(titleToPath('Ends with dots...', '')).toBe('Ends with dots.md');
  });
  it('moves into a subfolder on a slash and refuses an empty title', () => {
    expect(titleToPath('ideas/new', '')).toBe('ideas/new.md');
    expect(titleToPath('../x', 'a')).toBe('a/x.md');
    expect(titleToPath('  ', 'a')).toBeNull();
    expect(titleToPath('/', '')).toBeNull();
  });
});
