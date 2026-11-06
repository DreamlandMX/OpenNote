/**
 * F05 mode-switch state preservation.
 *
 * Switching live → source → read must not re-serialize the document (F05-R01),
 * must keep the caret/selection (F05 §6 `{selection, scrollAnchor}`), and must
 * refuse to apply a switch while an IME composition is in progress (F05-A03).
 * This module only computes what the new editor state should be; the caller
 * performs one reconfigure transaction.
 */

export type EditorMode = 'live' | 'source' | 'read';

export interface EditorSelection {
  /** UTF-16 anchor of the main range. */
  anchor: number;
  /** UTF-16 head of the main range. */
  head: number;
  /** The visually dominant range, when the user selected from right to left. */
  reversed: boolean;
}

export interface ModeSnapshot {
  selection: EditorSelection;
  /** First visible document position, used to restore the scroll anchor. */
  scrollTop: number;
  /** Line number at the top of the viewport; survives a layout change better. */
  scrollLine: number;
  /** One-based caret line, used to locate the section in the reading pane. */
  caretLine: number;
}

export interface ModeSwitchDecision {
  /** False means "not now": the caller must keep the current mode. */
  apply: boolean;
  /** Why the switch was deferred, for a status message. */
  deferredReason: 'composition' | null;
  /** Mode to apply when `apply` is true, or the mode to stay in when false. */
  mode: EditorMode;
  /** Selection to restore, or null when the mode has none to restore. */
  selection: EditorSelection | null;
}

/**
 * Decides whether a mode switch may run now.
 *
 * A composition owns the pending text: applying a switch mid-composition is how
 * half-typed characters end up in the document (F05-A03). The caller retries
 * after the composition ends instead of dropping the request silently.
 */
export function decideModeSwitch(
  next: EditorMode,
  /** `null` means the editor has not rendered a mode yet, so nothing to keep. */
  current: EditorMode | null,
  composing: boolean,
  snapshot: ModeSnapshot | null,
): ModeSwitchDecision {
  if (next === current) {
    return { apply: false, deferredReason: null, mode: next, selection: null };
  }
  if (composing) {
    return {
      apply: false,
      deferredReason: 'composition',
      mode: current ?? next,
      selection: null,
    };
  }
  // Read mode hides the editor surface; the selection is remembered so that
  // returning to an editing mode lands where the user left off.
  const selection = next === 'read' ? null : snapshot?.selection ?? null;
  return { apply: true, deferredReason: null, mode: next, selection };
}

/**
 * Clamps a remembered selection into a document of `length`.
 *
 * A mode switch can happen after the document shrank (an external replace, an
 * undo from another tab), and an out-of-range CodeMirror selection throws. A
 * degenerate range at the end is correct and visible; an exception is not.
 */
export function clampSelection(selection: EditorSelection, length: number): EditorSelection {
  const clamp = (value: number): number => {
    // NaN has no position, so it becomes the start; an infinite position is
    // clamped like any other out-of-range value rather than silently collapsing
    // to zero and moving the caret to the top of a long note.
    if (Number.isNaN(value)) {
      return 0;
    }
    return Math.min(Math.max(0, Math.trunc(value)), length);
  };
  const anchor = clamp(selection.anchor);
  const head = clamp(selection.head);
  return { anchor, head, reversed: head < anchor };
}

/**
 * Whether a selection belongs inside the editable body.
 *
 * Live mode hides the frontmatter; a caret left inside it by an earlier state
 * must move to the body start so the first keystroke does not edit hidden text.
 */
export function bodySafeSelection(
  selection: EditorSelection,
  bodyStart: number,
): EditorSelection {
  if (selection.head >= bodyStart && selection.anchor >= bodyStart) {
    return selection;
  }
  const position = Math.max(0, bodyStart);
  return { anchor: position, head: position, reversed: false };
}

/** Reading order comparisons for tests and callers that need a plain value. */
export function selectionText(selection: EditorSelection): { from: number; to: number } {
  return {
    from: Math.min(selection.anchor, selection.head),
    to: Math.max(selection.anchor, selection.head),
  };
}
