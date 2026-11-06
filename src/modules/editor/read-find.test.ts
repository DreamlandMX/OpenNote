import { describe, expect, it } from 'vitest';
import { matchSpans, stepMatch } from './read-find';

describe('find in the reading pane', () => {
  it('finds every occurrence, ignoring case by default', () => {
    expect(matchSpans(['Apple apple APPLE'], 'apple', false)).toEqual([
      { start: [0, 0], end: [0, 5] },
      { start: [0, 6], end: [0, 11] },
      { start: [0, 12], end: [0, 17] },
    ]);
    expect(matchSpans(['Apple apple APPLE'], 'apple', true)).toEqual([{ start: [0, 6], end: [0, 11] }]);
  });

  it('matches across formatting, which splits the text into pieces', () => {
    // `foo **bar** baz` renders as "foo ", "bar", " baz".
    expect(matchSpans(['foo ', 'bar', ' baz'], 'o bar b', false)).toEqual([{ start: [0, 2], end: [2, 2] }]);
  });

  it('ends a match inside the piece it closes, not at the start of the next', () => {
    expect(matchSpans(['ab', 'cd'], 'ab', false)).toEqual([{ start: [0, 0], end: [0, 2] }]);
    expect(matchSpans(['ab', 'cd'], 'cd', false)).toEqual([{ start: [1, 0], end: [1, 2] }]);
  });

  it('finds Chinese text and does not overlap matches', () => {
    expect(matchSpans(['真的真的真的'], '真的真的', false)).toEqual([{ start: [0, 0], end: [0, 4] }]);
  });

  it('finds nothing for an empty query or empty pane', () => {
    expect(matchSpans(['text'], '', false)).toEqual([]);
    expect(matchSpans([], 'x', false)).toEqual([]);
  });

  it('wraps when stepping past either end', () => {
    expect(stepMatch(2, 3, 1)).toBe(0);
    expect(stepMatch(0, 3, -1)).toBe(2);
    expect(stepMatch(-1, 3, -1)).toBe(2);
    expect(stepMatch(0, 0, 1)).toBe(-1);
  });
});
