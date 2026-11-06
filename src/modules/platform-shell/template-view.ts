import type { AssistApplier, NoteService, ServiceOutcome, TemplateExpansion, TemplateRow, TemplateVariables } from '../editor';
import type { ExpansionWarning } from '../editor';
import { utf8OffsetToUtf16Index } from './citation-panel';
import { serviceErrorFromThrown } from './port';

/**
 * S33/S34 template picker logic.
 *
 * The expansion itself lives in M06 so preview and insert cannot disagree; this
 * module only decides how a template is presented and how its warnings are
 * reported without erasing the author's text (F18-R01).
 */

export interface TemplateChoice {
  noteId: string;
  relativePath: string;
  title: string;
  /** First non-empty body line after the frontmatter, as a preview subtitle. */
  summary: string;
  /** Body without the template's own frontmatter, which is what gets expanded. */
  body: string;
}

export interface TemplatePreviewSnapshot {
  service: Pick<NoteService, 'expandTemplate'>;
  noteId: string;
  body: string;
  variables: TemplateVariables;
  outcome: ServiceOutcome<TemplateExpansion>;
}

export function currentTemplatePreview(snapshot: TemplatePreviewSnapshot | null, service: Pick<NoteService, 'expandTemplate'>, choice: TemplateChoice | null, variables: TemplateVariables): ServiceOutcome<TemplateExpansion> | null {
  return snapshot !== null && snapshot.service === service && snapshot.noteId === choice?.noteId &&
    snapshot.body === choice.body && snapshot.variables.title === variables.title &&
    snapshot.variables.date === variables.date && snapshot.variables.time === variables.time ? snapshot.outcome : null;
}

export function observeTemplatePreview(service: Pick<NoteService, 'expandTemplate'>, choice: TemplateChoice, variables: TemplateVariables, publish: (snapshot: TemplatePreviewSnapshot) => void): () => void {
  let active = true;
  void (async () => {
    let outcome: ServiceOutcome<TemplateExpansion>;
    try { outcome = await service.expandTemplate(choice.body, variables); }
    catch (error) { outcome = { ok: false, error: serviceErrorFromThrown(error) }; }
    if (active) publish({ service, noteId: choice.noteId, body: choice.body, variables, outcome });
  })();
  return () => { active = false; };
}

export interface TemplateInsertTarget {
  noteId: string;
  paneId: string;
  source: string;
  selection: { from: number; to: number };
}

export function sameTemplateTarget(target: TemplateInsertTarget | null, current: TemplateInsertTarget | null): boolean {
  return target !== null && current !== null && target.noteId === current.noteId &&
    target.paneId === current.paneId && target.source === current.source &&
    target.selection.from === current.selection.from && target.selection.to === current.selection.to;
}

export function applyTemplateInsert(target: TemplateInsertTarget | null, current: TemplateInsertTarget | null, apply: AssistApplier | undefined, text: string, caret: number, writable: boolean): boolean {
  if (!writable || !target || !sameTemplateTarget(target, current) || !apply) return false;
  return apply([{ start: target.selection.from, end: target.selection.to, text }], {
    noteId: target.noteId, source: target.source, selection: target.selection, caret: target.selection.from + caret,
  });
}

/**
 * Removes the frontmatter block from a template body.
 *
 * A template's `template: true` marker is metadata, not content, so it must never
 * be inserted into the new note.
 */
export function templateBody(body: string): string {
  const match = /^---\r?\n([\s\S]*?)\r?\n(?:---|\.\.\.)\r?\n?/.exec(body);
  return match === null ? body : body.slice(match[0].length);
}

export function toChoices(templates: readonly TemplateRow[]): TemplateChoice[] {
  return templates
    .map((template) => {
      const body = templateBody(template.body);
      const summary =
        body
          .split(/\r?\n/)
          .map((line) => line.trim())
          .find((line) => line !== '' && !line.startsWith('#')) ?? '';
      return {
        noteId: template.noteId,
        relativePath: template.relativePath,
        title: template.title,
        summary,
        body,
      };
    })
    .sort((left, right) => left.title.localeCompare(right.title));
}

/** Warning kinds, deduplicated by name, so one typo is reported once. */
export interface WarningSummary {
  unknown: string[];
  unterminated: boolean;
}

export function summarizeWarnings(warnings: readonly ExpansionWarning[]): WarningSummary {
  const unknown = new Set<string>();
  let unterminated = false;
  for (const warning of warnings) {
    if (warning.kind === 'unknownVariable') {
      unknown.add(warning.name);
    } else {
      unterminated = true;
    }
  }
  return { unknown: [...unknown].sort((left, right) => left.localeCompare(right)), unterminated };
}

/**
 * The fragment to insert plus the caret position.
 *
 * When the template has no `{{cursor}}`, the caret goes to the end, which is
 * where a user would type next. An offset beyond the fragment is clamped rather
 * than trusted, because a malformed host response must not move the caret into
 * an unrelated position.
 */
export function insertable(
  expansion: TemplateExpansion,
): { text: string; caret: number } {
  const raw = expansion.fragment;
  const offset = expansion.cursorOffset;
  const index = offset === null || !Number.isFinite(offset) ? raw.length : utf8OffsetToUtf16Index(raw, offset);
  return { text: raw.replace(/\r\n?/g, '\n'), caret: raw.slice(0, index).replace(/\r\n?/g, '\n').length };
}

/** True when any warning means the user must look before inserting. */
export function needsAttention(expansion: TemplateExpansion): boolean {
  return expansion.warnings.length > 0;
}

/** Builds the variables for a new note: title from the chosen name, local date. */
export function variablesFor(title: string, today: string, time: string | null): TemplateVariables {
  return { title, date: today, time };
}

/**
 * The file name a template's new note would get.
 *
 * Kept separate from M06's Rust helper on purpose: the UI preview must show the
 * same sanitization the host will apply, and a test pins both behaviours.
 */
export function noteNameFor(title: string): string {
  const cleaned = title
    .trim()
    .replace(/[/\\:*?"<>|]/g, '-')
    .replace(/\0/g, '')
    .replace(/[. ]+$/, '');
  return cleaned === '' ? 'Untitled' : cleaned;
}

/**
 * Replaces the selection (or inserts at the caret) and returns the new source
 * plus the absolute caret position.
 *
 * The caller must feed this through the editor, not through a replaced
 * `<textarea>` value: the editor transaction is what makes the insertion a
 * single undo step (F18 §4 mode `insert`).
 */
export function insertAtSelection(
  source: string,
  selection: { from: number; to: number } | null,
  fragment: string,
  caretInFragment: number,
): { source: string; caret: number } {
  const from = Math.min(Math.max(0, selection?.from ?? source.length), source.length);
  const to = Math.min(Math.max(from, selection?.to ?? from), source.length);
  const next = source.slice(0, from) + fragment + source.slice(to);
  const caret = Math.min(Math.max(0, from + caretInFragment), next.length);
  return { source: next, caret };
}
