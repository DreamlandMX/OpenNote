import type { CommitIssue, RenameCommit, RenamePlan, RenameWarning } from '../editor';
import type { Translator } from '../presentation-i18n';

/**
 * S13 rename preview.
 *
 * Renaming a note is a batch mutation: the note's own path changes and every note
 * that links to it may be rewritten. This module decides what the reader is told
 * before that happens, and it refuses to soften three facts:
 *
 * * a note that changed since the plan was made invalidates the plan
 *   (VERSION_CONFLICT), so the commit is not applicable;
 * * an occupied destination is a collision, not an overwrite
 *   (PATH_COLLISION);
 * * a link the plan deliberately left alone is listed, because a rename that
 *   quietly repaired a broken link would be claiming knowledge it does not have.
 */

export interface StepRow {
  noteId: string;
  relativePath: string;
  edits: number;
}

export interface IssueRow {
  key:
    | 'rename.issue.revisionChanged'
    | 'rename.issue.targetMissing'
    | 'rename.issue.destinationOccupied'
    | 'rename.issue.nestedDestination';
  params: Record<string, string | number>;
}

/** The notes whose links would be rewritten. */
export function stepRows(commit: RenameCommit): StepRow[] {
  return commit.steps.map((step) => ({
    noteId: step.noteId,
    relativePath: step.relativePath,
    edits: step.edits,
  }));
}

/** Maps a commit issue to a message key. */
export function issueRow(issue: CommitIssue): IssueRow {
  switch (issue.kind) {
    case 'revisionChanged':
      return {
        key: 'rename.issue.revisionChanged',
        params: { expected: issue.expected, actual: issue.actual },
      };
    case 'targetMissing':
      return { key: 'rename.issue.targetMissing', params: {} };
    case 'destinationOccupied':
      return { key: 'rename.issue.destinationOccupied', params: { path: issue.path } };
    case 'nestedDestination':
    default:
      return { key: 'rename.issue.nestedDestination', params: { path: issue.path } };
  }
}

export function issueRows(commit: RenameCommit): IssueRow[] {
  return commit.issues.map(issueRow);
}

/** Resolves an issue to a sentence. */
export function issueText(row: IssueRow, translator: Translator): string {
  return translator(row.key, row.params);
}

/** The reason a link was left alone. */
export function warningKey(warning: RenameWarning): 'rename.warning.ambiguous' | 'rename.warning.broken' {
  return warning.reason === 'ambiguousTarget' ? 'rename.warning.ambiguous' : 'rename.warning.broken';
}

/** A warning as a readable sentence. */
export function warningText(warning: RenameWarning, translator: Translator): string {
  return translator(warningKey(warning), {
    path: warning.relativePath,
    target: warning.target,
  });
}

/**
 * Whether the commit may run.
 *
 * A single issue stops the whole commit: a rename is one operation even though it
 * touches several notes, so writing the notes whose links could be updated and
 * leaving the moved note in place would be an inconsistent state presented as
 * success.
 */
export function canCommit(commit: RenameCommit | null): boolean {
  return commit !== null && commit.issues.length === 0;
}

/** A one-line summary of the blast radius. */
export function commitSummary(commit: RenameCommit): {
  notes: number;
  edits: number;
  warnings: number;
  keepsIdentity: boolean;
} {
  return {
    notes: commit.steps.length,
    edits: commit.steps.reduce((total, step) => total + step.edits, 0),
    warnings: commit.untouched.length,
    keepsIdentity: commit.keepsIdentity,
  };
}

/** The revision snapshot a commit request needs, from what the plan reported. */
export function expectedRevisions(
  plan: RenamePlan,
  revisions: readonly { noteId: string; revision: number }[],
): { noteId: string; revision: number }[] {
  const ids = new Set(plan.edits.map((edit) => edit.noteId));
  return revisions.filter((entry) => ids.has(entry.noteId));
}

/**
 * A cleaned destination path.
 *
 * The user may type separators or a leading slash; the plan works in
 * vault-relative form, so the same normalisation is applied here rather than
 * letting the host reject an avoidable mistake.
 */
export function normalizeDestination(input: string): string {
  const normalized = input.trim().replace(/\\/g, '/');
  const segments: string[] = [];
  for (const segment of normalized.split('/')) {
    if (segment === '' || segment === '.') {
      continue;
    }
    if (segment === '..') {
      // A `..` that climbs above the root cannot be expressed as a vault path.
      if (segments.length === 0) {
        return '';
      }
      segments.pop();
      continue;
    }
    segments.push(segment);
  }
  return segments.join('/');
}

/**
 * Whether a destination needs a file extension.
 *
 * A note path must end in `.md`; typing a bare name is a mistake worth catching
 * before the host reports a generic failure.
 */
export function needsExtension(destination: string): boolean {
  const name = destination.split('/').pop() ?? '';
  return name !== '' && !name.toLowerCase().endsWith('.md');
}

/** Appends `.md` when the destination lacks it. */
export function withExtension(destination: string): string {
  return needsExtension(destination) ? `${destination}.md` : destination;
}

/** The folder part of a destination, or an empty string at the root. */
export function folderOf(destination: string): string {
  const parts = destination.split('/');
  return parts.length > 1 ? parts.slice(0, -1).join('/') : '';
}
