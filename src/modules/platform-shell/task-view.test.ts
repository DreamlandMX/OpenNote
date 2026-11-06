import { describe, expect, it, vi } from 'vitest';
import { SaveController, type TaskRow } from '../editor';
import type { NoteDocument, ServiceError } from '../domain';
import {
  filterCounts,
  filterTasks,
  groupOf,
  groupTasks,
  inFolder,
  sourceFolders,
  toggleRequest,
  writeTaskSource,
} from './task-view';

function task(overrides: Partial<TaskRow> & { text: string }): TaskRow {
  return {
    noteId: 'n1',
    relativePath: 'notes/a.md',
    sourceRevision: 1,
    sourceHash: 'hash-1',
    readOnly: false,
    offset: 0,
    line: 0,
    status: 'open',
    due: null,
    depth: 0,
    ...overrides,
  };
}

const today = '2026-10-15';

describe('task grouping', () => {
  it('puts a dated task in today, upcoming or overdue', () => {
    expect(groupOf(task({ text: 'a', due: today }), today)).toBe('today');
    expect(groupOf(task({ text: 'b', due: '2026-10-18' }), today)).toBe('upcoming');
    expect(groupOf(task({ text: 'c', due: '2026-10-01' }), today)).toBe('overdue');
  });

  it('separates undated open tasks from done ones regardless of date', () => {
    expect(groupOf(task({ text: 'a' }), today)).toBe('undated');
    expect(groupOf(task({ text: 'b', due: today, status: 'done' }), today)).toBe('done');
  });

  it('orders sections overdue, today, upcoming, undated, done and omits empty ones', () => {
    const tasks = [
      task({ text: 'upcoming', due: '2026-10-20' }),
      task({ text: 'today', due: today }),
      task({ text: 'done', status: 'done' }),
      task({ text: 'overdue', due: '2026-10-01' }),
      task({ text: 'undated' }),
    ];
    const groups = groupTasks(tasks, today, 'all');
    expect(groups.map((group) => group.key)).toEqual([
      'overdue',
      'today',
      'upcoming',
      'undated',
      'done',
    ]);
    expect(groups[1]?.tasks[0]?.task.text).toBe('today');
  });

  it('keeps the source index so keys stay stable across filters', () => {
    const tasks = [task({ text: 'a' }), task({ text: 'b', status: 'done' })];
    const open = filterTasks(tasks, 'open');
    expect(open.map((entry) => entry.index)).toEqual([0]);
    const done = filterTasks(tasks, 'done');
    expect(done.map((entry) => entry.index)).toEqual([1]);
    expect(filterTasks(tasks, 'all')).toHaveLength(2);
  });

  it('returns no groups for an empty list', () => {
    expect(groupTasks([], today, 'all')).toEqual([]);
  });
});

describe('task counts and folders', () => {
  it('counts each filter', () => {
    const tasks = [
      task({ text: 'a' }),
      task({ text: 'b' }),
      task({ text: 'c', status: 'done' }),
    ];
    expect(filterCounts(tasks)).toEqual({ all: 3, open: 2, done: 1 });
    expect(filterCounts([])).toEqual({ all: 0, open: 0, done: 0 });
  });

  it('lists distinct source folders and matches a subtree', () => {
    const tasks = [
      task({ text: 'a', relativePath: 'work/plan/a.md' }),
      task({ text: 'b', relativePath: 'work/b.md' }),
      task({ text: 'c', relativePath: 'root.md' }),
    ];
    expect(sourceFolders(tasks)).toEqual(['work', 'work/plan']);
    expect(inFolder(tasks[0]!, 'work')).toBe(true);
    expect(inFolder(tasks[0]!, 'work/plan')).toBe(true);
    expect(inFolder(tasks[2]!, 'work')).toBe(false);
    expect(inFolder(tasks[2]!, null)).toBe(true);
  });

  it('normalizes Windows separators when matching a folder', () => {
    const windows = task({ text: 'a', relativePath: 'work\\plan\\a.md' });
    expect(inFolder(windows, 'work/plan')).toBe(true);
    expect(sourceFolders([windows])).toEqual(['work', 'work/plan']);
  });
});

describe('toggle requests', () => {
  it('carries the offset and the exact text so a moved line is refused', () => {
    const row = task({ text: 'buy milk', offset: 42 });
    expect(toggleRequest(row, 'done')).toEqual({
      offset: 42,
      expectedText: 'buy milk',
      status: 'done',
    });
  });
});


describe('task source writeback', () => {
  const source = '- [ ] same\n- [ ] same\n';
  const document: NoteDocument = {
    noteId: 'n1', vaultId: 'v1', relativePath: 'notes/a.md', source,
    localRevision: 1, contentHash: 'hash-1', readOnly: false,
  };
  const failure: ServiceError = { code: 'IO_DENIED', messageKey: 'opennote.service.error',
    params: {}, retryable: true, operationId: null, retryAfterSeconds: null };
  function setup(open = true) {
    const save = vi.fn(async () => ({ ok: true as const, value: {
      noteId: 'n1', localRevision: 2, contentHash: 'hash-2',
    } }));
    const controller = new SaveController({ saveNote: save }, { uuid: () => 'task-operation' });
    if (open) controller.registerDocument(document);
    const io = { service: { readNote: vi.fn(async () => ({ ok: true as const, value: document })) },
      controller, canWrite: () => true, onSourceApplied: vi.fn() };
    const rewrite = vi.fn(async (text: string, offset: number) => ({ ok: true as const, value: {
      source: text.slice(0, offset) + text.slice(offset).replace('[ ]', '[x]'),
    } }));
    return { io, save, rewrite };
  }

  it.each([true, false])('writes exactly the second duplicate through SaveController (open=%s)', async open => {
    const { io, save, rewrite } = setup(open);
    const row = task({ text: 'same', offset: 11 });
    expect(await writeTaskSource(io, row, rewrite)).toEqual({ ok: true, value: { changed: true } });
    expect(save).toHaveBeenCalledExactlyOnceWith({ noteId: 'n1', source: '- [ ] same\n- [x] same\n',
      expectedRevision: 1, operationId: 'task-operation' });
    expect(io.controller.getBufferText('n1')).toBe('- [ ] same\n- [x] same\n');
    expect(io.onSourceApplied).toHaveBeenCalledOnce();
  });

  it.each(['revision', 'hash', 'readOnly', 'identity'] as const)('rejects changed disk %s even when text and offsets still match', async change => {
    const { io, save, rewrite } = setup(false);
    const changed = { ...document };
    if (change === 'revision') changed.localRevision = 2;
    if (change === 'hash') changed.contentHash = 'external-hash';
    if (change === 'readOnly') changed.readOnly = true;
    if (change === 'identity') changed.noteId = 'other-note';
    io.service.readNote.mockResolvedValue({ ok: true, value: changed });
    expect(await writeTaskSource(io, task({ text: 'same' }), rewrite)).toMatchObject({ ok: false, error: { code: 'VERSION_CONFLICT' } });
    expect(save).not.toHaveBeenCalled();
    expect(rewrite).not.toHaveBeenCalled();
  });

  it.each(['dirty', 'composition', 'readonly', 'workspace'] as const)('refuses %s before rewriting', async state => {
    const { io, save, rewrite } = setup();
    if (state === 'dirty') io.controller.setBufferText('n1', source + 'draft');
    if (state === 'composition') io.controller.compositionStarted('n1');
    if (state === 'workspace') io.canWrite = () => false;
    expect((await writeTaskSource(io, task({ text: 'same', readOnly: state === 'readonly' }), rewrite)).ok).toBe(false);
    expect(io.service.readNote).not.toHaveBeenCalled();
    expect(save).not.toHaveBeenCalled();
  });

  it.each(['draft', 'composition', 'workspace', 'registration'] as const)('discards late rewrite after %s changes', async change => {
    const { io, save } = setup(change !== 'registration');
    let finish!: (value: { ok: true; value: { source: string } }) => void;
    const pending = new Promise<{ ok: true; value: { source: string } }>(resolve => { finish = resolve; });
    const rewriting = vi.fn(() => pending);
    const result = writeTaskSource(io, task({ text: 'same' }), rewriting);
    await vi.waitFor(() => expect(rewriting).toHaveBeenCalledOnce());
    if (change === 'registration') io.controller.registerDocument(document);
    if (change === 'draft' || change === 'registration') io.controller.setBufferText('n1', source + 'new input');
    if (change === 'composition') io.controller.compositionStarted('n1');
    if (change === 'workspace') io.canWrite = () => false;
    finish({ ok: true, value: { source: '- [x] same\n' } });
    expect((await result).ok).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(io.onSourceApplied).not.toHaveBeenCalled();
    if (change === 'draft' || change === 'registration') expect(io.controller.getBufferText('n1')).toBe(source + 'new input');
  });

  it('retains a failed request and operation ID for normal retry, blocking a second task mutation', async () => {
    const { io, save, rewrite } = setup(false);
    save.mockResolvedValueOnce({ ok: false, error: failure } as never);
    const row = task({ text: 'same' });
    expect(await writeTaskSource(io, row, rewrite)).toEqual({ ok: false, error: failure });
    const failed = io.controller.snapshot('n1')?.failedOperation;
    expect(failed).toMatchObject({ source: '- [x] same\n- [ ] same\n', operationId: 'task-operation' });
    expect((await writeTaskSource(io, row, rewrite)).ok).toBe(false);
    expect(save).toHaveBeenCalledTimes(1);
    await io.controller.retryFailed('n1');
    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[1]).toEqual(save.mock.calls[0]);
  });

  it('propagates a pure rewrite failure without changing the buffer', async () => {
    const { io, save } = setup();
    expect(await writeTaskSource(io, task({ text: 'same' }), async () => ({ ok: false, error: failure }))).toEqual({ ok: false, error: failure });
    expect(io.controller.getBufferText('n1')).toBe(source);
    expect(save).not.toHaveBeenCalled();
  });
});

