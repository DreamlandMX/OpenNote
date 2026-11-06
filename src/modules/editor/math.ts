/**
 * F06 math detection and limits.
 *
 * This owns the boundaries checked before the isolated renderer runs,
 * because getting it wrong corrupts ordinary text:
 *
 * * `$5 and $10` is money, not a formula. Treating the second `$` as a closing
 *   delimiter swallows the words between them (F06-R02);
 * * a code span or fence is code, so a `$` inside it is literal (F06-A02);
 * * an over-long formula or one over the nesting limit is shown as source with a
 *   fixable hint rather than rendered (F06-R09);
 * * macros are scoped to one formula, so they cannot leak into another note
 *   (F06-R09).
 *
 * These are pure functions over text: no dependency, no DOM.
 */

/** The KaTeX limits the spec fixes (F06-R09). */
export const MAX_FORMULA_CODE_POINTS = 8192;
export const MAX_EXPAND = 1000;
export const MAX_SIZE_EM = 20;
export const RENDER_BUDGET_MS = 500;

export type MathDelimiter = 'inline' | 'block';

export interface MathSpan {
  /** The TeX between the delimiters, trimmed for block spans. */
  tex: string;
  /** Byte-free character offsets into the source, so callers can replace it. */
  start: number;
  end: number;
  delimiter: MathDelimiter;
}

export type MathSkip =
  | { kind: 'codeSpan'; start: number; end: number }
  | { kind: 'fence'; start: number; end: number }
  | { kind: 'escaped'; start: number };

function escapedAt(source: string, index: number): boolean {
  let count = 0;
  while (index > 0 && source[--index] === '\\') count += 1;
  return count % 2 === 1;
}

function standaloneDollars(source: string, index: number): boolean {
  const start = source.lastIndexOf('\n', index - 1) + 1;
  const end = source.indexOf('\n', index);
  return source.slice(start, index).trim() === '' &&
    source.slice(index + 2, end < 0 ? source.length : end).trim() === '';
}

/**
 * Finds the regions where a `$` is literal rather than a delimiter.
 *
 * A code fence runs to its closing fence (or the end of the text, because an
 * unterminated fence is still code as far as the author is concerned). An escaped
 * `\$` is literal by definition.
 */
export function skipRegions(source: string): MathSkip[] {
  const regions: MathSkip[] = [];
  const lines = source.split('\n');
  let offset = 0;
  let fenceStart: number | null = null;
  let fenceMarker: string | null = null;

  for (const line of lines) {
    const trimmed = line.trimStart();
    const fence = /^(`{3,}|~{3,})/.exec(trimmed);
    if (fenceStart === null && fence !== null) {
      fenceStart = offset;
      fenceMarker = fence[1] ?? '```';
    } else if (fenceStart !== null && fence !== null &&
      fence[1]?.[0] === fenceMarker?.[0] && (fence[1]?.length ?? 0) >= (fenceMarker?.length ?? 3) &&
      trimmed.slice(fence[0].length).trim() === '') {
      regions.push({ kind: 'fence', start: fenceStart, end: offset + line.length });
      fenceStart = null;
      fenceMarker = null;
    }
    offset += line.length + 1;
  }
  // An unclosed fence masks the rest of the note, which is what a reader sees.
  if (fenceStart !== null) {
    regions.push({ kind: 'fence', start: fenceStart, end: source.length });
  }

  // Inline code spans and escapes, scanning only outside fences.
  let index = 0;
  while (index < source.length) {
    const inFence = regions.some(
      (region) => region.kind === 'fence' && index >= region.start && index < region.end,
    );
    if (inFence) {
      const fence = regions.find(
        (region) =>
          region.kind === 'fence' && index >= region.start && index < region.end,
      );
      // Only a fence has an `end`; the guard above already proved this one is.
      index = fence !== undefined && fence.kind === 'fence' ? fence.end : index + 1;
      continue;
    }
    const character = source[index];
    if (character === '$' && escapedAt(source, index)) {
      regions.push({ kind: 'escaped', start: index - 1 });
      index += 1;
      continue;
    }
    if (character === '`') {
      const run = /^`+/.exec(source.slice(index))?.[0] ?? '`';
      let close = source.indexOf(run, index + run.length);
      while (close !== -1 && (source[close - 1] === '`' || source[close + run.length] === '`')) {
        close = source.indexOf(run, close + run.length);
      }
      if (close !== -1) {
        regions.push({ kind: 'codeSpan', start: index, end: close + run.length });
        index = close + run.length;
        continue;
      }
    }
    index += 1;
  }
  return regions.sort((left, right) => left.start - right.start);
}

function insideSkip(regions: readonly MathSkip[], index: number): boolean {
  return regions.some((region) => {
    if (region.kind === 'escaped') {
      return index === region.start || index === region.start + 1;
    }
    return index >= region.start && index < region.end;
  });
}

/**
 * Whether a `$` at this position may open a formula.
 *
 * Two conditions keep prices readable: the character after `$` must not be
 * whitespace, and a closing `$` must not be preceded by whitespace. `$5 and $10`
 * therefore has no valid closer and stays as text (F06-R02).
 */
function canOpenInline(source: string, index: number): boolean {
  const next = source[index + 1];
  return next !== undefined && next !== '$' && !/\s/.test(next);
}

function canCloseInline(source: string, index: number): boolean {
  const previous = source[index - 1];
  return previous !== undefined && previous !== '$' && !/\s/.test(previous) &&
    source[index + 1] !== '$' && !/[0-9]/.test(source[index + 1] ?? '');
}

/**
 * Extracts the formulas in a note.
 *
 * `$$…$$` is a block formula; `$…$` is inline. Anything inside a code region, an
 * escape, or with no valid closer stays out of the result, which is what keeps
 * money and code intact.
 */
export function findFormulas(source: string): MathSpan[] {
  const regions = skipRegions(source);
  const spans: MathSpan[] = [];
  let index = 0;
  while (index < source.length) {
    if (source[index] !== '$' || insideSkip(regions, index)) {
      index += 1;
      continue;
    }
    // Block first: `$$` must not be read as an empty inline formula.
    if (source[index + 1] === '$') {
      let close = source.indexOf('$$', index + 2);
      while (close !== -1 && (!standaloneDollars(source, close) || insideSkip(regions, close))) {
        close = source.indexOf('$$', close + 2);
      }
      if (standaloneDollars(source, index) && close !== -1) {
        spans.push({
          tex: source.slice(index + 2, close).trim(),
          start: index,
          end: close + 2,
          delimiter: 'block',
        });
        index = close + 2;
        continue;
      }
      // An unclosed `$$` is literal text, not a formula that ate the rest.
      index += 2;
      continue;
    }
    if (!canOpenInline(source, index)) {
      index += 1;
      continue;
    }
    // An invalid nearest delimiter ends the attempt, keeping prices literal.
    let cursor = index + 1;
    let closed = false;
    while (cursor < source.length) {
      if (source[cursor] === '$' && !insideSkip(regions, cursor)) {
        if (canCloseInline(source, cursor)) {
          spans.push({ tex: source.slice(index + 1, cursor), start: index, end: cursor + 1, delimiter: 'inline' });
          index = cursor + 1;
          closed = true;
        }
        break;
      }
      // A newline ends the search: an inline formula does not span paragraphs,
      // and continuing would let one stray `$` consume many lines.
      if (source[cursor] === '\n' || source[cursor] === '\r') {
        break;
      }
      cursor += 1;
    }
    if (!closed) {
      index += 1;
    }
  }
  return spans;
}

/** Counts Unicode code points, so an emoji costs one and not two. */
export function codePointLength(text: string): number {
  let count = 0;
  for (const _ of text) {
    count += 1;
  }
  return count;
}

export type FormulaLimit =
  | { kind: 'ok' }
  | { kind: 'tooLong'; codePoints: number; limit: number }
  | { kind: 'tooDeep'; opens: number; limit: number }
  | { kind: 'empty' };

/**
 * Checks one formula against the documented limits.
 *
 * `{` nesting is a cheap proxy for macro expansion depth: KaTeX expands macros, and
 * a formula that opens more groups than the limit is the shape that produces a
 * runaway expansion (F06-R09).
 */
export function checkFormula(tex: string): FormulaLimit {
  if (tex.trim() === '') {
    return { kind: 'empty' };
  }
  const codePoints = codePointLength(tex);
  if (codePoints > MAX_FORMULA_CODE_POINTS) {
    return { kind: 'tooLong', codePoints, limit: MAX_FORMULA_CODE_POINTS };
  }
  let depth = 0;
  for (let index = 0; index < tex.length; index += 1) {
    const character = tex[index];
    if (character === '{' && !escapedAt(tex, index)) {
      depth += 1;
      if (depth > MAX_EXPAND) {
        return { kind: 'tooDeep', opens: depth, limit: MAX_EXPAND };
      }
    } else if (character === '}' && !escapedAt(tex, index) && depth > 0) {
      depth -= 1;
    }
  }
  return { kind: 'ok' };
}

/** What the reader sees for one formula. */
export interface FormulaView {
  span: MathSpan;
  limit: FormulaLimit;
  /** True when the renderer may try; false means show the source. */
  renderable: boolean;
  /** The message key explaining why it is not rendered, or null. */
  hint: 'math.hint.tooLong' | 'math.hint.tooDeep' | 'math.hint.empty' | null;
}

/**
 * Prepares every formula for display.
 *
 * A formula over a limit is **shown as source with a hint**, never silently
 * dropped and never rendered anyway: dropping it loses the author's text, and
 * rendering it risks a hang (F06-R09).
 */
export function prepareFormulas(source: string): FormulaView[] {
  return findFormulas(source).map((span) => {
    const limit = checkFormula(span.tex);
    switch (limit.kind) {
      case 'tooLong':
        return { span, limit, renderable: false, hint: 'math.hint.tooLong' };
      case 'tooDeep':
        return { span, limit, renderable: false, hint: 'math.hint.tooDeep' };
      case 'empty':
        return { span, limit, renderable: false, hint: 'math.hint.empty' };
      case 'ok':
      default:
        return { span, limit, renderable: true, hint: null };
    }
  });
}

/**
 * Options used by the isolated KaTeX renderer.
 *
 * `trust: false` blocks `\href` and `\includegraphics`-style commands, `maxExpand`
 * bounds macro expansion, `maxSize` bounds rendered size, and `throwOnError: false`
 * keeps a broken formula on screen as source instead of throwing (F06-R09, F06-R03).
 */
export interface KatexOptions {
  displayMode: boolean;
  trust: false;
  throwOnError: false;
  strict: 'warn';
  maxExpand: number;
  maxSize: number;
  globalGroup: true;
}

export function katexOptions(delimiter: MathDelimiter): KatexOptions {
  return {
    displayMode: delimiter === 'block',
    // Never trust input: this is what disables \href and \url injection.
    trust: false,
    // A parse error must not remove the formula from the page.
    throwOnError: false,
    strict: 'warn',
    maxExpand: MAX_EXPAND,
    maxSize: MAX_SIZE_EM,
    // The renderer supplies a fresh macros object for each formula.
    globalGroup: true,
  };
}

/**
 * Whether a URL is safe to emit in rendered output.
 *
 * Only `http`, `https` and `mailto` are allowed: `javascript:` and `data:` URLs in
 * a note would be executable content (F06-R03).
 */
export function isSafeUrl(url: string): boolean {
  const trimmed = url.trim();
  if (trimmed === '') {
    return false;
  }
  const scheme = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(trimmed);
  if (scheme === null) {
    // A relative URL cannot execute a script in a link.
    return !trimmed.startsWith('//');
  }
  const name = scheme[1]?.toLowerCase() ?? '';
  return name === 'http' || name === 'https' || name === 'mailto';
}

/**
 * Whether a raw HTML tag must be dropped.
 *
 * The renderer never emits user HTML, so this is a guard for the paste path: a
 * pasted `<script>` or `onerror=` would otherwise reach the DOM (F06-R03).
 */
export function isDangerousHtml(text: string): boolean {
  const lowered = text.toLowerCase();
  return (
    /<\s*script\b/.test(lowered) ||
    /<\s*iframe\b/.test(lowered) ||
    /<\s*object\b/.test(lowered) ||
    /<\s*embed\b/.test(lowered) ||
    /\son[a-z]+\s*=/.test(lowered) ||
    lowered.includes('javascript:')
  );
}

/**
 * Whether a render has exceeded its time budget.
 *
 * The caller checks this between formulas, so an expensive note degrades to source
 * instead of blocking the save path (F06-R09).
 */
export function exceedsBudget(startedAt: number, now: number): boolean {
  return now - startedAt > RENDER_BUDGET_MS;
}

/**
 * What to show when the budget is exhausted.
 *
 * The remaining formulas become source, and the reader is told the preview was
 * paused — a partially rendered note that looks complete would be a lie.
 */
export function budgetFallback(views: readonly FormulaView[], fromIndex: number): {
  render: FormulaView[];
  asSource: FormulaView[];
  notice: 'math.budgetExceeded';
} {
  return {
    render: views.slice(0, fromIndex).filter((view) => view.renderable),
    asSource: views.slice(fromIndex),
    notice: 'math.budgetExceeded',
  };
}
