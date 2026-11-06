import { describe, expect, it, vi } from 'vitest';
import type { NoteSummary } from '../domain';
import type { ServiceOutcome } from '../editor';
import { droppedPath, notesUnder, planFolderMove, runSteps } from './folder-ops';

const notes = ['a/x.md', 'a/b/y.md', 'ab.md', 'c/z.md'].map((relativePath, i) => ({ noteId: `n${i}`, relativePath } as NoteSummary));
const folders = ['a', 'a/b', 'a/empty', 'c'];

describe('folder move plan', () => {
  it('moves every note and subfolder, empty ones included, but not a shared prefix', () => {
    const plan = planFolderMove(notes, folders, 'a', 'c/a2');
    expect(plan).toEqual({
      ok: true, from: 'a', to: 'c/a2',
      moves: [{ noteId: 'n1', from: 'a/b/y.md', to: 'c/a2/b/y.md' }, { noteId: 'n0', from: 'a/x.md', to: 'c/a2/x.md' }],
      folders: ['c/a2', 'c/a2/b', 'c/a2/empty'],
    });
  });
  it('refuses the same place, its own subfolder, a taken name and a case-only rename with notes', () => {
    expect(planFolderMove(notes, folders, 'a', 'a')).toEqual({ ok: false, reason: 'same' });
    expect(planFolderMove(notes, folders, 'a', 'a/b/a')).toEqual({ ok: false, reason: 'intoItself' });
    expect(planFolderMove(notes, folders, 'a', 'C')).toEqual({ ok: false, reason: 'exists' });
    expect(planFolderMove(notes, folders, 'c', 'ab.md')).toEqual({ ok: false, reason: 'exists' });
    expect(planFolderMove(notes, folders, 'a', 'A')).toEqual({ ok: false, reason: 'caseOnly' });
    expect(planFolderMove(notes, folders, 'a/empty', 'a/Empty')).toMatchObject({ ok: true, moves: [] });
  });
  it('lists the notes a delete takes and where a drop lands', () => {
    expect(notesUnder(notes, 'a').map((note) => note.relativePath)).toEqual(['a/b/y.md', 'a/x.md']);
    expect(droppedPath('a/b', '')).toBe('b');
    expect(droppedPath('ab.md', 'c')).toBe('c/ab.md');
  });
});

describe('run steps', () => {
  const ok: ServiceOutcome<unknown> = { ok: true, value: null };
  const failed: ServiceOutcome<unknown> = { ok: false, error: { code: 'IO_DENIED', messageKey: 'x', params: {}, retryable: true, operationId: null, retryAfterSeconds: null } };

  it('stops at the first failure and resumes from it', async () => {
    const second = vi.fn().mockResolvedValueOnce(failed).mockResolvedValueOnce(ok);
    const steps = [{ run: vi.fn(async () => ok) }, { run: second }, { run: vi.fn(async () => ok) }];
    const progress = vi.fn();
    const first = await runSteps(steps, 0, progress);
    expect(first).toEqual({ done: 1, error: failed.ok ? null : failed.error });
    expect(steps[2]!.run).not.toHaveBeenCalled();
    const resumed = await runSteps(steps, first.done, progress);
    expect(resumed).toEqual({ done: 3, error: null });
    expect(steps[0]!.run).toHaveBeenCalledTimes(1);
    expect(progress).toHaveBeenLastCalledWith(3, 3);
  });
  it('turns a thrown step into a failure', async () => {
    const result = await runSteps([{ run: async () => { throw new Error('boom'); } }], 0, () => undefined);
    expect(result.done).toBe(0);
    expect(result.error?.code).toBe('IO_DENIED');
  });
});
