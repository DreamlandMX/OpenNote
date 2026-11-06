import { describe, expect, it, vi } from 'vitest';
import type { NoteSummary, ServiceError } from '../domain';
import type { RenameEdit, RenamePlan, RenameResult, ServiceOutcome } from '../editor';
import { batchDeleteSteps, deleteSteps, linkSummary, moveSteps, type FolderActionsIo } from './folder-actions';
import { planBatchDelete, planFolderMove, runSteps } from './folder-ops';

const error = (code: ServiceError['code']): ServiceError =>
  ({ code, messageKey: code, params: {}, retryable: true, operationId: null, retryAfterSeconds: null });

function plan(noteId: string, fromPath: string, toPath: string, edits: Partial<RenameEdit>[] = [], untouched = 0): RenamePlan {
  return {
    planId: `plan-${noteId}`, vaultId: 'v', noteId, fromPath, toPath, keepsIdentity: true,
    expectedHashes: [{ noteId, relativePath: fromPath, expectedRevision: 3, contentHash: 'h' }],
    edits: edits as RenameEdit[],
    untouched: Array.from({ length: untouched }, () => ({}) as RenamePlan['untouched'][number]),
  };
}

/** A vault of note paths; renames and trashing change it, so retries see what already happened. */
function fakeVault(paths: Record<string, string>) {
  const log: string[] = [];
  let ids = 0;
  const io = {
    notePath: vi.fn(async (noteId: string) => paths[noteId] ?? null),
    previewNoteRename: vi.fn(async (noteId: string, toPath: string): Promise<ServiceOutcome<RenamePlan>> => {
      log.push(`preview ${noteId}`);
      return { ok: true, value: plan(noteId, paths[noteId]!, toPath) };
    }),
    applyRename: vi.fn(async (renamePlan: RenamePlan, _revision: number, operationId: string): Promise<ServiceOutcome<RenameResult>> => {
      log.push(`rename ${renamePlan.noteId} ${operationId}`);
      paths[renamePlan.noteId] = renamePlan.toPath;
      return { ok: true, value: { commitId: 'c', completed: true, items: [] } };
    }),
    createFolder: vi.fn(async (path: string): Promise<ServiceOutcome<string>> => {
      log.push(`mkdir ${path}`);
      return { ok: true, value: path };
    }),
    removeEmptyFolder: vi.fn(async (path: string): Promise<ServiceOutcome<null>> => {
      log.push(`rmdir ${path}`);
      return { ok: true, value: null };
    }),
    recycleFolder: vi.fn(async (path: string): Promise<ServiceOutcome<null>> => {
      log.push(`recycle ${path}`);
      return { ok: true, value: null };
    }),
    renameEmptyFolder: vi.fn(async (from: string, to: string): Promise<ServiceOutcome<string>> => {
      log.push(`mvdir ${from} ${to}`);
      return { ok: true, value: to };
    }),
    trashNote: vi.fn(async (note: NoteSummary, operationId: string): Promise<ServiceOutcome<unknown>> => {
      log.push(`trash ${note.noteId} ${operationId}`);
      delete paths[note.noteId];
      return { ok: true, value: null };
    }),
    afterTrash: vi.fn(async (noteId: string) => { log.push(`closed ${noteId}`); }),
    newOperationId: () => `op${++ids}`,
  } satisfies FolderActionsIo;
  return { io, log, paths };
}

const notes = (paths: Record<string, string>) =>
  Object.entries(paths).map(([noteId, relativePath]) => ({ noteId, relativePath, localRevision: 1, contentHash: 'h' } as NoteSummary));

describe('folder move steps', () => {
  it('moves each note, planning when it runs, then recreates subfolders and removes the old folder', async () => {
    const { io, log, paths } = fakeVault({ a: 'old/a.md', b: 'old/sub/b.md' });
    const folderPlan = planFolderMove(notes(paths), ['old', 'old/sub', 'old/empty'], 'old', 'new');
    if (!folderPlan.ok) throw new Error('plan refused');
    const steps = moveSteps(folderPlan, io);
    // Building the steps plans nothing yet.
    expect(io.previewNoteRename).not.toHaveBeenCalled();
    expect(await runSteps(steps, 0, () => undefined)).toEqual({ done: steps.length, error: null });
    expect(log).toEqual([
      'preview a', 'rename a op1', 'preview b', 'rename b op2',
      'mkdir new', 'mkdir new/empty', 'mkdir new/sub', 'rmdir old',
    ]);
    expect(paths).toEqual({ a: 'new/a.md', b: 'new/sub/b.md' });
  });

  it('resumes where it stopped: a note already moved is skipped, and the failed one keeps its operation id', async () => {
    const { io, log, paths } = fakeVault({ a: 'old/a.md', b: 'old/b.md' });
    io.applyRename.mockImplementationOnce(async (renamePlan, _revision, operationId) => {
      // The rename lands but the reply is lost.
      paths[renamePlan.noteId] = renamePlan.toPath;
      log.push(`rename ${renamePlan.noteId} ${operationId} (reply lost)`);
      return { ok: false, error: error('IO_DENIED') };
    });
    const folderPlan = planFolderMove(notes(paths), ['old'], 'old', 'new');
    if (!folderPlan.ok) throw new Error('plan refused');
    const steps = moveSteps(folderPlan, io);
    const first = await runSteps(steps, 0, () => undefined);
    expect(first.done).toBe(0);
    const resumed = await runSteps(steps, first.done, () => undefined);
    expect(resumed.error).toBeNull();
    // `a` was already in place, so it was neither planned nor renamed again.
    expect(log.filter((line) => line.includes(' a'))).toEqual(['preview a', 'rename a op1 (reply lost)']);
    expect(paths).toEqual({ a: 'new/a.md', b: 'new/b.md' });
  });

  it('treats a destination folder that already exists as done, and stops on an incomplete rename', async () => {
    const { io } = fakeVault({ a: 'old/a.md' });
    io.createFolder.mockResolvedValue({ ok: false, error: error('PATH_COLLISION') });
    const folderPlan = planFolderMove(notes({ a: 'old/a.md' }), ['old'], 'old', 'new');
    if (!folderPlan.ok) throw new Error('plan refused');
    expect((await runSteps(moveSteps(folderPlan, io), 0, () => undefined)).error).toBeNull();

    const stuck = fakeVault({ a: 'old/a.md' });
    stuck.io.applyRename.mockResolvedValueOnce({ ok: true, value: { commitId: 'c', completed: false, items: [] } });
    const result = await runSteps(moveSteps(folderPlan, stuck.io), 0, () => undefined);
    expect(result).toMatchObject({ done: 0, error: { code: 'IO_DENIED', messageKey: 'folderOp.incomplete' } });
    expect(stuck.io.removeEmptyFolder).not.toHaveBeenCalled();
  });

  it('renames a folder without notes in one step', async () => {
    const { io, log } = fakeVault({});
    const folderPlan = planFolderMove([], ['empty'], 'empty', 'renamed');
    if (!folderPlan.ok) throw new Error('plan refused');
    await runSteps(moveSteps(folderPlan, io), 0, () => undefined);
    expect(log).toEqual(['mvdir empty renamed']);
  });
});

describe('folder delete steps', () => {
  it('trashes each note and closes it, then removes the empty folder; a retry replays the same operation', async () => {
    const { io, log, paths } = fakeVault({ a: 'gone/a.md', b: 'gone/b.md' });
    io.trashNote.mockImplementationOnce(async (note, operationId) => {
      log.push(`trash ${note.noteId} ${operationId} (failed)`);
      return { ok: false, error: error('FILE_LOCKED') };
    });
    const steps = deleteSteps('gone', notes(paths), io);
    const first = await runSteps(steps, 0, () => undefined);
    expect(first.done).toBe(0);
    await runSteps(steps, first.done, () => undefined);
    expect(log).toEqual([
      'trash a op1 (failed)', 'closed a',
      'trash a op1', 'closed a', 'trash b op2', 'closed b', 'rmdir gone',
    ]);
    expect(paths).toEqual({});
  });
});

describe('link summary', () => {
  it('counts rewritten links in other notes, not in notes that move together', async () => {
    const io = {
      previewNoteRename: vi.fn(async (noteId: string, toPath: string): Promise<ServiceOutcome<RenamePlan>> => ({
        ok: true,
        value: plan(noteId, 'x', toPath, [
          { noteId: 'other', relativePath: 'other.md' },
          { noteId: noteId === 'a' ? 'b' : 'a', relativePath: 'moving.md' },
        ], noteId === 'a' ? 1 : 0),
      })),
    };
    const moves = [{ noteId: 'a', from: 'o/a.md', to: 'n/a.md' }, { noteId: 'b', from: 'o/b.md', to: 'n/b.md' }];
    expect(await linkSummary(moves, io)).toEqual({ links: 4, notes: 1, untouched: 1 });
  });
});

describe('deleting a selection', () => {
  const vault = { a: 'docs/a.md', b: 'docs/sub/b.md', c: 'loose.md', d: 'thesis/notes.md', e: 'other/e.md' };

  it('trashes each note once and removes only the outermost selected folders', () => {
    const plan = planBatchDelete([
      { kind: 'folder', path: 'docs' }, { kind: 'folder', path: 'docs/sub' },
      { kind: 'note', noteId: 'b' }, { kind: 'note', noteId: 'c' }, { kind: 'folder', path: '' },
    ], notes(vault), new Map());
    expect(plan.notes.map((note) => note.noteId)).toEqual(['a', 'b', 'c']);
    expect(plan.folders).toEqual(['docs']);
    expect(plan.recycle).toEqual([]);
    expect(plan.skipped).toEqual([]);
  });

  it('sends a folder of other files to the recycle bin after trashing its notes', () => {
    const plan = planBatchDelete([{ kind: 'folder', path: 'thesis' }, { kind: 'folder', path: 'other' }],
      notes(vault), new Map([['thesis', 'files' as const]]));
    expect(plan.recycle).toEqual(['thesis']);
    expect(plan.folders).toEqual(['other']);
    expect(plan.notes.map((note) => note.noteId)).toEqual(['e', 'd']);
  });

  it('skips a folder holding a link, but still trashes notes picked inside it', () => {
    const plan = planBatchDelete([{ kind: 'folder', path: 'thesis' }, { kind: 'note', noteId: 'd' }],
      notes(vault), new Map([['thesis', 'link' as const]]));
    expect(plan.skipped).toEqual(['thesis']);
    expect(plan.recycle).toEqual([]);
    expect(plan.notes.map((note) => note.noteId)).toEqual(['d']);
  });

  it('runs the notes first, then the recycle bin, then the empty folders, deepest first', async () => {
    const { io, log } = fakeVault({ a: 'x/a.md', b: 'y/z/b.md', c: 'p/c.md' });
    const plan = { notes: notes({ a: 'x/a.md', b: 'y/z/b.md', c: 'p/c.md' }), folders: ['y/z', 'x'], recycle: ['p'], skipped: [] };
    expect((await runSteps(batchDeleteSteps(plan, io), 0, () => undefined)).error).toBeNull();
    expect(log).toEqual(['trash a op1', 'closed a', 'trash b op2', 'closed b', 'trash c op3', 'closed c', 'recycle p', 'rmdir y/z', 'rmdir x']);
  });

  it('counts a folder already gone from the recycle step as done, and stops on a real failure', async () => {
    const { io } = fakeVault({});
    io.recycleFolder.mockResolvedValueOnce({ ok: false, error: error('OBJECT_NOT_FOUND') });
    const plan = { notes: [], folders: [], recycle: ['p'], skipped: [] };
    expect((await runSteps(batchDeleteSteps(plan, io), 0, () => undefined)).error).toBeNull();
    io.recycleFolder.mockResolvedValueOnce({ ok: false, error: error('IO_DENIED') });
    expect((await runSteps(batchDeleteSteps(plan, io), 0, () => undefined)).error?.code).toBe('IO_DENIED');
  });
});
