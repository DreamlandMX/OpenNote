import { describe, expect, it } from 'vitest';
import {
  COMMON_TIME_ZONES,
  firstDayOfWeek,
  formatCalendarDate,
  formatInstant,
  formatNumber,
  localCalendarDate,
  machineDate,
  validateTimeZone,
  zoneLabel,
} from './regional';

describe('calendar dates never move', () => {
  it('formats a stored date from its own parts, not from an instant', () => {
    // A user in UTC-8 must still see the 20th, never the 19th.
    const formatted = formatCalendarDate('2026-10-20', 'en-US');
    expect(formatted).toContain('20');
    expect(formatted).not.toContain('19');
  });

  it('is independent of the requested display zone', () => {
    const west = formatInstant('2026-10-20', {
      region: 'en-US',
      timeZone: 'America/Los_Angeles',
    });
    const east = formatInstant('2026-10-20', {
      region: 'en-US',
      timeZone: 'Asia/Tokyo',
    });
    expect(west).toBe(east);
    expect(west).toContain('20');
  });

  it('refuses to roll an impossible date forward', () => {
    expect(formatCalendarDate('2026-02-30', 'en-US')).toBe('2026-02-30');
    expect(formatCalendarDate('2026-13-01', 'en-US')).toBe('2026-13-01');
    expect(formatCalendarDate('not a date', 'en-US')).toBe('not a date');
  });

  it('returns the input for a malformed region instead of throwing', () => {
    expect(formatCalendarDate('2026-10-20', 'x-invalid-!!')).toBe('2026-10-20');
  });

  it('respects the requested style', () => {
    const short = formatCalendarDate('2026-10-20', 'en-US', 'short');
    const long = formatCalendarDate('2026-10-20', 'en-US', 'long');
    expect(short.length).toBeLessThanOrEqual(long.length);
  });
});

describe('instants use the chosen zone', () => {
  it('formats early calendar years without a 1900 offset', () => {
    expect(formatCalendarDate('0042-06-01', 'en-US')).toBe('Jun 1, 42');
    expect(formatCalendarDate('0004-02-29', 'en-US')).toBe('Feb 29, 4');
    expect(localCalendarDate(new Date('0042-06-01T12:00:00'))).toBe('0042-06-01');
  });

  it('formats epoch milliseconds like the matching instant and preserves invalid values', () => {
    const preferences = { region: 'en-GB', timeZone: 'Asia/Tokyo' };
    expect(formatInstant(Date.parse('2026-10-20T23:30:00Z'), preferences))
      .toBe(formatInstant('2026-10-20T23:30:00Z', preferences));
    expect(formatInstant(Number.NaN, preferences)).toBe('NaN');
    expect(formatInstant(Number.POSITIVE_INFINITY, preferences)).toBe('Infinity');
  });

  it('formats an RFC 3339 value in the display zone', () => {
    const utc = formatInstant('2026-10-20T23:30:00Z', {
      region: 'en-GB',
      timeZone: 'UTC',
    });
    const tokyo = formatInstant('2026-10-20T23:30:00Z', {
      region: 'en-GB',
      timeZone: 'Asia/Tokyo',
    });
    expect(utc).not.toBe(tokyo);
    // Tokyo is 9 hours ahead, so 23:30Z is the next day there.
    expect(tokyo).toContain('21');
  });

  it('returns an invalid instant verbatim', () => {
    expect(formatInstant('yesterday', { region: 'en', timeZone: 'UTC' })).toBe('yesterday');
  });

  it('falls back to the zone id when the zone is unusable', () => {
    expect(zoneLabel({ region: 'en', timeZone: 'Not/AZone' })).toBe('Not/AZone');
  });
});

describe('local calendar date', () => {
  it('builds the date from local parts, not UTC', () => {
    // 23:30 local on the 20th: toISOString in a negative offset would say the 21st.
    const local = new Date(2026, 9, 20, 23, 30, 0);
    expect(localCalendarDate(local)).toBe('2026-10-20');
  });

  it('pads single-digit months and days', () => {
    expect(localCalendarDate(new Date(2026, 0, 5, 8, 0, 0))).toBe('2026-01-05');
  });

  it('machineDate keeps a stored date byte-identical', () => {
    expect(machineDate('2026-10-20')).toBe('2026-10-20');
    expect(machineDate(' 2026-10-20 ')).toBe('2026-10-20');
    expect(machineDate('tomorrow')).toBe('tomorrow');
  });
});

describe('numbers and week start', () => {
  it('groups numbers per region', () => {
    expect(formatNumber(1234567, 'en-US')).toBe('1,234,567');
    expect(formatNumber(1234567, 'de-DE')).toBe('1.234.567');
    expect(formatNumber(Number.NaN, 'en-US')).toBe('NaN');
    expect(formatNumber(1234.55, 'de-DE', { maximumFractionDigits: 1 })).toBe('1.234,6');
    expect(formatNumber(1234.5, 'x-invalid-!!', { maximumFractionDigits: 1 })).toBe('1,234.5');
  });

  it('starts the week on Sunday only where that is the convention', () => {
    expect(firstDayOfWeek('en-US')).toBe(7);
    expect(firstDayOfWeek('ja-JP')).toBe(7);
    expect(firstDayOfWeek('de-DE')).toBe(1);
    expect(firstDayOfWeek('zh-CN')).toBe(1);
    expect(firstDayOfWeek('en_GB')).toBe(1);
  });

  it.each([['en', 7], ['ar', 6], ['ja', 7], ['zh-TW', 7], ['ar-EG', 6], ['ar-SA', 7], ['dv-MV', 5], ['EN_us', 7]] as const)
    ('resolves regional week data for %s', (region, expected) => {
      expect(firstDayOfWeek(region)).toBe(expected);
    });

  it('falls back to territory week data when both week APIs are missing', () => {
    const prototype = Intl.Locale.prototype;
    const descriptors = ['getWeekInfo', 'weekInfo'].map(key => [key, Object.getOwnPropertyDescriptor(prototype, key)] as const);
    try {
      for (const [key] of descriptors) Object.defineProperty(prototype, key, { configurable: true, value: undefined });
      expect(firstDayOfWeek('en')).toBe(7);
      expect(firstDayOfWeek('ar')).toBe(6);
      expect(firstDayOfWeek('dv-MV')).toBe(5);
      expect(firstDayOfWeek('en-GB')).toBe(1);
      expect(firstDayOfWeek('x-invalid-!!')).toBe(1);
    } finally {
      for (const [key, descriptor] of descriptors) {
        if (descriptor) Object.defineProperty(prototype, key, descriptor);
        else Reflect.deleteProperty(prototype, key);
      }
    }
  });

  it('prefers valid method data and rejects an out-of-range native first day', () => {
    const prototype = Intl.Locale.prototype;
    const descriptor = Object.getOwnPropertyDescriptor(prototype, 'getWeekInfo');
    try {
      Object.defineProperty(prototype, 'getWeekInfo', { configurable: true, value: () => ({ firstDay: 5 }) });
      expect(firstDayOfWeek('en-US')).toBe(5);
      Object.defineProperty(prototype, 'getWeekInfo', { configurable: true, value: () => ({ firstDay: 99 }) });
      expect(firstDayOfWeek('en-GB')).toBe(1);
    } finally {
      if (descriptor) Object.defineProperty(prototype, 'getWeekInfo', descriptor);
      else Reflect.deleteProperty(prototype, 'getWeekInfo');
    }
  });

  it('accepts the zones the picker offers and rejects nonsense', () => {
    for (const zone of COMMON_TIME_ZONES) {
      expect(validateTimeZone(zone)).toBe(true);
    }
    expect(validateTimeZone('')).toBe(false);
    expect(validateTimeZone('Mars/Olympus')).toBe(false);
  });
});
