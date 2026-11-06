import { describe, expect, it } from 'vitest';
import type { DailyOutcome } from '../editor';
import {
  actionFor,
  chosenDate,
  isCalendarDay,
  shiftDate,
  statusKey,
  WEEKDAY_KEYS,
  weekdayIndex,
} from './daily-view';

function outcome(overrides: Partial<DailyOutcome>): DailyOutcome {
  return {
    status: 'opened',
    noteId: 'n1',
    relativePath: 'Daily/2026/2026-10-20.md',
    matchedBy: 'path',
    candidates: [],
    ...overrides,
  };
}

describe('daily actions', () => {
  it('distinguishes created from opened', () => {
    expect(actionFor(outcome({ status: 'created', matchedBy: null }))).toEqual({
      kind: 'open',
      noteId: 'n1',
      created: true,
    });
    expect(actionFor(outcome({}))).toEqual({ kind: 'open', noteId: 'n1', created: false });
    expect(statusKey(actionFor(outcome({ status: 'created' })))).toBe('daily.created');
    expect(statusKey(actionFor(outcome({})))).toBe('daily.opened');
  });

  it('never auto-opens an ambiguous day', () => {
    const action = actionFor(
      outcome({
        status: 'ambiguous',
        noteId: '',
        candidates: [
          { noteId: 'a', relativePath: 'Daily/a.md', propertyDate: null },
          { noteId: 'b', relativePath: 'Copies/b.md', propertyDate: '2026-10-20' },
        ],
      }),
    );
    expect(action.kind).toBe('choose');
    expect(statusKey(action)).toBe('daily.ambiguous');
    if (action.kind === 'choose') {
      expect(action.candidates).toHaveLength(2);
    }
  });

  it('turns a missing template into a choice instead of a blank or failed note', () => {
    const action = actionFor(
      outcome({ status: 'templateMissing', noteId: '', matchedBy: null }),
    );
    expect(action).toEqual({ kind: 'template-missing' });
    expect(statusKey(action)).toBe('daily.templateMissingPrompt');
  });

  it('reports a result without an id instead of opening an empty editor', () => {
    const action = actionFor(outcome({ noteId: '' }));
    expect(action).toEqual({ kind: 'failed', reason: 'error.dailyUnavailable' });
    expect(statusKey(action)).toBe('daily.openFailed');
  });
});

describe('calendar stepping', () => {
  it('moves by whole days across month and year boundaries', () => {
    expect(shiftDate('2026-10-20', 1)).toBe('2026-10-21');
    expect(shiftDate('2026-10-31', 1)).toBe('2026-11-01');
    expect(shiftDate('2026-12-31', 1)).toBe('2027-01-01');
    expect(shiftDate('2026-01-01', -1)).toBe('2025-12-31');
    expect(shiftDate('2024-02-28', 1)).toBe('2024-02-29');
    expect(shiftDate('2026-02-28', 1)).toBe('2026-03-01');
  });

  it('is not affected by a daylight-saving offset', () => {
    // A 24-hour addition would land on the same date in some zones; the calendar
    // walk cannot.
    expect(shiftDate('2026-03-08', 1)).toBe('2026-03-09');
    expect(shiftDate('2026-11-01', 1)).toBe('2026-11-02');
  });

  it('returns the input unchanged for a malformed date', () => {
    expect(shiftDate('not-a-date', 1)).toBe('not-a-date');
    expect(shiftDate('2026-1-1', 1)).toBe('2026-1-1');
  });
});

describe('calendar validation and weekday', () => {
  it('accepts real days and rejects impossible ones', () => {
    expect(isCalendarDay('2026-10-20')).toBe(true);
    expect(isCalendarDay('2024-02-29')).toBe(true);
    expect(isCalendarDay('2026-02-29')).toBe(false);
    expect(isCalendarDay('2026-13-01')).toBe(false);
    expect(isCalendarDay('')).toBe(false);
  });

  it('reports the weekday in UTC so the day cannot shift', () => {
    // 2026-10-20 is a Tuesday and the message table starts on Monday.
    expect(weekdayIndex('2026-10-20')).toBe(1);
    expect(weekdayIndex('2026-10-19')).toBe(0);
    // Sunday must stay on the last key, not wrap to Monday.
    expect(weekdayIndex('2026-10-25')).toBe(6);
    expect(weekdayIndex('bad')).toBe(0);
  });

  it('accepts only a real day from the date picker', () => {
    expect(chosenDate('2026-10-20')).toBe('2026-10-20');
    expect(chosenDate('')).toBeNull();
    expect(chosenDate('2026-02-30')).toBeNull();
    expect(chosenDate('2026-1-1')).toBeNull();
  });
});

describe('weekday keys', () => {
  it('preserves early years and rejects navigation outside valid date inputs', () => {
    expect(isCalendarDay('0004-02-29')).toBe(true);
    expect(chosenDate('0042-06-01')).toBe('0042-06-01');
    expect(weekdayIndex('0001-01-01')).toBe(0);
    expect(shiftDate('0099-12-31', 1)).toBe('0100-01-01');
    expect(shiftDate('0004-02-28', 1)).toBe('0004-02-29');
  });

  it.each([
    ['2026-02-30', 1], ['9999-12-31', 1], ['0001-01-01', -1],
    ['2026-10-20', Number.NaN], ['2026-10-20', 1.5],
  ] as const)('keeps %s unchanged for an invalid day step %s', (date, step) => {
    expect(shiftDate(date, step)).toBe(date);
  });

  it('covers every weekday exactly once and in UTC order', () => {
    expect(WEEKDAY_KEYS).toHaveLength(7);
    expect(new Set(WEEKDAY_KEYS).size).toBe(7);
    expect(WEEKDAY_KEYS[weekdayIndex('2026-10-20')]).toBe('daily.weekday.1');
    expect(WEEKDAY_KEYS[weekdayIndex('2026-10-25')]).toBe('daily.weekday.6');
  });
});
