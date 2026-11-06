import type { NoteSummary } from '../domain';

/**
 * F04 split panes.
 *
 * A pane is a view over a note id, never a second copy of a note. Two panes may
 * show the same note, and they share the committed version while keeping separate
 * selections (F04-R01) — the sharing is the point, because a second editable copy
 * of the same file would produce two competing saves.
 *
 * `splitOrientation` is a preference, not a permanent state: a narrow window merges
 * to a single column but keeps the preference so widening restores it (§4). Treating
 * the merge as "the user chose single column" would silently discard their choice.
 */

export type Orientation = 'horizontal' | 'vertical';

/** Below this width two panes cannot both show readable text. */
export const NARROW_BREAKPOINT_PX = 720;

/** No more than two panes: more would make each too narrow to read. */
export const MAX_PANES = 2;

export interface Pane {
  paneId: string;
  /** The note shown in this pane, or null for an empty pane. */
  noteId: string | null;
  /** Which pane owns keyboard focus. */
  active: boolean;
}

export interface PaneLayout {
  panes: readonly Pane[];
  orientation: Orientation;
  /** The pane that receives the next focus move. */
  activePaneId: string;
}

/** A view-level selection, so two panes on one note can differ (F04-R01). */
export interface PaneSelection {
  paneId: string;
  from: number;
  to: number;
}

/** The layout a window starts with. */
export function initialLayout(noteId: string | null): PaneLayout {
  return {
    panes: [{ paneId: 'pane-1', noteId, active: true }],
    orientation: 'horizontal',
    activePaneId: 'pane-1',
  };
}

/**
 * Whether the window is too narrow for two panes.
 *
 * At or below the breakpoint the panes merge to one column, which is a display
 * decision and must not rewrite the stored orientation.
 */
export function isNarrow(widthPx: number): boolean {
  return widthPx <= NARROW_BREAKPOINT_PX;
}

/** How many columns are actually shown at a given width. */
export function visibleColumns(layout: PaneLayout, widthPx: number): number {
  if (isNarrow(widthPx)) {
    return 1;
  }
  return Math.min(layout.panes.length, MAX_PANES);
}

/**
 * The panes to render at a given width.
 *
 * In a narrow window only the active pane is shown: hiding the inactive one is
 * better than squeezing both, and the user's focus decides which they see. The
 * layout itself is untouched, so widening restores both.
 */
export function renderedPanes(layout: PaneLayout, widthPx: number): Pane[] {
  if (!isNarrow(widthPx)) {
    return [...layout.panes];
  }
  const active = layout.panes.find((pane) => pane.paneId === layout.activePaneId);
  return active === undefined ? layout.panes.slice(0, 1) : [active];
}

/** The next pane id, so ids stay stable and readable. */
function nextPaneId(layout: PaneLayout): string {
  let index = layout.panes.length + 1;
  const used = new Set(layout.panes.map((pane) => pane.paneId));
  while (used.has(`pane-${index}`)) {
    index += 1;
  }
  return `pane-${index}`;
}

/**
 * Splits the active pane.
 *
 * The new pane shows the same note, which is the useful default: comparing two
 * places in one long note is the reason to split. Both panes keep their own
 * selection, so this does not duplicate the editor state.
 */
export function splitPane(layout: PaneLayout, noteId: string | null): PaneLayout {
  if (layout.panes.length >= MAX_PANES) {
    return layout;
  }
  const source = layout.panes.find((pane) => pane.paneId === layout.activePaneId);
  const paneId = nextPaneId(layout);
  const panes: Pane[] = layout.panes.map((pane) => ({ ...pane, active: false }));
  panes.push({ paneId, noteId: noteId ?? source?.noteId ?? null, active: true });
  return { panes, orientation: layout.orientation, activePaneId: paneId };
}

/**
 * Closes one pane.
 *
 * The last pane is never closed: a window with no pane would have nowhere to show
 * the active note, and the caller shows the vault home instead (F04-R03).
 */
export function closePane(layout: PaneLayout, paneId: string): PaneLayout {
  if (layout.panes.length <= 1) {
    return layout;
  }
  const panes = layout.panes.filter((pane) => pane.paneId !== paneId);
  const wasActive = layout.activePaneId === paneId;
  const activePaneId = wasActive ? (panes[0]?.paneId ?? layout.activePaneId) : layout.activePaneId;
  return {
    panes: panes.map((pane) => ({ ...pane, active: pane.paneId === activePaneId })),
    orientation: layout.orientation,
    activePaneId,
  };
}

/** Moves focus to another pane. */
export function focusPane(layout: PaneLayout, paneId: string): PaneLayout {
  if (!layout.panes.some((pane) => pane.paneId === paneId)) {
    return layout;
  }
  return {
    panes: layout.panes.map((pane) => ({ ...pane, active: pane.paneId === paneId })),
    orientation: layout.orientation,
    activePaneId: paneId,
  };
}

/** Focuses the other pane, for the keyboard shortcut. */
export function focusOtherPane(layout: PaneLayout): PaneLayout {
  if (layout.panes.length < 2) {
    return layout;
  }
  const other = layout.panes.find((pane) => pane.paneId !== layout.activePaneId);
  return other === undefined ? layout : focusPane(layout, other.paneId);
}

/** Flips the split direction. */
export function toggleOrientation(layout: PaneLayout): PaneLayout {
  return {
    panes: [...layout.panes],
    orientation: layout.orientation === 'horizontal' ? 'vertical' : 'horizontal',
    activePaneId: layout.activePaneId,
  };
}

/**
 * Shows a note in the active pane.
 *
 * Opening a note in a pane does not affect the other pane's note, which is what
 * makes a side-by-side reference view possible.
 */
export function openInActivePane(layout: PaneLayout, noteId: string): PaneLayout {
  return {
    panes: layout.panes.map((pane) =>
      pane.paneId === layout.activePaneId ? { ...pane, noteId } : pane,
    ),
    orientation: layout.orientation,
    activePaneId: layout.activePaneId,
  };
}

/**
 * Removes a note from every pane.
 *
 * A deleted note must not linger in a pane: showing it would offer editing of
 * something that no longer exists. Panes that held it become empty rather than
 * closing, so the layout the user built survives.
 */
export function dropNote(layout: PaneLayout, noteId: string): PaneLayout {
  return {
    panes: layout.panes.map((pane) =>
      pane.noteId === noteId ? { ...pane, noteId: null } : pane,
    ),
    orientation: layout.orientation,
    activePaneId: layout.activePaneId,
  };
}

/** The note shown in the active pane. */
export function activeNoteId(layout: PaneLayout): string | null {
  return layout.panes.find((pane) => pane.paneId === layout.activePaneId)?.noteId ?? null;
}

/**
 * Whether two panes show the same note.
 *
 * The caller uses this to share one committed version between them instead of
 * loading the note twice, which is the substance of F04-R01.
 */
export function sharesNote(layout: PaneLayout): boolean {
  const ids = layout.panes.map((pane) => pane.noteId).filter((id): id is string => id !== null);
  return new Set(ids).size < ids.length;
}

/**
 * Merges a selection update.
 *
 * A selection belongs to one pane only. Applying it to the active pane regardless
 * of where it came from would make the two views fight over the caret.
 */
export function applySelection(
  selections: readonly PaneSelection[],
  update: PaneSelection,
): PaneSelection[] {
  const next = selections.filter((selection) => selection.paneId !== update.paneId);
  next.push(update);
  return next.sort((left, right) => left.paneId.localeCompare(right.paneId));
}

/** The selection for a pane, or null when it has never been set. */
export function selectionOf(
  selections: readonly PaneSelection[],
  paneId: string,
): PaneSelection | null {
  return selections.find((selection) => selection.paneId === paneId) ?? null;
}

/** Drops a pane's selection when the pane closes, so nothing leaks. */
export function dropSelection(
  selections: readonly PaneSelection[],
  paneId: string,
): PaneSelection[] {
  return selections.filter((selection) => selection.paneId !== paneId);
}

/**
 * The tabs a layout represents.
 *
 * Both panes may show one note, but there is still one tab for it: the tab strip
 * lists notes, not panes.
 */
export function layoutTabs(layout: PaneLayout, notes: readonly NoteSummary[]): NoteSummary[] {
  const ids = new Set(
    layout.panes.map((pane) => pane.noteId).filter((id): id is string => id !== null),
  );
  return notes.filter((note) => ids.has(note.noteId));
}
