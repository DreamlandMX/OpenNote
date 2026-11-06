/**
 * F05: locating the current section across modes.
 *
 * The reading pane is sanitized HTML rendered from the same source, so a source
 * offset cannot be mapped into it directly. What both sides share is the ordered
 * list of headings: the outline already knows their line numbers, and the HTML
 * has one heading element per heading in document order. Matching by index keeps
 * the mapping correct even when two headings share the same text.
 */

export interface HeadingAnchor {
  /** Zero-based source line of the heading. */
  line: number;
  text: string;
}

/**
 * Index of the heading whose section contains `caretLine` (zero-based), or -1
 * before the first heading. The last heading at or before the caret wins, which
 * is what a reader means by "the section I am in".
 */
export function headingIndexAtLine(
  headings: readonly HeadingAnchor[],
  caretLine: number,
): number {
  let found = -1;
  for (let index = 0; index < headings.length; index += 1) {
    const heading = headings[index];
    if (heading === undefined) {
      break;
    }
    if (heading.line <= caretLine) {
      found = index;
    } else {
      break;
    }
  }
  return found;
}

/**
 * Which heading element the reading pane should scroll to.
 *
 * Returns -1 when there is nothing to do: no heading before the caret, or the
 * rendered pane has no headings at all (for example because the note opens with
 * frontmatter only).
 */
export function readPaneTarget(
  headings: readonly HeadingAnchor[],
  headingElements: number,
  caretLine: number,
): number {
  if (headingElements === 0) {
    return -1;
  }
  const index = headingIndexAtLine(headings, caretLine);
  if (index < 0) {
    return -1;
  }
  // A heading list longer than the rendered pane means the two disagree; using
  // the last real element is safer than returning an index that cannot exist.
  return Math.min(index, headingElements - 1);
}

/** Counts the heading elements a rendered HTML string actually produced. */
export function countHeadings(html: string): number {
  const matches = html.match(/<h[1-6][\s>]/gi);
  return matches === null ? 0 : matches.length;
}
