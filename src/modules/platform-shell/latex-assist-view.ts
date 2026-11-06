import type { AssistEdit, AssistRange, AssistResult, CompletionCandidate, LanguageMode } from '../editor';

/**
 * S72 LaTeX source assistance presentation.
 *
 * The edits are computed in M13; this module only decides what the editor is told.
 * The rule it owns: an action that cannot apply cleanly reports why instead of
 * applying something approximate, and every action is one edit over one explicit
 * range so it becomes one undo step (F47-R02).
 */

export interface EditPreview {
  before: string;
  after: string;
  selection: AssistRange;
  kind: string;
}

/** Renders the effect of a result so the user can see it before it applies. */
export function previewEdit(source: string, result: AssistResult): EditPreview[] {
  return result.edits.map((edit) => ({
    before: source.slice(edit.range.start, edit.range.end),
    after: edit.text,
    selection: result.selection,
    kind: edit.kind,
  }));
}

/** Applies edits back to front, mirroring what the editor will do. */
export function applyResult(source: string, result: AssistResult): string {
  const ordered = [...result.edits].sort((left, right) => right.range.start - left.range.start);
  let text = source;
  for (const edit of ordered) {
    text = text.slice(0, edit.range.start) + edit.text + text.slice(edit.range.end);
  }
  return text;
}

/** True when the range is inside the text and splits no character boundary. */
export function isValidRange(text: string, range: AssistRange): boolean {
  if (range.start < 0 || range.end < range.start || range.end > text.length) {
    return false;
  }
  // A UTF-16 surrogate pair must not be split.
  const isLowSurrogate = (index: number): boolean => {
    const code = text.charCodeAt(index);
    return code >= 0xdc00 && code <= 0xdfff;
  };
  return !isLowSurrogate(range.start) && !isLowSurrogate(range.end);
}

/**
 * Whether an assist action should be blocked right now.
 *
 * A composition owns the pending text; pairing a bracket or wrapping a selection
 * mid-composition is what swallows a half-typed character (F47-R03).
 */
export function isBlockedByComposition(composing: boolean): boolean {
  return composing;
}

/** The label for the detected language, so the editor can show what it assumed. */
export function languageLabel(mode: LanguageMode): 'TeX' | 'BibTeX' | 'Plain text' {
  if (mode === 'tex') {
    return 'TeX';
  }
  if (mode === 'bib') {
    return 'BibTeX';
  }
  return 'Plain text';
}

/**
 * Filters a completion list by an exact-label match, so a re-preview after the
 * buffer changed cannot accept the wrong candidate.
 */
export function findCandidate(
  candidates: readonly CompletionCandidate[],
  label: string,
): CompletionCandidate | null {
  return candidates.find((candidate) => candidate.label === label) ?? null;
}

/**
 * The text a completion replaces, given the caret and the prefix it typed.
 *
 * The range covers only the typed prefix, so accepting a completion never
 * replaces text the user wrote earlier in the line.
 */
export function completionRange(
  text: string,
  caret: number,
): { range: AssistRange; prefix: string } | null {
  if (caret < 0 || caret > text.length) {
    return null;
  }
  let start = caret;
  while (start > 0) {
    const character = text[start - 1];
    if (character === undefined || !/[A-Za-z\\]/.test(character)) {
      break;
    }
    start -= 1;
  }
  const prefix = text.slice(start, caret);
  if (!prefix.startsWith('\\') || prefix.length < 2) {
    return null;
  }
  return { range: { start, end: caret }, prefix };
}

/** A summary of what an action would do, for the status line. */
export function describeResult(result: AssistResult): { edits: number; kind: string } {
  return {
    edits: result.edits.length,
    kind: result.edits[0]?.kind ?? 'none',
  };
}

/** Groups candidates by where they came from, so the list is explainable. */
export function groupCandidates(
  candidates: readonly CompletionCandidate[],
): { source: 'lexicon' | 'project'; labels: string[] }[] {
  const lexicon: string[] = [];
  const project: string[] = [];
  for (const candidate of candidates) {
    if (candidate.source === 'project') {
      project.push(candidate.label);
    } else {
      lexicon.push(candidate.label);
    }
  }
  const groups: { source: 'lexicon' | 'project'; labels: string[] }[] = [];
  if (lexicon.length > 0) {
    groups.push({ source: 'lexicon', labels: lexicon });
  }
  if (project.length > 0) {
    groups.push({ source: 'project', labels: project });
  }
  return groups;
}

/** Every edit in a result must be over a valid range, or none are applied. */
export function editsAreSafe(text: string, edits: readonly AssistEdit[]): boolean {
  return edits.every((edit) => isValidRange(text, edit.range));
}
