import { describe, expect, it } from 'vitest';
import type { HeadingInfo } from '../editor';
import {
  buildOutline,
  currentSection,
  lineStartOffsets,
  moveOutline,
  outlineIsStale,
  toggleCollapsed,
  visibleOutline,
} from './outline';

function heading(level: number, text: string, slug: string, line: number): HeadingInfo {
  return { level, text, slug, line, offset: 0 };
}

const source = [
  '---',
  'title: demo',
  '---',
  '# 从记录到连接', // line 3
  '',
  '正文',
  '## 本周行动', // line 6
  '### 细节', // line 7
  '## 下一节', // line 8
].join('\n');

const headings: HeadingInfo[] = [
  heading(1, '从记录到连接', '从记录到连接', 3),
  heading(2, '本周行动', '本周行动', 6),
  heading(3, '细节', '细节', 7),
  heading(2, '下一节', '下一节', 8),
];

describe('outline model', () => {
  it('maps a line index to a UTF-16 line start, not the host byte offset', () => {
    const offsets = lineStartOffsets(source);
    expect(offsets[3]).toBe(source.indexOf('# 从记录到连接'));
    expect(offsets[6]).toBe(source.indexOf('## 本周行动'));
    const rows = buildOutline(headings, source);
    expect(rows.map((row) => row.offset)).toEqual([
      source.indexOf('# 从记录到连接'),
      source.indexOf('## 本周行动'),
      source.indexOf('### 细节'),
      source.indexOf('## 下一节'),
    ]);
  });

  it('keeps heading levels as depth and marks parents that have children', () => {
    const rows = buildOutline(headings, source);
    expect(rows.map((row) => row.depth)).toEqual([0, 1, 2, 1]);
    expect(rows.map((row) => row.hasChildren)).toEqual([true, true, false, false]);
  });

  it('hides descendants of a folded heading and keeps siblings visible', () => {
    const rows = buildOutline(headings, source, ['本周行动']);
    expect(visibleOutline(rows).map((row) => row.text)).toEqual([
      '从记录到连接',
      '本周行动',
      '下一节',
    ]);
    const folded = rows.find((row) => row.id === '本周行动');
    expect(folded?.collapsed).toBe(true);
    expect(rows.find((row) => row.id === '细节')?.hidden).toBe(true);
  });

  it('reports the last heading at or before the caret', () => {
    const rows = buildOutline(headings, source);
    expect(currentSection(rows, 0)).toBeNull();
    expect(currentSection(rows, rows[2]!.offset + 4)?.text).toBe('细节');
    expect(currentSection(rows, source.length)?.text).toBe('下一节');
  });

  it('moves through visible rows and respects folding on parent/child moves', () => {
    const rows = buildOutline(headings, source, ['本周行动']);
    expect(moveOutline(rows, null, 'down')).toBe('从记录到连接');
    expect(moveOutline(rows, '从记录到连接', 'down')).toBe('本周行动');
    expect(moveOutline(rows, '本周行动', 'next')).toBe('下一节');
    expect(moveOutline(rows, '下一节', 'up')).toBe('本周行动');
    // Folded: no child is reachable.
    expect(moveOutline(rows, '本周行动', 'firstChild')).toBe('本周行动');
    const expanded = buildOutline(headings, source);
    expect(moveOutline(expanded, '本周行动', 'firstChild')).toBe('细节');
    expect(moveOutline(expanded, '细节', 'parent')).toBe('本周行动');
    expect(moveOutline(expanded, '从记录到连接', 'up')).toBe('从记录到连接');
  });

  it('toggles a folded id without touching other preferences', () => {
    expect(toggleCollapsed([], 'a')).toEqual(['a']);
    expect(toggleCollapsed(['a', 'b'], 'a')).toEqual(['b']);
  });

  it('detects an outline that no longer matches the headings (F14-R02)', () => {
    const rows = buildOutline(headings, source);
    expect(outlineIsStale(rows, headings)).toBe(false);
    expect(outlineIsStale(rows, headings.slice(0, 3))).toBe(true);
    const renamed = headings.map((entry, index) =>
      index === 1 ? { ...entry, slug: '本周行动-1' } : entry,
    );
    expect(outlineIsStale(rows, renamed)).toBe(true);
  });

  it('returns no rows for a note without headings instead of inventing one', () => {
    expect(buildOutline([], '# body only')).toEqual([]);
    expect(visibleOutline([])).toEqual([]);
    expect(currentSection([], 10)).toBeNull();
  });

  it('clamps a malformed level into 1–6', () => {
    const rows = buildOutline(
      [heading(0, 'zero', 'zero', 0), heading(9, 'nine', 'nine', 1)],
      'zero\nnine',
    );
    expect(rows.map((row) => row.depth)).toEqual([0, 5]);
  });
});
