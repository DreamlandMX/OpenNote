/**
 * F06/S11 insertion model. A slash-menu item produces one replacement over the
 * current selection; the caller applies it as a single editor transaction, so a
 * table insertion is undone by one undo and restores the original selection
 * (F06-A03). Nothing here touches the document, the file system or a renderer,
 * which keeps the rules testable and keeps M03 free of IPC.
 */

export type FragmentKind =
  | 'text'
  | 'heading'
  | 'task'
  | 'table'
  | 'equation'
  | 'code'
  | 'link';

export interface Selection {
  /** UTF-16 offset of the selection start. */
  from: number;
  /** UTF-16 offset of the selection end; equal to `from` for a caret. */
  to: number;
}

export interface Insertion {
  /** Replacement text for `[from, to)`. */
  text: string;
  /** Caret or selection after insertion, in the caller's coordinates. */
  selection: Selection;
}

export type InsertionError =
  | { code: 'tableRange'; rows: boolean; columns: boolean }
  | { code: 'equationTooLong'; limit: number };

export type InsertionResult =
  | { ok: true; insertion: Insertion }
  | { ok: false; error: InsertionError };

/** F06 §4: the table helper allows 1–100 rows and 1–30 columns. */
export const TABLE_MIN_ROWS = 1;
export const TABLE_MAX_ROWS = 100;
export const TABLE_MIN_COLUMNS = 1;
export const TABLE_MAX_COLUMNS = 30;
export const DEFAULT_TABLE_ROWS = 3;
export const DEFAULT_TABLE_COLUMNS = 3;

/** F06-R09: at most 8192 Unicode code points per equation. */
export const MAX_EQUATION_CHARS = 8192;

function caretAt(offset: number): Selection {
  return { from: offset, to: offset };
}

function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.min(max, Math.max(min, Math.trunc(value)));
}

export function validateTableSize(rows: number, columns: number): InsertionError | null {
  const rowsBad = !Number.isInteger(rows) || rows < TABLE_MIN_ROWS || rows > TABLE_MAX_ROWS;
  const columnsBad =
    !Number.isInteger(columns) || columns < TABLE_MIN_COLUMNS || columns > TABLE_MAX_COLUMNS;
  return rowsBad || columnsBad ? { code: 'tableRange', rows: rowsBad, columns: columnsBad } : null;
}

export function headingInsertion(selection: Selection, level: number, text: string): Insertion {
  const marker = '#'.repeat(clamp(level, 1, 6));
  const inserted = `${marker} ${text.trim()}`;
  return { text: inserted, selection: caretAt(selection.from + inserted.length) };
}

/**
 * Wraps the selection in markers. With no selection the caret goes between the
 * markers with `fallback` text selected, so the user types over a placeholder
 * rather than deleting one (F06 §5).
 */
export function wrapText(
  selection: Selection,
  selected: string,
  open: string,
  close: string,
  fallback = '',
): Insertion {
  const inner = selected.length > 0 ? selected : fallback;
  const inserted = `${open}${inner}${close}`;
  const innerStart = selection.from + open.length;
  return {
    text: inserted,
    selection:
      selected.length > 0
        ? { from: innerStart, to: innerStart + inner.length }
        : { from: innerStart, to: innerStart + inner.length },
  };
}

function alignmentMarker(align: 'left' | 'center' | 'right' | null): string {
  if (align === 'left') return ':---';
  if (align === 'center') return ':---:';
  if (align === 'right') return '---:';
  return '---';
}

/**
 * Builds a GFM table with an empty header and empty body cells. The caret lands
 * inside the first header cell so the user can type immediately; no placeholder
 * text is inserted because it would become note content (F06 §5).
 */
export function tableInsertion(
  selection: Selection,
  rows: number,
  columns: number,
  align: readonly ('left' | 'center' | 'right' | null)[] = [],
): InsertionResult {
  const error = validateTableSize(rows, columns);
  if (error !== null) {
    return { ok: false, error };
  }
  const header = `|${Array.from({ length: columns }, () => '  ').join('|')}|`;
  const delimiter = `|${Array.from({ length: columns }, (_unused, index) =>
    ` ${alignmentMarker(align[index] ?? null)} `,
  ).join('|')}|`;
  const bodyRow = `|${Array.from({ length: columns }, () => '  ').join('|')}|`;
  const lines = [header, delimiter, ...Array.from({ length: rows }, () => bodyRow)];
  const text = lines.join('\n');
  // Inside the first cell: after `|` and one space.
  return { ok: true, insertion: { text, selection: caretAt(selection.from + 2) } };
}

/**
 * F06 §4/R08: inline math is `$...$` on one line; display math needs `$$` alone
 * on its own lines. Newlines in the body are folded for inline math so an
 * inline equation can never swallow the next paragraph.
 */
export function equationInsertion(
  selection: Selection,
  body: string,
  display: boolean,
): InsertionResult {
  if (Array.from(body).length > MAX_EQUATION_CHARS) {
    return { ok: false, error: { code: 'equationTooLong', limit: MAX_EQUATION_CHARS } };
  }
  if (display) {
    const content = body.trim();
    const inserted = `$$\n${content}\n$$`;
    return {
      ok: true,
      insertion: { text: inserted, selection: caretAt(selection.from + 3 + content.length) },
    };
  }
  const content = body.replace(/[\r\n]+/g, ' ').trim();
  const inserted = `$${content}$`;
  return { ok: true, insertion: { text: inserted, selection: caretAt(selection.from + 1) } };
}

/** Fenced code with an optional language; an unknown language is only a label. */
export function codeInsertion(
  selection: Selection,
  language: string,
  body: string,
): Insertion {
  // Only characters that would break the fence line are removed; a multi-word
  // info string is valid CommonMark, and the language is still just a label.
  const info = language.replace(/[`\r\n]/g, '').trim();
  const content = body.replace(/\r\n?/g, '\n');
  const fence = '```';
  const inserted = `${fence}${info}\n${content}\n${fence}`;
  // Put the caret at the start of the code body, not after the closing fence.
  return { text: inserted, selection: caretAt(selection.from + fence.length + info.length + 1) };
}

export function taskInsertion(selection: Selection, text: string): Insertion {
  const inserted = `- [ ] ${text.trim()}`;
  return { text: inserted, selection: caretAt(selection.from + inserted.length) };
}

/**
 * A link whose label is the selected text. The target is supplied by the caller:
 * F19 owns attachment paths, so this helper never invents one.
 */
export function linkInsertion(selection: Selection, selected: string, target: string): Insertion {
  const label = selected.trim().length > 0 ? selected : target.trim();
  const inserted = `[${label}](${target.trim()})`;
  const targetStart = selection.from + label.length + 3;
  return {
    text: inserted,
    selection: { from: targetStart, to: targetStart + target.trim().length },
  };
}

export function textInsertion(selection: Selection, text: string): Insertion {
  return { text, selection: caretAt(selection.from + text.length) };
}

/** One slash-menu entry. Labels are localized by the UI; aliases stay stable. */
export interface SlashItem {
  kind: FragmentKind;
  aliases: readonly string[];
  /** Digit shortcut shown in S11 (1–9). */
  digit: number;
}

export const SLASH_ITEMS: readonly SlashItem[] = [
  { kind: 'text', aliases: ['text', 'paragraph'], digit: 1 },
  { kind: 'heading', aliases: ['heading', 'title', 'h1', 'h2'], digit: 2 },
  { kind: 'task', aliases: ['task', 'todo', 'checkbox'], digit: 3 },
  { kind: 'table', aliases: ['table', 'grid'], digit: 4 },
  { kind: 'equation', aliases: ['equation', 'math', 'formula'], digit: 5 },
  { kind: 'code', aliases: ['code', 'fence'], digit: 6 },
  { kind: 'link', aliases: ['link'], digit: 7 },
];

/**
 * Detects an open slash query: a `/` at the line start or after whitespace, with
 * no whitespace before the caret. `from` is the offset of that `/`, so applying
 * the insertion removes the typed command instead of leaving it in the note.
 */
export function detectSlashQuery(
  source: string,
  caret: number,
): { from: number; query: string } | null {
  if (caret < 0 || caret > source.length) {
    return null;
  }
  const lineStart = source.lastIndexOf('\n', Math.max(0, caret - 1)) + 1;
  const linePrefix = source.slice(lineStart, caret);
  const slash = linePrefix.lastIndexOf('/');
  if (slash < 0) {
    return null;
  }
  const absolute = lineStart + slash;
  const before = absolute === 0 ? '' : (source[absolute - 1] ?? '');
  if (before !== '' && !/\s/.test(before)) {
    return null;
  }
  const query = source.slice(absolute + 1, caret);
  if (/\s/.test(query)) {
    return null;
  }
  return { from: absolute, query };
}

/** Ranks menu items for a query; an empty query keeps the S11 order. */
export function rankSlashItems(
  query: string,
  labels: Readonly<Record<FragmentKind, string>>,
): SlashItem[] {
  const needle = query.trim().toLocaleLowerCase();
  if (needle === '') {
    return [...SLASH_ITEMS];
  }
  const scored: { item: SlashItem; score: number }[] = [];
  for (const item of SLASH_ITEMS) {
    let best: number | null = null;
    for (const candidate of [labels[item.kind], item.kind, ...item.aliases]) {
      const haystack = (candidate ?? '').toLocaleLowerCase();
      if (haystack === '') {
        continue;
      }
      const index = haystack.indexOf(needle);
      if (index < 0) {
        continue;
      }
      const score = haystack === needle ? 0 : index === 0 ? 1 : 2;
      if (best === null || score < best) {
        best = score;
      }
    }
    if (best !== null) {
      scored.push({ item, score: best });
    }
  }
  scored.sort((left, right) => left.score - right.score || left.item.digit - right.item.digit);
  return scored.map((entry) => entry.item);
}

/** Items to show for the typed query; empty query keeps the S11 order. */
export function visibleSlashItems(
  query: string,
  labels: Readonly<Record<FragmentKind, string>>,
): SlashItem[] {
  return rankSlashItems(query, labels);
}
