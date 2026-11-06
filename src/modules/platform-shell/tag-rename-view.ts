import type { TagPlanIssue, TagRenamePlan } from '../editor';
import type { Translator } from '../presentation-i18n';

/**
 * S15 tag rename preview.
 *
 * A tag rename is a batch mutation across notes, so the panel's job is to show the
 * blast radius **before** anything is written and to be honest about what it cannot
 * do. Three rules drive the code here:
 *
 * * a conflicting note is listed next to the notes that would change, never folded
 *   into a single "done" (F08-R04);
 * * per-note conflicts remain visible while unaffected notes can proceed;
 * * a merge is named as a merge, because "renamed" and "two tags became one" are
 *   different facts (F08-A03).
 */

export interface RewriteRow {
  noteId: string;
  relativePath: string;
  changed: number;
}

export interface ConflictRow {
  noteId: string;
  key:
    | 'tags.conflict.sourceUnsupported'
    | 'tags.conflict.revisionChanged'
    | 'tags.conflict.revisionUnknown'
    | 'tags.conflict.invalidTag'
    | 'tags.conflict.noChange';
  params: Record<string, string | number>;
}

/** The rows a preview lists. */
export function rewriteRows(plan: TagRenamePlan): RewriteRow[] {
  return plan.rewrites.map((rewrite) => ({
    noteId: rewrite.noteId,
    relativePath: rewrite.relativePath,
    changed: rewrite.changed,
  }));
}

/** Maps a plan issue to a message key and its parameters. */
export function conflictRow(conflict: { noteId: string; issue: TagPlanIssue }): ConflictRow {
  const issue = conflict.issue;
  switch (issue.kind) {
    case 'sourceUnsupported':
      return { noteId: conflict.noteId, key: 'tags.conflict.sourceUnsupported', params: {} };
    case 'revisionChanged':
      return {
        noteId: conflict.noteId,
        key: 'tags.conflict.revisionChanged',
        params: { expected: issue.expected, actual: issue.actual },
      };
    case 'revisionUnknown':
      return {
        noteId: conflict.noteId,
        key: 'tags.conflict.revisionUnknown',
        params: {},
      };
    case 'invalidTag':
      return {
        noteId: conflict.noteId,
        key: 'tags.conflict.invalidTag',
        params: { reason: issue.reason },
      };
    case 'noChange':
    default:
      return { noteId: conflict.noteId, key: 'tags.conflict.noChange', params: {} };
  }
}

export function conflictRows(plan: TagRenamePlan): ConflictRow[] {
  return plan.conflicted.map(conflictRow);
}

/** Resolves a conflict to a sentence. */
export function conflictText(row: ConflictRow, translator: Translator): string {
  return translator(row.key, row.params);
}

/**
 * Whether the plan may be applied.
 *
 * Invalid targets block the operation; per-note conflicts do not block other notes.
 */
export function canApply(plan: TagRenamePlan | null): boolean {
  return (
    plan !== null &&
    !plan.conflicted.some(conflict => conflict.issue.kind === 'invalidTag' || conflict.issue.kind === 'noChange') &&
    plan.rewrites.length > 0 &&
    plan.totalChanges > 0
  );
}

/** The headline for a plan: how many notes and occurrences would change. */
export function planSummary(plan: TagRenamePlan): {
  notes: number;
  changes: number;
  conflicts: number;
  merge: boolean;
} {
  return {
    notes: plan.rewrites.length,
    changes: plan.totalChanges,
    conflicts: plan.conflicted.length,
    merge: plan.isMerge,
  };
}

/** The label the action button carries. */
export function actionKey(plan: TagRenamePlan | null): 'tags.applyRename' | 'tags.applyMerge' | 'tags.applyRemoval' {
  return plan?.newTag === '' ? 'tags.applyRemoval' : plan?.isMerge ? 'tags.applyMerge' : 'tags.applyRename';
}

/** Whether a proposed new tag differs from the old one once trimmed. */
export function isChange(oldTag: string, newTag: string): boolean {
  return oldTag.trim() !== newTag.trim() && newTag.trim() !== '';
}

/**
 * Builds the note snapshots a plan request needs.
 *
 * Only notes that already report the tag are sent, so the request stays
 * proportional to the tag's usage rather than to the vault size.
 */
export function notesForPlan(
  noteIds: readonly string[],
  sources: readonly { noteId: string; relativePath: string; source: string; revision: number }[],
): { noteId: string; relativePath: string; source: string; revision: number }[] {
  return sources.filter((source) => noteIds.includes(source.noteId));
}

/** A per-note change description for the reader. */
export function changeDescription(changed: number, translator: Translator): string {
  return translator('tags.changes', { count: changed });
}
