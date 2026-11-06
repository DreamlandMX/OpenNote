import type { NoteSummary } from '../domain';

/**
 * F04 tab and recent-record model. Everything is device state: tabs are a view
 * over note ids, and a recent entry is only a remembered id, never a second
 * copy of a note. Renaming a file keeps the id, so a remembered id keeps
 * pointing at the same note (F04-R02).
 */

export interface TabState {
  tabs: readonly NoteSummary[];
  activeId: string | null;
}

/**
 * Closes one tab. The neighbour to the right becomes active, then the left, so
 * closing never leaves a gap. Closing the last tab yields no active note and
 * the caller shows the vault home state; it never closes the app (F04-R03).
 */
export function closeTab(state: TabState, closingId: string): TabState {
  const index = state.tabs.findIndex((note) => note.noteId === closingId);
  if (index < 0) {
    return state;
  }
  const tabs = state.tabs.filter((note) => note.noteId !== closingId);
  if (state.activeId !== closingId) {
    return { tabs, activeId: state.activeId };
  }
  const next = tabs[index] ?? tabs[index - 1] ?? null;
  return { tabs, activeId: next?.noteId ?? null };
}

/** Ctrl+Tab / Ctrl+Shift+Tab; wraps in both directions. */
export function cycleTab(
  tabs: readonly NoteSummary[],
  activeId: string | null,
  step: number,
): string | null {
  if (tabs.length === 0) {
    return null;
  }
  const current = tabs.findIndex((note) => note.noteId === activeId);
  if (current < 0) {
    return tabs[0]?.noteId ?? null;
  }
  const size = tabs.length;
  const next = (((current + step) % size) + size) % size;
  return tabs[next]?.noteId ?? null;
}

export interface NoteReferenceEntry {
  noteId: string;
  /** Path at the time it was remembered; only a label, never trusted as truth. */
  rememberedPath: string | null;
  /** The note as it is now, when it still exists. */
  note: NoteSummary | null;
  available: boolean;
}

/**
 * Resolves remembered ids against the current note list. A missing id is kept
 * as an unavailable entry so the user can remove it deliberately instead of
 * having it silently vanish or open an empty note (F04 §7 NOTE_UNAVAILABLE).
 */
function resolveRemembered(
  ids: readonly string[],
  notes: readonly NoteSummary[],
): NoteReferenceEntry[] {
  const byId = new Map(notes.map((note) => [note.noteId, note] as const));
  const seen = new Set<string>();
  const entries: NoteReferenceEntry[] = [];
  for (const noteId of ids) {
    if (seen.has(noteId)) {
      continue;
    }
    seen.add(noteId);
    const note = byId.get(noteId) ?? null;
    entries.push({
      noteId,
      rememberedPath: note?.relativePath ?? null,
      note,
      available: note !== null,
    });
  }
  return entries;
}

export function resolveRecent(
  recentIds: readonly string[],
  notes: readonly NoteSummary[],
): NoteReferenceEntry[] {
  return resolveRemembered(recentIds, notes);
}

/**
 * Favorites are note ids, like recents. A rename keeps the id, so a favorite
 * keeps opening the same note; a deleted note stays as unavailable until the
 * user removes it (F04-R02). No second copy of the path is treated as truth.
 */
export function resolveFavorites(
  favoriteIds: readonly string[],
  notes: readonly NoteSummary[],
): NoteReferenceEntry[] {
  return resolveRemembered(favoriteIds, notes);
}

/**
 * Adds the note to the front of the list, or removes it when already starred.
 * The limit applies when adding, so dropping an older favorite is the only way
 * the list shrinks without the user removing it explicitly.
 */
export function toggleFavorite(
  favorites: readonly string[],
  noteId: string,
  limit: number,
): readonly string[] {
  if (noteId === '') {
    return favorites;
  }
  if (favorites.includes(noteId)) {
    return favorites.filter((entry) => entry !== noteId);
  }
  return [noteId, ...favorites].slice(0, limit);
}

export function isFavorite(favorites: readonly string[], noteId: string): boolean {
  return favorites.includes(noteId);
}

export function removeRecent(recentIds: readonly string[], noteId: string): readonly string[] {
  return recentIds.filter((entry) => entry !== noteId);
}

/** Display name for a remembered-note row: current path, else a stable placeholder. */
export function recentLabel(entry: NoteReferenceEntry): string {
  if (entry.note !== null) {
    return entry.note.relativePath.split(/[/\\]/).pop()?.replace(/\.md$/i, '') ?? entry.note.relativePath;
  }
  return entry.rememberedPath ?? entry.noteId;
}

/** F04-A02: a tab with an unsaved failure must be offered save/cancel first. */
export function tabNeedsGuard(
  entry: { status: string } | undefined,
): entry is { status: 'dirty' | 'saving' | 'failed' | 'conflict' } {
  return entry !== undefined && entry.status !== 'clean';
}
