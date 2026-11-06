import { describe, expect, it } from 'vitest';
import {
  appearanceStyle,
  bodySizeLabel,
  clampBodySize,
  clampLineMeasure,
  DEFAULT_BODY_SIZE,
  DEFAULT_CJK_MEASURE,
  DEFAULT_LATIN_MEASURE,
  defaultMeasureFor,
  lineMeasureLabel,
  MAX_BODY_SIZE,
  MAX_LINE_MEASURE,
  MIN_BODY_SIZE,
  MIN_LINE_MEASURE,
  resolveAppearance,
} from './appearance';

describe('reading appearance', () => {
  it('uses a narrower measure for CJK locales than for Latin ones', () => {
    for (const locale of ['zh-CN', 'zh-TW', 'ja', 'ko']) {
      expect(defaultMeasureFor(locale)).toBe(DEFAULT_CJK_MEASURE);
    }
    for (const locale of ['en', 'de', 'ar', 'hi', 'vi']) {
      expect(defaultMeasureFor(locale)).toBe(DEFAULT_LATIN_MEASURE);
    }
  });

  it('clamps body size into the documented 12–32 px range', () => {
    expect(clampBodySize(11)).toBe(MIN_BODY_SIZE);
    expect(clampBodySize(33)).toBe(MAX_BODY_SIZE);
    expect(clampBodySize(18.6)).toBe(19);
    expect(clampBodySize(Number.NaN)).toBe(DEFAULT_BODY_SIZE);
  });

  it('clamps the line measure into 30–80 ch', () => {
    expect(clampLineMeasure(1)).toBe(MIN_LINE_MEASURE);
    expect(clampLineMeasure(999)).toBe(MAX_LINE_MEASURE);
    expect(clampLineMeasure(64.4)).toBe(64);
    expect(clampLineMeasure(Number.POSITIVE_INFINITY)).toBe(MAX_LINE_MEASURE);
    expect(clampLineMeasure(Number.NaN)).toBe(DEFAULT_LATIN_MEASURE);
  });

  it('falls back to the script default when nothing is stored', () => {
    expect(resolveAppearance('ja', {})).toEqual({
      bodySize: DEFAULT_BODY_SIZE,
      lineMeasure: DEFAULT_CJK_MEASURE,
    });
    expect(resolveAppearance('en', {})).toEqual({
      bodySize: DEFAULT_BODY_SIZE,
      lineMeasure: DEFAULT_LATIN_MEASURE,
    });
  });

  it('clamps a stored out-of-range preference instead of ignoring it', () => {
    expect(resolveAppearance('en', { bodySize: 100, lineMeasure: 5 })).toEqual({
      bodySize: MAX_BODY_SIZE,
      lineMeasure: MIN_LINE_MEASURE,
    });
  });

  it('ignores a stored value of the wrong type', () => {
    expect(resolveAppearance('zh-CN', { bodySize: '20', lineMeasure: null })).toEqual({
      bodySize: DEFAULT_BODY_SIZE,
      lineMeasure: DEFAULT_CJK_MEASURE,
    });
  });

  it('emits only CSS variables so no new layout path is introduced', () => {
    const style = appearanceStyle({ bodySize: 20, lineMeasure: 55 });
    expect(style).toEqual({
      '--body-font-size': '20px',
      '--reading-measure': '55ch',
      '--body-line-height': '33px',
    });
    // An out-of-range input is still clamped at the boundary.
    expect(appearanceStyle({ bodySize: 200, lineMeasure: 200 })).toEqual({
      '--body-font-size': '32px',
      '--reading-measure': '80ch',
      '--body-line-height': '52px',
    });
  });

  it('counts a CJK measure in full-width characters, not half-width ones', () => {
    expect(appearanceStyle({ bodySize: 16, lineMeasure: DEFAULT_CJK_MEASURE }, 'zh-CN')['--reading-measure']).toBe('40em');
    expect(appearanceStyle({ bodySize: 16, lineMeasure: DEFAULT_LATIN_MEASURE }, 'en')['--reading-measure']).toBe('65ch');
  });

  it('formats labels without inventing precision', () => {
    expect(bodySizeLabel(17.4)).toBe('17 px');
    expect(bodySizeLabel(200)).toBe('32 px');
    expect(lineMeasureLabel(64.5)).toBe('65 ch');
  });
});
