/**
 * Editor context-menu edits. Each helper turns the current selection into one
 * replacement the caller applies as a single transaction, so one undo restores
 * the original text. Like `insert.ts`, nothing here touches the editor view.
 */

export type InlineMarker = '**' | '*' | '~~' | '==' | '%%' | '`' | '$';

export type BlockKind =
  | 'paragraph'
  | 'heading1'
  | 'heading2'
  | 'heading3'
  | 'quote'
  | 'bullet'
  | 'numbered'
  | 'task'
  | 'codeBlock';

export interface Range {
  from: number;
  to: number;
}

/** Replace `[from, to)` with `insert`, then select `selection` (new coordinates). */
export interface Edit {
  from: number;
  to: number;
  insert: string;
  selection: Range;
}

/** Length of the run of `char` that ends at `end` (exclusive). */
function runBefore(source: string, end: number, char: string): number {
  let count = 0;
  while (end - count - 1 >= 0 && source[end - count - 1] === char) count += 1;
  return count;
}

/** Length of the run of `char` that starts at `start`. */
function runAfter(source: string, start: number, char: string): number {
  let count = 0;
  while (start + count < source.length && source[start + count] === char) count += 1;
  return count;
}

/**
 * True when a run of `*` holds `marker`. A run of 1 or 3 is italic and a run of
 * 2 or 3 is bold, so toggling italic on `**bold**` adds a marker instead of
 * stealing one from the bold pair.
 */
function starRunHas(run: number, marker: InlineMarker): boolean {
  return marker === '*' ? run === 1 || run === 3 : run >= 2;
}

function surrounds(source: string, range: Range, marker: InlineMarker): boolean {
  if (marker === '*' || marker === '**') {
    return starRunHas(runBefore(source, range.from, '*'), marker)
      && starRunHas(runAfter(source, range.to, '*'), marker);
  }
  return range.from >= marker.length
    && source.slice(range.from - marker.length, range.from) === marker
    && source.slice(range.to, range.to + marker.length) === marker;
}

function wrapsItself(text: string, marker: InlineMarker): boolean {
  if (text.length < marker.length * 2) return false;
  if (marker === '*' || marker === '**') {
    return starRunHas(runAfter(text, 0, '*'), marker) && starRunHas(runBefore(text, text.length, '*'), marker);
  }
  return text.startsWith(marker) && text.endsWith(marker);
}

/**
 * Adds or removes one inline marker around the selection. The marker is
 * removed when it sits just outside the selection or at both ends of it;
 * otherwise the selection is wrapped and stays selected inside the markers.
 */
export function toggleInline(source: string, range: Range, marker: InlineMarker): Edit {
  const length = marker.length;
  const selected = source.slice(range.from, range.to);
  if (surrounds(source, range, marker)) {
    const from = range.from - length;
    return {
      from,
      to: range.to + length,
      insert: selected,
      selection: { from, to: from + selected.length },
    };
  }
  if (wrapsItself(selected, marker)) {
    const inner = selected.slice(length, selected.length - length);
    return {
      from: range.from,
      to: range.to,
      insert: inner,
      selection: { from: range.from, to: range.from + inner.length },
    };
  }
  return {
    from: range.from,
    to: range.to,
    insert: `${marker}${selected}${marker}`,
    selection: { from: range.from + length, to: range.from + length + selected.length },
  };
}

const INLINE_MARKERS = /\*\*|~~|==|%%|[*`$]/g;

/**
 * Removes inline markers inside the selection, and the markers directly around
 * it. Block structure (headings, lists, quotes) is left alone.
 */
export function clearInline(source: string, range: Range): Edit {
  let from = range.from;
  let to = range.to;
  // Widen over markers that hug the selection, such as `**` in `**|bold|**`.
  for (;;) {
    const before = source.slice(Math.max(0, from - 2), from);
    const after = source.slice(to, to + 2);
    const pair = ['**', '~~', '==', '%%'].find(marker => before === marker && after === marker)
      ?? ['*', '`', '$'].find(marker => before.endsWith(marker) && after.startsWith(marker));
    if (pair === undefined) break;
    from -= pair.length;
    to += pair.length;
  }
  const insert = source.slice(from, to).replace(INLINE_MARKERS, '');
  return { from, to, insert, selection: { from, to: from + insert.length } };
}

const BLOCK_PREFIX = /^(\s*)(?:#{1,6}\s+|>\s?|[-*+]\s+\[[ xX]\]\s+|[-*+]\s+|\d+[.)]\s+)?/;

function prefixFor(kind: BlockKind, index: number): string {
  switch (kind) {
    case 'heading1': return '# ';
    case 'heading2': return '## ';
    case 'heading3': return '### ';
    case 'quote': return '> ';
    case 'bullet': return '- ';
    case 'numbered': return `${index + 1}. `;
    case 'task': return '- [ ] ';
    default: return '';
  }
}

/**
 * Turns every line the selection touches into `kind`, replacing the block
 * marker each line already had. A code block wraps the lines in a fence
 * instead. A caret ends up at the end of the block; a selection covers it.
 */
export function convertBlock(source: string, range: Range, kind: BlockKind): Edit {
  const from = source.lastIndexOf('\n', range.from - 1) + 1;
  const lineEnd = source.indexOf('\n', range.to);
  const to = lineEnd === -1 ? source.length : lineEnd;
  const lines = source.slice(from, to).split('\n');
  let insert: string;
  if (kind === 'codeBlock') {
    insert = ['```', ...lines, '```'].join('\n');
  } else {
    insert = lines
      .map((line, index) => line.replace(BLOCK_PREFIX, (_whole, indent: string) => indent + prefixFor(kind, index)))
      .join('\n');
  }
  const selection = range.from === range.to
    ? { from: from + insert.length, to: from + insert.length }
    : { from, to: from + insert.length };
  if (kind === 'codeBlock' && range.from === range.to) {
    // The caret stays on the code line, not after the closing fence.
    const caret = from + insert.length - 4;
    return { from, to, insert, selection: { from: caret, to: caret } };
  }
  return { from, to, insert, selection };
}

/**
 * `[text](url)`: with a selection the caret lands in the empty URL; without
 * one it lands in the empty label.
 */
export function externalLink(source: string, range: Range): Edit {
  const selected = source.slice(range.from, range.to);
  const insert = `[${selected}]()`;
  const caret = selected.length > 0 ? range.from + insert.length - 1 : range.from + 1;
  return { from: range.from, to: range.to, insert, selection: { from: caret, to: caret } };
}

/**
 * `[[text]]` for a selection. Without one it types `[[`, which opens the link
 * picker at the caret.
 */
export function internalLink(source: string, range: Range): Edit {
  const selected = source.slice(range.from, range.to);
  const insert = selected.length > 0 ? `[[${selected}]]` : '[[';
  const caret = range.from + insert.length;
  return { from: range.from, to: range.to, insert, selection: { from: caret, to: caret } };
}

/** Clipboard text as the editor stores it: `\n` line breaks only. */
export function plainText(text: string): string {
  return text.replace(/\r\n?/g, '\n');
}
