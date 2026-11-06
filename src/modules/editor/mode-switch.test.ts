import { describe, expect, it } from 'vitest';
import {
  bodySafeSelection,
  clampSelection,
  decideModeSwitch,
  selectionText,
  type ModeSnapshot,
} from './mode-switch';

const snapshot: ModeSnapshot = {
  selection: { anchor: 12, head: 20, reversed: false },
  scrollTop: 480,
  scrollLine: 9,
  caretLine: 13,
};

describe('mode switch decisions', () => {
  it('refuses to switch during an IME composition and says why', () => {
    const decision = decideModeSwitch('read', 'live', true, snapshot);
    expect(decision).toEqual({
      apply: false,
      deferredReason: 'composition',
      mode: 'live',
      selection: null,
    });
  });

  it('does nothing when the mode is unchanged', () => {
    expect(decideModeSwitch('live', 'live', false, snapshot)).toEqual({
      apply: false,
      deferredReason: null,
      mode: 'live',
      selection: null,
    });
  });

  it('keeps the selection when moving between editing modes', () => {
    const decision = decideModeSwitch('source', 'live', false, snapshot);
    expect(decision.apply).toBe(true);
    expect(decision.selection).toEqual(snapshot.selection);
    expect(decideModeSwitch('live', 'source', false, snapshot).selection).toEqual(
      snapshot.selection,
    );
  });

  it('drops the selection for read mode but still switches', () => {
    const decision = decideModeSwitch('read', 'live', false, snapshot);
    expect(decision.apply).toBe(true);
    expect(decision.mode).toBe('read');
    expect(decision.selection).toBeNull();
  });

  it('switches without a snapshot instead of failing', () => {
    expect(decideModeSwitch('source', 'live', false, null)).toMatchObject({
      apply: true,
      selection: null,
    });
  });
});

describe('selection clamping', () => {
  it('clamps both ends into the document', () => {
    expect(clampSelection({ anchor: -5, head: 99, reversed: false }, 40)).toEqual({
      anchor: 0,
      head: 40,
      reversed: false,
    });
  });

  it('keeps a reversed selection reversed', () => {
    const clamped = clampSelection({ anchor: 30, head: 10, reversed: true }, 40);
    expect(clamped).toEqual({ anchor: 30, head: 10, reversed: true });
    expect(selectionText(clamped)).toEqual({ from: 10, to: 30 });
  });

  it('treats a non-finite position as the document start', () => {
    expect(clampSelection({ anchor: Number.NaN, head: Number.POSITIVE_INFINITY, reversed: false }, 10))
      .toEqual({ anchor: 0, head: 10, reversed: false });
  });
});

describe('body-safe selections', () => {
  it('moves a caret inside hidden frontmatter to the body start', () => {
    expect(
      bodySafeSelection({ anchor: 3, head: 3, reversed: false }, 18),
    ).toEqual({ anchor: 18, head: 18, reversed: false });
  });

  it('keeps a selection already inside the body', () => {
    const inside = { anchor: 20, head: 30, reversed: false };
    expect(bodySafeSelection(inside, 18)).toBe(inside);
  });

  it('moves a partially hidden selection to the body start', () => {
    expect(bodySafeSelection({ anchor: 4, head: 30, reversed: false }, 18)).toEqual({
      anchor: 18,
      head: 18,
      reversed: false,
    });
  });
});
