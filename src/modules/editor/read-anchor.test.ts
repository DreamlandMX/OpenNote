import { describe, expect, it } from 'vitest';
import {
  countHeadings,
  headingIndexAtLine,
  readPaneTarget,
  type HeadingAnchor,
} from './read-anchor';

const headings: HeadingAnchor[] = [
  { line: 3, text: '从记录到连接' },
  { line: 8, text: '本周行动' },
  { line: 12, text: '同名' },
  { line: 14, text: '同名' },
];

describe('read pane anchoring', () => {
  it('finds the last heading at or before the caret', () => {
    expect(headingIndexAtLine(headings, 0)).toBe(-1);
    expect(headingIndexAtLine(headings, 3)).toBe(0);
    expect(headingIndexAtLine(headings, 7)).toBe(0);
    expect(headingIndexAtLine(headings, 9)).toBe(1);
    expect(headingIndexAtLine(headings, 99)).toBe(3);
  });

  it('distinguishes duplicate headings by position, not by text', () => {
    expect(headingIndexAtLine(headings, 12)).toBe(2);
    expect(headingIndexAtLine(headings, 14)).toBe(3);
  });

  it('returns nothing to scroll to before the first heading', () => {
    expect(readPaneTarget(headings, 4, 1)).toBe(-1);
  });

  it('returns nothing when the rendered pane has no headings', () => {
    expect(readPaneTarget(headings, 0, 20)).toBe(-1);
    expect(readPaneTarget([], 4, 20)).toBe(-1);
  });

  it('clamps to the last real element when the two lists disagree', () => {
    expect(readPaneTarget(headings, 2, 99)).toBe(1);
  });

  it('counts heading elements in rendered HTML, including levels 4-6', () => {
    expect(countHeadings('<h1>a</h1><h2>b</h2><h3>c</h3><h4>d</h4><h5>e</h5><h6>f</h6>')).toBe(6);
    expect(countHeadings('<p>no headings</p>')).toBe(0);
    expect(countHeadings('<header>not a heading</header><h2 class="x">yes</h2>')).toBe(1);
  });
});
