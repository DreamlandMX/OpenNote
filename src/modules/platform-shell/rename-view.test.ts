import { describe, expect, it } from 'vitest';
import type { RenameCommit, RenamePlan, RenameWarning } from '../editor';
import type { Translator } from '../presentation-i18n';
import {
  canCommit,
  commitSummary,
  expectedRevisions,
  folderOf,
  issueRow,
  issueRows,
  issueText,
  needsExtension,
  normalizeDestination,
  stepRows,
  warningKey,
  warningText,
  withExtension,
} from './rename-view';

const translator: Translator = ((key: string) => key) as Translator;

function commit(overrides: Partial<RenameCommit> = {}): RenameCommit {
  return {
    steps: [
      {
        noteId: 'n1',
        relativePath: 'a.md',
        source: '[[b]]\n',
        expectedRevision: 1,
        edits: 1,
      },
    ],
    moveFrom: 'b.md',
    moveTo: 'moved/b.md',
    keepsIdentity: true,
    issues: [],
    untouched: [],
    ...overrides,
  };
}

describe('preview rows', () => {
  it('lists the notes whose links change', () => {
    expect(stepRows(commit())).toEqual([{ noteId: 'n1', relativePath: 'a.md', edits: 1 }]);
  });

  it('maps each issue to a key and parameters', () => {
    expect(
      issueRow({ kind: 'revisionChanged', noteId: 'n1', expected: 1, actual: 2 }),
    ).toEqual({
      key: 'rename.issue.revisionChanged',
      params: { expected: 1, actual: 2 },
    });
    expect(issueRow({ kind: 'targetMissing', noteId: 'n1' }).key).toBe(
      'rename.issue.targetMissing',
    );
    expect(issueRow({ kind: 'destinationOccupied', path: 'a.md' }).key).toBe(
      'rename.issue.destinationOccupied',
    );
    expect(issueRow({ kind: 'nestedDestination', path: 'b.md', destination: 'b.md/x' }).key).toBe(
      'rename.issue.nestedDestination',
    );
  });

  it('keeps every issue visible', () => {
    const rows = issueRows(
      commit({
        issues: [
          { kind: 'destinationOccupied', path: 'a.md' },
          { kind: 'targetMissing', noteId: 'n9' },
        ],
      }),
    );
    expect(rows).toHaveLength(2);
  });

  it('resolves an issue to a sentence', () => {
    expect(issueText({ key: 'rename.issue.targetMissing', params: {} }, translator)).toBe(
      'rename.issue.targetMissing',
    );
  });

  it('distinguishes an ambiguous link from a broken one', () => {
    const ambiguous: RenameWarning = {
      noteId: 'n1',
      relativePath: 'a.md',
      offset: 0,
      target: 'x',
      reason: 'ambiguousTarget',
    };
    const broken: RenameWarning = { ...ambiguous, reason: 'brokenLink' };
    expect(warningKey(ambiguous)).toBe('rename.warning.ambiguous');
    expect(warningKey(broken)).toBe('rename.warning.broken');
    expect(warningText(broken, translator)).toBe('rename.warning.broken');
  });
});

describe('applicability', () => {
  it('commits only when there are no issues', () => {
    expect(canCommit(commit())).toBe(true);
    expect(canCommit(commit({ issues: [{ kind: 'targetMissing', noteId: 'n1' }] }))).toBe(false);
    expect(canCommit(null)).toBe(false);
  });

  it('summarises the blast radius and identity', () => {
    expect(commitSummary(commit())).toEqual({
      notes: 1,
      edits: 1,
      warnings: 0,
      keepsIdentity: true,
    });
  });
});

describe('destination handling', () => {
  it('normalises separators and drops a leading slash', () => {
    expect(normalizeDestination('  a/b.md  ')).toBe('a/b.md');
    expect(normalizeDestination('a\\b.md')).toBe('a/b.md');
    expect(normalizeDestination('/a//b.md')).toBe('a/b.md');
    expect(normalizeDestination('./a/./b.md')).toBe('a/b.md');
  });

  it('lets a safe parent segment return toward the root', () => {
    expect(normalizeDestination('a/../b.md')).toBe('b.md');
  });

  it('refuses a destination that escapes the vault', () => {
    // An empty result signals an unusable path rather than silently clamping it.
    expect(normalizeDestination('../b.md')).toBe('');
    expect(normalizeDestination('a/../../b.md')).toBe('');
  });

  it('catches a missing .md extension', () => {
    expect(needsExtension('notes/plan')).toBe(true);
    expect(needsExtension('notes/plan.md')).toBe(false);
    expect(needsExtension('notes/PLAN.MD')).toBe(false);
    expect(withExtension('notes/plan')).toBe('notes/plan.md');
    expect(withExtension('notes/plan.md')).toBe('notes/plan.md');
  });

  it('reports the folder a note would land in', () => {
    expect(folderOf('moved/b.md')).toBe('moved');
    expect(folderOf('b.md')).toBe('');
  });
});

describe('revision snapshot', () => {
  it('sends only the notes the plan touches', () => {
    const plan: RenamePlan = {
      planId: 'plan', vaultId: 'vault', expectedHashes: [],
      noteId: 'n2',
      fromPath: 'b.md',
      toPath: 'moved/b.md',
      keepsIdentity: true,
      edits: [
        { noteId: 'n1', relativePath: 'a.md', offset: 0, length: 4, before: '[[b]]', after: '[[moved/b]]' },
      ],
      untouched: [],
    };
    const revisions = [
      { noteId: 'n1', revision: 3 },
      { noteId: 'n9', revision: 7 },
    ];
    expect(expectedRevisions(plan, revisions)).toEqual([{ noteId: 'n1', revision: 3 }]);
  });
});
