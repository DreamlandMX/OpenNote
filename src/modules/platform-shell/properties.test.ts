import { describe, expect, it } from 'vitest';
import type { PropertyValue } from '../editor';
import {
  displayValue,
  incompatibleValues,
  isCalendarDate,
  isEditableName,
  kindOfHostValue,
  parseInput,
  toHostValue,
  toPropertyValue,
  type HostValue,
} from './properties';

describe('property value model', () => {
  it('accepts early-year calendar dates without normalizing their year', () => {
    expect(isCalendarDate('0004-02-29')).toBe(true);
    expect(isCalendarDate('0042-06-01')).toBe(true);
    expect(parseInput('date', '0042-06-01')).toEqual({ ok: true, value: { kind: 'date', date: '0042-06-01' } });
    expect(isCalendarDate('0000-01-01')).toBe(false);
    expect(isCalendarDate('0001-02-29')).toBe(false);
  });

  it('keeps unset distinct from empty string, zero and false', () => {
    expect(displayValue('unset')).toBe('');
    expect(displayValue({ text: '' })).toBe('');
    expect(toPropertyValue('unset')).toEqual({ kind: 'unset' });
    expect(toPropertyValue({ text: '' })).toEqual({ kind: 'text', text: '' });
    expect(toPropertyValue({ number: 0 })).toEqual({ kind: 'number', value: 0 });
    expect(toPropertyValue({ boolean: false })).toEqual({ kind: 'boolean', value: false });
    expect(kindOfHostValue('unset')).toBe('unset');
  });

  it('maps each host variant to its kind and back', () => {
    const cases: [HostValue, string][] = [
      [{ text: 'x' }, 'text'],
      [{ number: 1 }, 'number'],
      [{ boolean: true }, 'boolean'],
      [{ date: '2026-10-20' }, 'date'],
      [{ dateTime: '2026-10-20T12:00:00Z' }, 'dateTime'],
      [{ select: 's' }, 'select'],
      [{ multiSelect: ['a', 'b'] }, 'multiSelect'],
      [{ link: '[[n]]' }, 'link'],
    ];
    for (const [value, kind] of cases) {
      expect(kindOfHostValue(value)).toBe(kind);
      expect(toPropertyValue(value).kind).toBe(kind);
    }
  });

  it('writes every kind in the externally tagged host shape', () => {
    const cases: [PropertyValue, HostValue][] = [
      [{ kind: 'text', text: 'x' }, { text: 'x' }],
      [{ kind: 'number', value: 1.5 }, { number: 1.5 }],
      [{ kind: 'boolean', value: false }, { boolean: false }],
      [{ kind: 'date', date: '2026-10-20' }, { date: '2026-10-20' }],
      [{ kind: 'dateTime', at: '2026-10-20T12:00:00Z' }, { dateTime: '2026-10-20T12:00:00Z' }],
      [{ kind: 'select', text: 'Done' }, { select: 'Done' }],
      [{ kind: 'multiSelect', values: ['a', 'b'] }, { multiSelect: ['a', 'b'] }],
      [{ kind: 'link', target: '[[n]]' }, { link: '[[n]]' }],
      [{ kind: 'unset' }, 'unset'],
    ];
    for (const [value, host] of cases) {
      expect(toHostValue(value)).toEqual(host);
      expect(toPropertyValue(host)).toEqual(value);
    }
  });

  it('blank input becomes unset rather than an empty concrete value', () => {
    expect(parseInput('number', '   ')).toEqual({ ok: true, value: { kind: 'unset' } });
    expect(parseInput('date', '')).toEqual({ ok: true, value: { kind: 'unset' } });
    expect(parseInput('text', '')).toEqual({ ok: true, value: { kind: 'text', text: '' } });
    expect(parseInput('multiSelect', '')).toEqual({
      ok: true,
      value: { kind: 'multiSelect', values: [] },
    });
  });

  it('rejects non-finite numbers and accepts a comma decimal on input only', () => {
    expect(parseInput('number', 'abc')).toEqual({ ok: false, reason: 'invalidNumber' });
    expect(parseInput('number', 'Infinity')).toEqual({ ok: false, reason: 'invalidNumber' });
    expect(parseInput('number', '1,5')).toEqual({ ok: true, value: { kind: 'number', value: 1.5 } });
    expect(parseInput('number', '-0')).toEqual({ ok: true, value: { kind: 'number', value: -0 } });
  });

  it('validates calendar dates strictly and keeps the day unchanged', () => {
    expect(isCalendarDate('2026-10-20')).toBe(true);
    expect(isCalendarDate('2026-02-30')).toBe(false);
    expect(isCalendarDate('2026-13-01')).toBe(false);
    expect(isCalendarDate('2026-1-1')).toBe(false);
    expect(isCalendarDate('2024-02-29')).toBe(true);
    expect(parseInput('date', '2026-02-30')).toEqual({ ok: false, reason: 'invalidDate' });
    expect(parseInput('date', '2026-10-20')).toEqual({
      ok: true,
      value: { kind: 'date', date: '2026-10-20' },
    });
  });

  it('stores a datetime as a UTC instant', () => {
    const parsed = parseInput('dateTime', '2026-10-20T12:30:00+02:00');
    expect(parsed).toEqual({
      ok: true,
      value: { kind: 'dateTime', at: '2026-10-20T10:30:00.000Z' },
    });
    expect(parseInput('dateTime', 'not a time')).toEqual({
      ok: false,
      reason: 'invalidDateTime',
    });
  });

  it('accepts the usual boolean spellings and rejects anything else', () => {
    for (const yes of ['true', 'YES', '1', 'on']) {
      expect(parseInput('boolean', yes)).toEqual({ ok: true, value: { kind: 'boolean', value: true } });
    }
    for (const no of ['false', 'No', '0', 'off']) {
      expect(parseInput('boolean', no)).toEqual({
        ok: true,
        value: { kind: 'boolean', value: false },
      });
    }
    expect(parseInput('boolean', 'maybe')).toEqual({ ok: false, reason: 'invalidBoolean' });
  });

  it('splits multi-select on commas and drops empty fragments', () => {
    expect(parseInput('multiSelect', ' a , ,b ')).toEqual({
      ok: true,
      value: { kind: 'multiSelect', values: ['a', 'b'] },
    });
    expect(parseInput('select', ' one ')).toEqual({ ok: true, value: { kind: 'select', text: 'one' } });
  });

  it('lists values a type change cannot convert instead of clearing them', () => {
    expect(incompatibleValues('text', 'number', ['1', 'two', '3'])).toEqual(['two']);
    expect(incompatibleValues('text', 'date', ['2026-10-20', 'someday'])).toEqual(['someday']);
    expect(incompatibleValues('text', 'text', ['anything'])).toEqual([]);
    expect(incompatibleValues('unset', 'number', ['x'])).toEqual([]);
  });

  it('refuses the reserved identity name and enforces 1-64 characters', () => {
    expect(isEditableName('title')).toBe(true);
    expect(isEditableName('')).toBe(false);
    expect(isEditableName('   ')).toBe(false);
    expect(isEditableName('opennote-id')).toBe(false);
    expect(isEditableName('OpenNote-Id')).toBe(false);
    expect(isEditableName('x'.repeat(64))).toBe(true);
    expect(isEditableName('x'.repeat(65))).toBe(false);
    expect(isEditableName('😀'.repeat(64))).toBe(true);
  });
});
