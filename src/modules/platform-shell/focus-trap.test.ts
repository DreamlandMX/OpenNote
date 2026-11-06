import { describe, expect, it } from 'vitest';
import { trapTabIndex } from './focus-trap';

describe('modal tab containment', () => {
  it('leaves the default alone when there is nothing to focus', () => {
    expect(trapTabIndex(0, -1, false)).toBeNull();
    expect(trapTabIndex(0, 0, true)).toBeNull();
  });

  it('keeps Tab inside the dialog', () => {
    expect(trapTabIndex(3, 0, false)).toBeNull();
    expect(trapTabIndex(3, 1, false)).toBeNull();
    expect(trapTabIndex(3, 2, false)).toBe(0);
  });

  it('keeps Shift+Tab inside the dialog', () => {
    expect(trapTabIndex(3, 2, true)).toBeNull();
    expect(trapTabIndex(3, 1, true)).toBeNull();
    expect(trapTabIndex(3, 0, true)).toBe(2);
  });

  it('moves into the dialog when focus is on the container or outside', () => {
    expect(trapTabIndex(3, -1, false)).toBe(0);
    expect(trapTabIndex(3, -1, true)).toBe(2);
  });

  it('cycles a single focusable element back to itself', () => {
    expect(trapTabIndex(1, 0, false)).toBe(0);
    expect(trapTabIndex(1, 0, true)).toBe(0);
  });
});
