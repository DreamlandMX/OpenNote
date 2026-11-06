/**
 * Pure state for the PDF pane (F29-R02 / LaTeX plan step 4).
 *
 * Kept free of pdf.js so it can be tested without a canvas: page clamping,
 * zoom steps, and text search over extracted page text.
 */

export const ZOOM_STEPS: readonly number[] = [0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4];
export const MIN_ZOOM = ZOOM_STEPS[0]!;
export const MAX_ZOOM = ZOOM_STEPS[ZOOM_STEPS.length - 1]!;

export function clampPage(page: number, pageCount: number): number {
  if (pageCount < 1 || !Number.isFinite(page)) return 1;
  return Math.min(pageCount, Math.max(1, Math.trunc(page)));
}

export function clampZoom(zoom: number): number {
  if (!Number.isFinite(zoom)) return 1;
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom));
}

/** Next zoom step in a direction; from an in-between value it snaps to the nearest step beyond it. */
export function stepZoom(zoom: number, direction: 1 | -1): number {
  const current = clampZoom(zoom);
  if (direction > 0) return ZOOM_STEPS.find((step) => step > current + 1e-6) ?? MAX_ZOOM;
  return [...ZOOM_STEPS].reverse().find((step) => step < current - 1e-6) ?? MIN_ZOOM;
}

/** Scale that fits a page of `pageWidth` PDF points into `containerWidth` CSS pixels. */
export function fitWidthZoom(containerWidth: number, pageWidth: number, gutter = 32): number {
  if (!(containerWidth > 0) || !(pageWidth > 0)) return 1;
  return clampZoom((containerWidth - gutter) / pageWidth);
}

export interface SearchHit {
  /** 1-based page number. */
  page: number;
  /** Offset of the match in that page's extracted text. */
  offset: number;
}

/** Normalises text for search: case-folded, whitespace collapsed (PDF text runs split words oddly). */
export function normalizeForSearch(text: string): string {
  return text.normalize('NFKC').toLocaleLowerCase().replace(/\s+/g, ' ');
}

/**
 * Every occurrence of `query` across pages, in reading order. An empty or
 * whitespace-only query has no hits rather than matching everything.
 */
export function findHits(pageTexts: readonly string[], query: string, limit = 1000): SearchHit[] {
  const needle = normalizeForSearch(query).trim();
  if (needle === '') return [];
  const hits: SearchHit[] = [];
  pageTexts.forEach((text, index) => {
    const haystack = normalizeForSearch(text);
    let from = 0;
    while (hits.length < limit) {
      const found = haystack.indexOf(needle, from);
      if (found < 0) break;
      hits.push({ page: index + 1, offset: found });
      from = found + Math.max(1, needle.length);
    }
  });
  return hits;
}

/** Index of the next/previous hit, wrapping; -1 when there are no hits. */
export function cycleHit(current: number, count: number, direction: 1 | -1): number {
  if (count <= 0) return -1;
  if (current < 0) return direction > 0 ? 0 : count - 1;
  return (current + direction + count) % count;
}

/** Which version of the document the pane is showing, relative to the source. */
export type PdfFreshness = 'current' | 'outdated';

export const MAX_PDF_BYTES = 128 * 1024 * 1024;
