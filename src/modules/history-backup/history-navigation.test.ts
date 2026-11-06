import { describe, expect, it } from 'vitest';
import { revisionFocusTarget } from './history-navigation';

describe('history revision keyboard navigation', () => {
  it('moves within the list and stops at both ends', () => {
    expect(revisionFocusTarget('ArrowDown', 0, 3)).toBe(1);
    expect(revisionFocusTarget('ArrowDown', 2, 3)).toBe(2);
    expect(revisionFocusTarget('ArrowUp', 2, 3)).toBe(1);
    expect(revisionFocusTarget('ArrowUp', 0, 3)).toBe(0);
  });

  it('jumps to the first and last revision', () => {
    expect(revisionFocusTarget('Home', 2, 3)).toBe(0);
    expect(revisionFocusTarget('End', 0, 3)).toBe(2);
  });

  it('starts from the first item when no revision button has focus', () => {
    expect(revisionFocusTarget('ArrowDown', -1, 3)).toBe(1);
    expect(revisionFocusTarget('End', -1, 3)).toBe(2);
  });

  it('leaves other keys and empty lists alone', () => {
    expect(revisionFocusTarget('Enter', 1, 3)).toBeNull();
    expect(revisionFocusTarget('ArrowDown', 0, 0)).toBeNull();
  });
});
