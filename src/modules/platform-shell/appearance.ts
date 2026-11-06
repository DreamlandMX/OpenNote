/**
 * F29 reading appearance.
 *
 * Body size and line measure are device preferences: they change how the app
 * renders, never a byte of the note (F29-R04). The limits come from the spec so
 * the UI and the model cannot disagree about them.
 */

export interface ReadingAppearance {
  /** Body text size in CSS pixels. */
  bodySize: number;
  /** Maximum line width in characters of the interface's script. */
  lineMeasure: number;
}

/** F29 §4: body size is 12–32 px; controls never shrink with it. */
export const MIN_BODY_SIZE = 12;
export const MAX_BODY_SIZE = 32;
export const DEFAULT_BODY_SIZE = 16;

/**
 * F29 §4: the comfortable measure is about 40 CJK characters or 65 Latin ones.
 * The value counts characters of the interface's script: `ch` (the width of
 * "0") for Latin text, `em` for CJK, whose glyphs are a full em wide. Using
 * `ch` for CJK would halve the line to about 20 characters.
 */
export const MIN_LINE_MEASURE = 30;
export const MAX_LINE_MEASURE = 80;
export const DEFAULT_LATIN_MEASURE = 65;
export const DEFAULT_CJK_MEASURE = 40;

/** Scripts whose text is set with a narrower measure for readability. */
const CJK_LOCALES: readonly string[] = ['zh-CN', 'zh-TW', 'ja', 'ko'];

export function defaultMeasureFor(locale: string): number {
  return CJK_LOCALES.includes(locale) ? DEFAULT_CJK_MEASURE : DEFAULT_LATIN_MEASURE;
}

export function clampBodySize(value: number): number {
  if (Number.isNaN(value)) {
    return DEFAULT_BODY_SIZE;
  }
  return Math.min(MAX_BODY_SIZE, Math.max(MIN_BODY_SIZE, Math.round(value)));
}

export function clampLineMeasure(value: number): number {
  // NaN has no measure to honour, so the script-neutral default is used; an
  // infinite value is clamped like any other out-of-range input.
  if (Number.isNaN(value)) {
    return DEFAULT_LATIN_MEASURE;
  }
  return Math.min(MAX_LINE_MEASURE, Math.max(MIN_LINE_MEASURE, Math.round(value)));
}

/**
 * Resolves stored preferences into values the app can apply.
 *
 * A missing or malformed value falls back to the script-appropriate default
 * instead of throwing or producing an unusable layout, and out-of-range values
 * are clamped rather than ignored (a stored 100 px note would otherwise be
 * unreadable with no way to tell why).
 */
export function resolveAppearance(
  locale: string,
  stored: { bodySize?: unknown; lineMeasure?: unknown },
): ReadingAppearance {
  const bodySize =
    typeof stored.bodySize === 'number'
      ? clampBodySize(stored.bodySize)
      : DEFAULT_BODY_SIZE;
  const lineMeasure =
    typeof stored.lineMeasure === 'number'
      ? clampLineMeasure(stored.lineMeasure)
      : defaultMeasureFor(locale);
  return { bodySize, lineMeasure };
}

/**
 * CSS custom properties for the reading surface.
 *
 * Only variables are set: the stylesheet keeps its layout rules, so a preference
 * can never introduce a new layout path that was not designed.
 */
export function appearanceStyle(appearance: ReadingAppearance, locale = 'en'): Record<string, string> {
  const unit = CJK_LOCALES.includes(locale) ? 'em' : 'ch';
  return {
    '--body-font-size': `${clampBodySize(appearance.bodySize)}px`,
    '--reading-measure': `${clampLineMeasure(appearance.lineMeasure)}${unit}`,
    // 26/16 is the reading line-height the tokens already use, so the ratio
    // stays constant as the body grows instead of drifting from the design.
    '--body-line-height': `${Math.round(clampBodySize(appearance.bodySize) * 1.625)}px`,
  };
}

/** A short label for the settings slider, e.g. `16 px`. */
export function bodySizeLabel(size: number): string {
  return `${clampBodySize(size)} px`;
}

export function lineMeasureLabel(measure: number): string {
  return `${clampLineMeasure(measure)} ch`;
}
