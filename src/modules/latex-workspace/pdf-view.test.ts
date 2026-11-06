import { describe, expect, it } from 'vitest';
import { clampPage, clampZoom, cycleHit, findHits, fitWidthZoom, stepZoom, MAX_ZOOM, MIN_ZOOM } from './pdf-view';

describe('pdf pane state', () => {
  it('keeps the page inside the document', () => {
    expect(clampPage(0, 5)).toBe(1);
    expect(clampPage(9, 5)).toBe(5);
    expect(clampPage(2.7, 5)).toBe(2);
    expect(clampPage(Number.NaN, 5)).toBe(1);
    expect(clampPage(3, 0)).toBe(1);
  });

  it('steps zoom through fixed levels and stops at the ends', () => {
    expect(stepZoom(1, 1)).toBe(1.1);
    expect(stepZoom(1, -1)).toBe(0.9);
    expect(stepZoom(1.3, 1)).toBe(1.5);
    expect(stepZoom(1.3, -1)).toBe(1.25);
    expect(stepZoom(MAX_ZOOM, 1)).toBe(MAX_ZOOM);
    expect(stepZoom(MIN_ZOOM, -1)).toBe(MIN_ZOOM);
    expect(clampZoom(99)).toBe(MAX_ZOOM);
  });

  it('fits page width with a gutter and survives a zero-sized container', () => {
    expect(fitWidthZoom(644, 612)).toBeCloseTo(1, 5);
    expect(fitWidthZoom(0, 612)).toBe(1);
  });

  it('finds matches across pages, ignoring case and broken whitespace', () => {
    const pages = ['Theorem 1. Let x be', 'see  THEOREM\n2 and theorem 3', ''];
    expect(findHits(pages, 'theorem')).toEqual([
      { page: 1, offset: 0 }, { page: 2, offset: 4 }, { page: 2, offset: 18 },
    ]);
    expect(findHits(pages, 'theorem 2')).toEqual([{ page: 2, offset: 4 }]);
    expect(findHits(pages, '   ')).toEqual([]);
    expect(findHits(['定理 定理'], '定理')).toHaveLength(2);
    expect(findHits(['aaaa'], 'a', 2)).toHaveLength(2);
  });

  it('cycles through hits and wraps', () => {
    expect(cycleHit(-1, 3, 1)).toBe(0);
    expect(cycleHit(-1, 3, -1)).toBe(2);
    expect(cycleHit(2, 3, 1)).toBe(0);
    expect(cycleHit(0, 3, -1)).toBe(2);
    expect(cycleHit(0, 0, 1)).toBe(-1);
  });
});
