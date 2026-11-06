/**
 * Find in the reading pane. The pane is rendered HTML, not an editor, so the
 * matches are DOM ranges over its text. They are shown with the CSS Custom
 * Highlight API, which paints ranges without changing the rendered markup.
 */

const ALL = 'opennote-read-find';
const CURRENT = 'opennote-read-find-current';

/** Text nodes whose words a reader sees; formula sources and scripts are skipped. */
function visibleTextNodes(root: Node): Text[] {
  const nodes: Text[] = [];
  const walker = root.ownerDocument!.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      for (let parent = node.parentElement; parent !== null && parent !== root; parent = parent.parentElement) {
        const tag = parent.tagName;
        if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'ANNOTATION' || parent.classList.contains('katex-mathml')) {
          return NodeFilter.FILTER_REJECT;
        }
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) nodes.push(node as Text);
  return nodes;
}

/** A match as positions in a list of text pieces: [piece index, offset]. */
export interface TextSpan {
  start: [number, number];
  end: [number, number];
}

/**
 * Every occurrence of `query` across consecutive text pieces, in order. A match
 * may span pieces, as `foo **bar**` renders "foo " and "bar" separately.
 */
export function matchSpans(pieces: readonly string[], query: string, matchCase: boolean): TextSpan[] {
  if (query === '' || pieces.length === 0) return [];
  const starts: number[] = [];
  let text = '';
  for (const piece of pieces) {
    starts.push(text.length);
    text += piece;
  }
  const folded = matchCase ? text : text.toLocaleLowerCase();
  const needle = matchCase ? query : query.toLocaleLowerCase();
  // Lower-casing changes length for a few characters; then match exactly.
  const sameLength = folded.length === text.length && needle.length === query.length;
  const haystack = sameLength ? folded : text;
  const target = sameLength ? needle : query;

  // The piece holding `offset`; an end offset belongs to the piece it closes.
  const locate = (offset: number, end: boolean): [number, number] => {
    let low = 0;
    let high = pieces.length - 1;
    while (low < high) {
      const middle = Math.ceil((low + high) / 2);
      const start = starts[middle]!;
      if (end ? start < offset : start <= offset) low = middle;
      else high = middle - 1;
    }
    return [low, offset - starts[low]!];
  };

  const spans: TextSpan[] = [];
  for (let index = haystack.indexOf(target); index !== -1; index = haystack.indexOf(target, index + target.length)) {
    spans.push({ start: locate(index, false), end: locate(index + target.length, true) });
  }
  return spans;
}

/** Every occurrence of `query` in the pane's visible text, as DOM ranges. */
export function findTextRanges(root: Node, query: string, matchCase: boolean): Range[] {
  const nodes = visibleTextNodes(root);
  return matchSpans(nodes.map((node) => node.data), query, matchCase).map((span) => {
    const range = root.ownerDocument!.createRange();
    range.setStart(nodes[span.start[0]]!, span.start[1]);
    range.setEnd(nodes[span.end[0]]!, span.end[1]);
    return range;
  });
}

interface HighlightRegistry {
  set(name: string, highlight: unknown): void;
  delete(name: string): void;
}

function registry(): { highlights: HighlightRegistry; Highlight: new (...ranges: Range[]) => unknown } | null {
  const css = (globalThis as { CSS?: { highlights?: HighlightRegistry } }).CSS;
  const Highlight = (globalThis as { Highlight?: new (...ranges: Range[]) => unknown }).Highlight;
  return css?.highlights && Highlight ? { highlights: css.highlights, Highlight } : null;
}

/** Paints all matches and the current one; returns false where unsupported. */
export function paintMatches(ranges: readonly Range[], current: number): boolean {
  const api = registry();
  if (api === null) return false;
  api.highlights.set(ALL, new api.Highlight(...ranges));
  const active = ranges[current];
  if (active) api.highlights.set(CURRENT, new api.Highlight(active));
  else api.highlights.delete(CURRENT);
  return true;
}

export function clearMatches(): void {
  const api = registry();
  api?.highlights.delete(ALL);
  api?.highlights.delete(CURRENT);
}

/** The index after a step, wrapping at both ends; -1 when there is nothing. */
export function stepMatch(current: number, total: number, direction: 1 | -1): number {
  if (total === 0) return -1;
  if (current < 0) return direction === 1 ? 0 : total - 1;
  return (current + direction + total) % total;
}

/** Scrolls the reading pane so the match is in view. */
export function revealMatch(range: Range | undefined): void {
  const element = range?.startContainer.parentElement;
  element?.scrollIntoView?.({ block: 'center', inline: 'nearest' });
}
