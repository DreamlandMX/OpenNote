/**
 * F31 regional formatting.
 *
 * The rule that matters most: a calendar date is a day, not an instant. Storing
 * `2026-10-20` and formatting it through a time zone would let it become the
 * 19th for a user west of UTC, which is exactly what F31 §4 forbids ("纯日期不转换").
 * So a pure date is formatted from its parts, and only values that carry a time
 * are converted.
 */

export type DateStyle = 'short' | 'medium' | 'long';

export interface RegionalPreferences {
  /** BCP 47 tag used for formatting; independent of the interface language. */
  region: string;
  /** IANA zone; only affects values that carry a time. */
  timeZone: string;
}

/** First day of the week, as ISO 1=Monday … 7=Sunday. */
export interface WeekInfo {
  firstDay: number;
}

// shortcut: CLDR 48 fallback stays pinned; refresh when territory week rules change.
const SUNDAY_FIRST = 'AG AS BD BR BS BT BW BZ CA CO DM DO ET GT GU HK HN ID IL IN IS JM JP KE KH KR LA MH MM MO MT MX MZ NI NP PA PE PH PK PR PT PY SA SG SV TH TT TW UM US VE VI WS YE ZA ZW'.split(' ');
const SATURDAY_FIRST = 'AF BH DJ DZ EG IQ IR JO KW LY OM QA SD SY'.split(' ');

export function firstDayOfWeek(region: string): number {
  try {
    const locale = new Intl.Locale(region.replace(/_/g, '-'));
    const weekLocale = locale as Intl.Locale & { getWeekInfo?: () => WeekInfo; weekInfo?: WeekInfo };
    const firstDay = (weekLocale.getWeekInfo?.() ?? weekLocale.weekInfo)?.firstDay;
    if (firstDay !== undefined && Number.isInteger(firstDay) && firstDay >= 1 && firstDay <= 7) return firstDay;
    const territory = locale.region ?? locale.maximize().region ?? '001';
    if (territory === 'MV') return 5;
    if (SATURDAY_FIRST.includes(territory)) return 6;
    return SUNDAY_FIRST.includes(territory) ? 7 : 1;
  } catch {
    return 1;
  }
}

function pad(value: number): string {
  return String(value).padStart(2, '0');
}

/** Strict positive four-digit year, returned at UTC midnight. */
export function parseCalendarDate(value: string): Date | null {
  const parts = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (parts === null || Number(parts[1]) === 0) return null;
  // ISO parsing preserves years 0001–0099; Date.UTC adds 1900 to those years.
  const date = new Date(`${value}T00:00:00.000Z`);
  return date.getUTCFullYear() === Number(parts[1]) &&
    date.getUTCMonth() === Number(parts[2]) - 1 && date.getUTCDate() === Number(parts[3])
    ? date : null;
}

/** Formats a calendar day in UTC so the display zone cannot move it. */
export function formatCalendarDate(
  isoDate: string,
  region: string,
  style: DateStyle = 'medium',
): string {
  const check = parseCalendarDate(isoDate.trim());
  if (check === null) {
    // Not a stored date: return the input so the caller can show it verbatim
    // rather than inventing a value.
    return isoDate;
  }
  try {
    return new Intl.DateTimeFormat(region, {
      timeZone: 'UTC',
      year: 'numeric',
      month: style === 'short' ? 'numeric' : style === 'long' ? 'long' : 'short',
      day: 'numeric',
    }).format(check);
  } catch {
    return isoDate;
  }
}

/**
 * Formats an instant in the chosen zone.
 *
 * Timestamp strings and epoch milliseconds use the display zone; a bare date
 * falls back to `formatCalendarDate` so it cannot shift.
 */
export function formatInstant(
  value: string | number,
  preferences: RegionalPreferences,
): string {
  if (typeof value === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(value.trim())) {
    return formatCalendarDate(value, preferences.region);
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return String(value);
  }
  try {
    return new Intl.DateTimeFormat(preferences.region, {
      timeZone: preferences.timeZone,
      dateStyle: 'medium',
      timeStyle: 'short',
    }).format(parsed);
  } catch {
    return String(value);
  }
}

/** Formats a count with the region's grouping and digits. */
export function formatNumber(value: number, region: string, options?: Intl.NumberFormatOptions): string {
  if (!Number.isFinite(value)) {
    return String(value);
  }
  try {
    return new Intl.NumberFormat(region, options).format(value);
  } catch {
    return new Intl.NumberFormat('en', options).format(value);
  }
}

/** Formats a note count; the plural form comes from the caller's message. */
export function formatCount(count: number, region: string): string {
  return formatNumber(count, region);
}

/**
 * A local date string, for "today" and daily notes.
 *
 * `toISOString()` would use UTC and can return yesterday's date for a user east
 * of UTC late in the evening, which is the F16/F31 midnight bug. This builds the
 * value from local parts instead.
 */
export function localCalendarDate(now: Date): string {
  return `${String(now.getFullYear()).padStart(4, '0')}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** Formats the stored date for a `<time datetime>` element, unchanged. */
export function machineDate(value: string): string {
  return /^\d{4}-\d{2}-\d{2}$/.test(value.trim()) ? value.trim() : value;
}

/** A short zone label for the settings preview. */
export function zoneLabel(preferences: RegionalPreferences, at: Date = new Date()): string {
  try {
    return new Intl.DateTimeFormat(preferences.region, {
      timeZone: preferences.timeZone,
      timeZoneName: 'short',
      month: 'short',
      day: 'numeric',
    }).format(at);
  } catch {
    return preferences.timeZone;
  }
}

/** The list of zones the picker offers; a fixed list avoids a platform query. */
export const COMMON_TIME_ZONES: readonly string[] = [
  'UTC',
  'America/New_York',
  'America/Chicago',
  'America/Denver',
  'America/Los_Angeles',
  'America/Sao_Paulo',
  'Europe/London',
  'Europe/Paris',
  'Europe/Berlin',
  'Europe/Moscow',
  'Asia/Dubai',
  'Asia/Kolkata',
  'Asia/Shanghai',
  'Asia/Tokyo',
  'Asia/Seoul',
  'Asia/Singapore',
  'Australia/Sydney',
  'Pacific/Auckland',
];

/** Why a proposed zone string was refused, if it was. */
export function validateTimeZone(zone: string): boolean {
  const trimmed = zone.trim();
  if (trimmed === '') {
    return false;
  }
  try {
    // Constructing the formatter is the only portable way to ask the platform
    // whether a zone id exists.
    new Intl.DateTimeFormat('en', { timeZone: trimmed });
    return true;
  } catch {
    return false;
  }
}
