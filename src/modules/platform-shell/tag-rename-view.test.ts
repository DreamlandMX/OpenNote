import { describe, expect, it } from 'vitest';
import type { TagRenamePlan } from '../editor';
import type { Translator } from '../presentation-i18n';
import {
  actionKey,
  canApply,
  changeDescription,
  conflictRow,
  conflictRows,
  conflictText,
  isChange,
  notesForPlan,
  planSummary,
  rewriteRows,
} from './tag-rename-view';

const translator: Translator = ((key: string) => key) as Translator;

function plan(overrides: Partial<TagRenamePlan> = {}): TagRenamePlan {
  return {
    oldTag: 'old',
    newTag: 'new',
    isMerge: false,
    rewrites: [
      { noteId: 'n1', relativePath: 'a.md', source: '#new\n', changed: 1, expectedRevision: 1 },
    ],
    conflicted: [],
    totalChanges: 1,
    ...overrides,
  };
}

describe('preview rows', () => {
  it('lists one row per rewritten note with its change count', () => {
    const rows = rewriteRows(
      plan({
        rewrites: [
          { noteId: 'n1', relativePath: 'a.md', source: '', changed: 2, expectedRevision: 1 },
          { noteId: 'n2', relativePath: 'b.md', source: '', changed: 1, expectedRevision: 1 },
        ],
      }),
    );
    expect(rows).toEqual([
      { noteId: 'n1', relativePath: 'a.md', changed: 2 },
      { noteId: 'n2', relativePath: 'b.md', changed: 1 },
    ]);
  });

  it('maps each conflict kind to a message key and parameters', () => {
    expect(
      conflictRow({
        noteId: 'n1',
        issue: { kind: 'revisionChanged', noteId: 'n1', expected: 1, actual: 2 },
      }),
    ).toEqual({
      noteId: 'n1',
      key: 'tags.conflict.revisionChanged',
      params: { expected: 1, actual: 2 },
    });
    expect(
      conflictRow({ noteId: 'n2', issue: { kind: 'revisionUnknown', noteId: 'n2' } }).key,
    ).toBe('tags.conflict.revisionUnknown');
    expect(
      conflictRow({ noteId: '', issue: { kind: 'invalidTag', reason: 'error.tagEmpty' } }).key,
    ).toBe('tags.conflict.invalidTag');
    expect(conflictRow({ noteId: '', issue: { kind: 'noChange' } }).key).toBe(
      'tags.conflict.noChange',
    );
  });

  it('keeps every conflict visible rather than collapsing them', () => {
    // F08-R04: a conflict is reported next to the rest, not summarised away.
    const rows = conflictRows(
      plan({
        conflicted: [
          { noteId: 'n1', issue: { kind: 'revisionChanged', noteId: 'n1', expected: 1, actual: 2 } },
          { noteId: 'n2', issue: { kind: 'revisionUnknown', noteId: 'n2' } },
        ],
      }),
    );
    expect(rows).toHaveLength(2);
    expect(rows[1]?.key).toBe('tags.conflict.revisionUnknown');
  });

  it('resolves a conflict to a sentence', () => {
    const rows = conflictRows(
      plan({ conflicted: [{ noteId: 'n1', issue: { kind: 'noChange' } }] }),
    );
    expect(conflictText(rows[0]!, translator)).toBe('tags.conflict.noChange');
  });
});

describe('applicability', () => {
  it('applies only with changes and no conflicts', () => {
    expect(canApply(plan())).toBe(true);
  });

  it('allows unaffected notes while keeping per-note conflicts visible', () => {
    // The result lists partial success explicitly (F08-R04).
    expect(
      canApply(
        plan({ conflicted: [{ noteId: 'n2', issue: { kind: 'revisionUnknown', noteId: 'n2' } }] }),
      ),
    ).toBe(true);
  });

  it('refuses an empty plan and a null plan', () => {
    expect(canApply(plan({ rewrites: [], totalChanges: 0 }))).toBe(false);
    expect(canApply(plan({ totalChanges: 0 }))).toBe(false);
    expect(canApply(null)).toBe(false);
    expect(canApply(plan({ conflicted: [{ noteId: '', issue: { kind: 'invalidTag', reason: 'invalid' } }] }))).toBe(false);
  });
});

describe('labels', () => {
  it('names a merge as a merge', () => {
    expect(actionKey(plan())).toBe('tags.applyRename');
    expect(actionKey(plan({ isMerge: true }))).toBe('tags.applyMerge');
    expect(actionKey(plan({ newTag: '' }))).toBe('tags.applyRemoval');
    expect(actionKey(null)).toBe('tags.applyRename');
  });

  it('summarises notes, changes, conflicts and merge', () => {
    const duplicated = plan({ isMerge: true, totalChanges: 3 });
    expect(planSummary(duplicated)).toEqual({ notes: 1, changes: 3, conflicts: 0, merge: true });
  });

  it('describes a per-note change count', () => {
    expect(changeDescription(2, translator)).toBe('tags.changes');
  });

  it('treats a trimmed identical or empty target as no change', () => {
    expect(isChange('old', 'old')).toBe(false);
    expect(isChange('old', '  old  ')).toBe(false);
    expect(isChange('old', '   ')).toBe(false);
    expect(isChange('old', 'new')).toBe(true);
  });
});

describe('plan request', () => {
  it('sends only the notes that report the tag', () => {
    const sources = [
      { noteId: 'n1', relativePath: 'a.md', source: '#old\n', revision: 1 },
      { noteId: 'n2', relativePath: 'b.md', source: 'nothing\n', revision: 1 },
    ];
    expect(notesForPlan(['n1'], sources).map((note) => note.noteId)).toEqual(['n1']);
    expect(notesForPlan([], sources)).toEqual([]);
  });
});
