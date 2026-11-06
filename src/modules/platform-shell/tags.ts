import type { TagEntry, TagReport } from '../editor';

/**
 * S15 tag browsing. Everything here is derived from the host's rebuildable tag
 * report; the note source stays authoritative (F08-R02). This module never
 * writes a note, so it cannot remove a tag or a note (F08-R03).
 */

export const MIN_TAG_LENGTH = 1;
export const MAX_TAG_LENGTH = 64;

export interface TagRow {
  tag: string;
  /** Notes carrying the tag, in path order. */
  notes: readonly { noteId: string; relativePath: string }[];
  count: number;
}

export interface TagValidation {
  ok: boolean;
  /** Trimmed value, suitable for display and comparison. */
  value: string;
  reason: 'empty' | 'tooLong' | null;
}

/**
 * Trims edge whitespace and enforces the 1–64 character rule. Display case is
 * preserved: `#Project` and `#project` stay distinct strings here, because
 * F08 says identity comparisons are the caller's decision, not a silent merge.
 */
export function validateTag(value: string): TagValidation {
  const trimmed = value.trim();
  if (trimmed.length === 0) {
    return { ok: false, value: trimmed, reason: 'empty' };
  }
  if (Array.from(trimmed).length > MAX_TAG_LENGTH) {
    return { ok: false, value: trimmed, reason: 'tooLong' };
  }
  return { ok: true, value: trimmed, reason: null };
}

/** Normalizes a leading `#` so a chip and a typed tag compare the same way. */
export function normalizeTag(value: string): string {
  const trimmed = value.trim();
  return trimmed.startsWith('#') ? trimmed.slice(1) : trimmed;
}

/** Rows sorted by display name, with ids and paths kept aligned per note. */
export function tagRows(report: TagReport): TagRow[] {
  const rows: TagRow[] = [];
  for (const entry of report.tags as readonly TagEntry[]) {
    const notes: { noteId: string; relativePath: string }[] = [];
    const count = Math.min(entry.noteIds.length, entry.notePaths.length);
    for (let index = 0; index < count; index += 1) {
      const noteId = entry.noteIds[index];
      const relativePath = entry.notePaths[index];
      if (noteId === undefined || relativePath === undefined) {
        continue;
      }
      notes.push({ noteId, relativePath });
    }
    if (notes.length === 0) {
      continue;
    }
    rows.push({ tag: entry.tag, notes, count: notes.length });
  }
  return rows.sort((left, right) => left.tag.localeCompare(right.tag));
}

/**
 * Filters by a query. A leading `#` is accepted because the UI shows tags that
 * way. An empty query returns every row, which is a browse state, not a search
 * over the vault.
 */
export function filterTags(rows: readonly TagRow[], query: string): TagRow[] {
  const needle = normalizeTag(query).toLocaleLowerCase();
  if (needle === '') {
    return [...rows];
  }
  return rows.filter((row) => normalizeTag(row.tag).toLocaleLowerCase().includes(needle));
}

/** Notes of one tag, optionally narrowed by a path query. */
export function notesForTag(
  row: TagRow | null,
  query: string,
): readonly { noteId: string; relativePath: string }[] {
  if (row === null) {
    return [];
  }
  const needle = query.trim().toLocaleLowerCase();
  if (needle === '') {
    return row.notes;
  }
  return row.notes.filter((note) => note.relativePath.toLocaleLowerCase().includes(needle));
}

/** Total distinct tags and notes, for the panel status line. */
export function tagSummary(rows: readonly TagRow[]): { tags: number; notes: number } {
  const noteIds = new Set<string>();
  for (const row of rows) {
    for (const note of row.notes) {
      noteIds.add(note.noteId);
    }
  }
  return { tags: rows.length, notes: noteIds.size };
}

/**
 * True when a tag report contains a note id that the current note list no
 * longer has, i.e. the tree and the tag index disagree. The panel then asks for
 * a refresh instead of opening a missing note (NOTE_UNAVAILABLE).
 */
export function hasMissingNotes(
  rows: readonly TagRow[],
  knownNoteIds: readonly string[],
): boolean {
  const known = new Set(knownNoteIds);
  return rows.some((row) => row.notes.some((note) => !known.has(note.noteId)));
}
