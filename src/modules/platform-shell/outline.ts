import type { HeadingInfo } from '../editor';

/**
 * F14 outline model. The host already resolves ATX headings outside fenced code
 * and gives every duplicate slug a stable suffix, so this module never re-parses
 * markdown: it only derives display rows, folding and the current section.
 */

export interface OutlineRow {
  /** Stable anchor from the host (`slug`), unique within the note. */
  id: string;
  level: number;
  text: string;
  /** UTF-16 offset of the heading line start, for the editor jump. */
  offset: number;
  line: number;
  /** 0 for a top-level row, 1+ inside a folded ancestor. */
  depth: number;
  hasChildren: boolean;
  collapsed: boolean;
  /** Hidden because an ancestor is collapsed. */
  hidden: boolean;
}

const MIN_LEVEL = 1;
const MAX_LEVEL = 6;

function normalizeLevel(level: number): number {
  if (!Number.isFinite(level)) {
    return MIN_LEVEL;
  }
  return Math.min(MAX_LEVEL, Math.max(MIN_LEVEL, Math.trunc(level)));
}

/**
 * UTF-16 offset of every line start. The host reports a zero-based line index
 * and a byte offset; CodeMirror positions are UTF-16, so the line index is the
 * safe input to convert. A trailing line is included so `line` can equal the
 * count of lines without going out of range.
 */
export function lineStartOffsets(source: string): number[] {
  const offsets = [0];
  for (let index = 0; index < source.length; index += 1) {
    if (source.charCodeAt(index) === 10) {
      offsets.push(index + 1);
    }
  }
  return offsets;
}

/**
 * Builds the visible tree. `collapsedIds` is a device preference: folding is
 * layout only and never changes the note (F14 §4). `source` supplies line
 * starts so a jump uses UTF-16 units, not the host's byte offset.
 */
export function buildOutline(
  headings: readonly HeadingInfo[],
  source: string,
  collapsedIds: readonly string[] = [],
): OutlineRow[] {
  const lineStarts = lineStartOffsets(source);
  const collapsed = new Set(collapsedIds);
  const rows: OutlineRow[] = [];
  // Levels whose subtree is folded. A heading at or above a folded level closes
  // that fold, which is what a tree user expects when a sibling follows.
  const foldedLevels: number[] = [];
  for (const heading of headings) {
    const level = normalizeLevel(heading.level);
    while (foldedLevels.length > 0 && level <= foldedLevels[foldedLevels.length - 1]!) {
      foldedLevels.pop();
    }
    const hidden = foldedLevels.length > 0;
    if (collapsed.has(heading.slug)) {
      foldedLevels.push(level);
    }
    rows.push({
      id: heading.slug,
      level,
      text: heading.text,
      offset: lineStarts[heading.line] ?? heading.offset,
      line: heading.line,
      depth: level - MIN_LEVEL,
      hasChildren: false,
      collapsed: collapsed.has(heading.slug),
      hidden,
    });
  }
  // A row has children when a later row is deeper than it, before any row at
  // the same or a shallower level. Scanning backwards makes this O(n).
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const row = rows[index]!;
    let hasChildren = false;
    for (let next = index + 1; next < rows.length; next += 1) {
      const candidate = rows[next]!;
      if (candidate.level <= row.level) {
        break;
      }
      hasChildren = true;
      break;
    }
    rows[index] = { ...row, hasChildren };
  }
  return rows;
}

/** Rows a keyboard user can reach: folded descendants are skipped. */
export function visibleOutline(rows: readonly OutlineRow[]): OutlineRow[] {
  return rows.filter((row) => !row.hidden);
}

/** Heading whose section contains `offset`; the last one at or before it. */
export function currentSection(
  rows: readonly OutlineRow[],
  offset: number | null,
): OutlineRow | null {
  if (offset === null || rows.length === 0) {
    return null;
  }
  let current: OutlineRow | null = null;
  for (const row of rows) {
    if (row.offset <= offset) {
      current = row;
    } else {
      break;
    }
  }
  return current;
}

/**
 * Moves the active row with the arrow keys. Left folds the active row or moves
 * to its parent; Right unfolds or moves to its first child (F14 §8).
 */
export function moveOutline(
  rows: readonly OutlineRow[],
  activeId: string | null,
  motion: 'up' | 'down' | 'parent' | 'firstChild' | 'next',
): string | null {
  const visible = visibleOutline(rows);
  if (visible.length === 0) {
    return null;
  }
  const index = visible.findIndex((row) => row.id === activeId);
  if (index < 0) {
    return visible[0]!.id;
  }
  const row = visible[index]!;
  if (motion === 'up') {
    return visible[Math.max(0, index - 1)]!.id;
  }
  if (motion === 'down') {
    return visible[Math.min(visible.length - 1, index + 1)]!.id;
  }
  if (motion === 'next') {
    return visible[(index + 1) % visible.length]!.id;
  }
  if (motion === 'parent') {
    for (let previous = index - 1; previous >= 0; previous -= 1) {
      if (visible[previous]!.level < row.level) {
        return visible[previous]!.id;
      }
    }
    return row.id;
  }
  // firstChild: only meaningful when the row is expanded and really has one.
  if (!row.hasChildren || row.collapsed) {
    return row.id;
  }
  for (let next = index + 1; next < visible.length; next += 1) {
    if (visible[next]!.level > row.level) {
      return visible[next]!.id;
    }
    break;
  }
  return row.id;
}

export function toggleCollapsed(
  collapsedIds: readonly string[],
  id: string,
): readonly string[] {
  return collapsedIds.includes(id)
    ? collapsedIds.filter((entry) => entry !== id)
    : [...collapsedIds, id];
}

/**
 * True when the outline no longer matches the note's headings, i.e. a heading
 * was removed or its slug changed after a save (F14-R02). Rows are keyed by
 * slug, so a stale id must never be used to jump.
 */
export function outlineIsStale(
  rows: readonly OutlineRow[],
  headings: readonly HeadingInfo[],
): boolean {
  if (rows.length !== headings.length) {
    return true;
  }
  return rows.some((row, index) => row.id !== headings[index]?.slug);
}
