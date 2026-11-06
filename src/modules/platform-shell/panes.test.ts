import { describe, expect, it } from 'vitest';
import type { NoteSummary } from '../domain';
import {
  activeNoteId,
  applySelection,
  closePane,
  dropNote,
  dropSelection,
  focusOtherPane,
  focusPane,
  initialLayout,
  isNarrow,
  layoutTabs,
  MAX_PANES,
  NARROW_BREAKPOINT_PX,
  openInActivePane,
  renderedPanes,
  selectionOf,
  sharesNote,
  splitPane,
  toggleOrientation,
  visibleColumns,
} from './panes';

function note(noteId: string, relativePath: string): NoteSummary {
  return {
    noteId,
    vaultId: 'vault',
    relativePath,
    contentHash: 'hash',
    localRevision: 1,
    readOnly: false,
  };
}

describe('initial layout', () => {
  it('starts with one active pane', () => {
    const layout = initialLayout('n1');
    expect(layout.panes).toHaveLength(1);
    expect(layout.panes[0]?.active).toBe(true);
    expect(layout.orientation).toBe('horizontal');
    expect(activeNoteId(layout)).toBe('n1');
  });
});

describe('splitting', () => {
  it('splits into a second pane showing the same note', () => {
    // Comparing two places in one note is why a split exists.
    const layout = splitPane(initialLayout('n1'), null);
    expect(layout.panes).toHaveLength(2);
    expect(layout.panes[1]?.noteId).toBe('n1');
    expect(layout.panes[0]?.active).toBe(false);
    expect(layout.panes[1]?.active).toBe(true);
    expect(sharesNote(layout)).toBe(true);
  });

  it('never exceeds two panes', () => {
    let layout = initialLayout('n1');
    for (let index = 0; index < 5; index += 1) {
      layout = splitPane(layout, null);
    }
    expect(layout.panes).toHaveLength(MAX_PANES);
  });

  it('gives each pane a distinct id', () => {
    const layout = splitPane(initialLayout('n1'), null);
    const ids = layout.panes.map((pane) => pane.paneId);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe('closing', () => {
  it('closes a pane and keeps the last one', () => {
    const two = splitPane(initialLayout('n1'), null);
    const one = closePane(two, 'pane-1');
    expect(one.panes).toHaveLength(1);
    expect(activeNoteId(one)).toBe('n1');
  });

  it('never closes the final pane', () => {
    // A window with no pane has nowhere to show the active note.
    const layout = initialLayout('n1');
    expect(closePane(layout, 'pane-1')).toEqual(layout);
  });

  it('moves focus when the active pane closes', () => {
    const two = splitPane(initialLayout('n1'), null);
    expect(two.activePaneId).toBe('pane-2');
    const one = closePane(two, 'pane-2');
    expect(one.activePaneId).toBe('pane-1');
    expect(one.panes[0]?.active).toBe(true);
  });
});

describe('focus', () => {
  it('moves focus to a named pane', () => {
    const two = splitPane(initialLayout('n1'), null);
    const focused = focusPane(two, 'pane-1');
    expect(focused.activePaneId).toBe('pane-1');
    expect(focused.panes[0]?.active).toBe(true);
  });

  it('ignores an unknown pane id', () => {
    const layout = initialLayout('n1');
    expect(focusPane(layout, 'nope')).toEqual(layout);
  });

  it('toggles focus to the other pane', () => {
    const two = splitPane(initialLayout('n1'), null);
    expect(focusOtherPane(two).activePaneId).toBe('pane-1');
    // A single pane has no other.
    expect(focusOtherPane(initialLayout('n1')).activePaneId).toBe('pane-1');
  });
});

describe('orientation and narrow windows', () => {
  it('flips the split direction', () => {
    const layout = toggleOrientation(initialLayout('n1'));
    expect(layout.orientation).toBe('vertical');
    expect(toggleOrientation(layout).orientation).toBe('horizontal');
  });

  it('treats a narrow window as one column', () => {
    expect(isNarrow(NARROW_BREAKPOINT_PX)).toBe(true);
    expect(isNarrow(NARROW_BREAKPOINT_PX + 1)).toBe(false);
    const two = splitPane(initialLayout('n1'), null);
    expect(visibleColumns(two, 1200)).toBe(2);
    expect(visibleColumns(two, 600)).toBe(1);
  });

  it('shows only the active pane when narrow but keeps both', () => {
    // Merging is a display decision; discarding the second pane would lose the
    // layout the user built.
    const two = splitPane(initialLayout('n1'), null);
    const rendered = renderedPanes(two, 600);
    expect(rendered).toHaveLength(1);
    expect(rendered[0]?.paneId).toBe('pane-2');
    // Widening restores both.
    expect(renderedPanes(two, 1400)).toHaveLength(2);
  });

  it('does not rewrite the orientation when narrow', () => {
    const vertical = toggleOrientation(splitPane(initialLayout('n1'), null));
    const rendered = renderedPanes(vertical, 500);
    expect(rendered).toHaveLength(1);
    // The preference survives the merge.
    expect(vertical.orientation).toBe('vertical');
  });
});

describe('note changes', () => {
  it('opens a note in the active pane only', () => {
    // The other pane keeps its note, which makes a reference view possible.
    const two = splitPane(initialLayout('n1'), null);
    const opened = openInActivePane(two, 'n2');
    expect(opened.panes[0]?.noteId).toBe('n1');
    expect(opened.panes[1]?.noteId).toBe('n2');
    expect(sharesNote(opened)).toBe(false);
  });

  it('empties the panes that showed a deleted note', () => {
    // A deleted note must not linger, but the layout survives.
    const two = splitPane(initialLayout('n1'), null);
    const dropped = dropNote(two, 'n1');
    expect(dropped.panes).toHaveLength(2);
    expect(dropped.panes.every((pane) => pane.noteId === null)).toBe(true);
    expect(activeNoteId(dropped)).toBeNull();
  });

  it('lists one tab per note, not per pane', () => {
    const two = splitPane(initialLayout('n1'), null);
    const notes = [note('n1', 'a.md'), note('n2', 'b.md')];
    expect(layoutTabs(two, notes).map((entry) => entry.noteId)).toEqual(['n1']);
    const different = openInActivePane(two, 'n2');
    expect(layoutTabs(different, notes).map((entry) => entry.noteId)).toEqual(['n1', 'n2']);
  });
});

describe('selections', () => {
  it('keeps a selection per pane so two views do not fight over the caret', () => {
    let selections = applySelection([], { paneId: 'pane-1', from: 0, to: 5 });
    selections = applySelection(selections, { paneId: 'pane-2', from: 10, to: 20 });
    expect(selectionOf(selections, 'pane-1')).toEqual({ paneId: 'pane-1', from: 0, to: 5 });
    expect(selectionOf(selections, 'pane-2')).toEqual({ paneId: 'pane-2', from: 10, to: 20 });
  });

  it('replaces the selection for the same pane', () => {
    let selections = applySelection([], { paneId: 'pane-1', from: 0, to: 5 });
    selections = applySelection(selections, { paneId: 'pane-1', from: 7, to: 9 });
    expect(selectionOf(selections, 'pane-1')).toEqual({ paneId: 'pane-1', from: 7, to: 9 });
    expect(selections).toHaveLength(1);
  });

  it('returns null for a pane with no selection', () => {
    expect(selectionOf([], 'pane-1')).toBeNull();
  });

  it('drops a closed pane selection so nothing leaks', () => {
    const selections = applySelection(
      applySelection([], { paneId: 'pane-1', from: 0, to: 1 }),
      { paneId: 'pane-2', from: 2, to: 3 },
    );
    expect(dropSelection(selections, 'pane-2')).toEqual([
      { paneId: 'pane-1', from: 0, to: 1 },
    ]);
  });
});
