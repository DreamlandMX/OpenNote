import type { PropertyValue } from '../editor';
import { parseCalendarDate } from './regional';

/**
 * F09 property panel model. Parsing lives in M04; this module only decides how a
 * row is displayed and how typed text becomes a typed value. Invalid input is
 * reported next to the field and never silently becomes a different type
 * (F09-R04), and an unset field stays distinguishable from "", 0 and false.
 */

export type PropertyKind =
  | 'text'
  | 'number'
  | 'boolean'
  | 'date'
  | 'dateTime'
  | 'select'
  | 'multiSelect'
  | 'link'
  | 'unset';

/** Wire shape returned by `read_note_metadata`/`parse_properties`. */
export interface HostProperty {
  name: string;
  value: HostValue;
  unknown: boolean;
}

export type HostValue =
  | { text: string }
  | { number: number }
  | { boolean: boolean }
  | { date: string }
  | { dateTime: string }
  | { select: string }
  | { multiSelect: readonly string[] }
  | { link: string }
  | 'unset';

export type ValueParse =
  | { ok: true; value: PropertyValue }
  | { ok: false; reason: 'invalidNumber' | 'invalidBoolean' | 'invalidDate' | 'invalidDateTime' | 'invalidLink' | 'empty' };

export function kindOfHostValue(value: HostValue): PropertyKind {
  if (value === 'unset') {
    return 'unset';
  }
  if ('text' in value) return 'text';
  if ('number' in value) return 'number';
  if ('boolean' in value) return 'boolean';
  if ('date' in value) return 'date';
  if ('dateTime' in value) return 'dateTime';
  if ('select' in value) return 'select';
  if ('multiSelect' in value) return 'multiSelect';
  if ('link' in value) return 'link';
  return 'unset';
}

/** Display text for a value; `unset` is not the empty string (F09-A01). */
export function displayValue(value: HostValue): string {
  if (value === 'unset') {
    return '';
  }
  if ('text' in value) return value.text;
  if ('number' in value) return String(value.number);
  if ('boolean' in value) return value.boolean ? 'true' : 'false';
  if ('date' in value) return value.date;
  if ('dateTime' in value) return value.dateTime;
  if ('select' in value) return value.select;
  if ('multiSelect' in value) return value.multiSelect.join(', ');
  if ('link' in value) return value.link;
  return '';
}

/** Converts a host value into the write model without inventing a type. */
export function toPropertyValue(value: HostValue): PropertyValue {
  if (value === 'unset') return { kind: 'unset' };
  if ('text' in value) return { kind: 'text', text: value.text };
  if ('number' in value) return { kind: 'number', value: value.number };
  if ('boolean' in value) return { kind: 'boolean', value: value.boolean };
  if ('date' in value) return { kind: 'date', date: value.date };
  if ('dateTime' in value) return { kind: 'dateTime', at: value.dateTime };
  if ('select' in value) return { kind: 'select', text: value.select };
  if ('multiSelect' in value) return { kind: 'multiSelect', values: value.multiSelect };
  if ('link' in value) return { kind: 'link', target: value.link };
  return { kind: 'unset' };
}

/**
 * Converts the write model into the wire shape `apply_property_edit` accepts.
 * The host enum is externally tagged, so `kind` must not reach serde.
 */
export function toHostValue(value: PropertyValue): HostValue {
  switch (value.kind) {
    case 'text':
      return { text: value.text };
    case 'number':
      return { number: value.value };
    case 'boolean':
      return { boolean: value.value };
    case 'date':
      return { date: value.date };
    case 'dateTime':
      return { dateTime: value.at };
    case 'select':
      return { select: value.text };
    case 'multiSelect':
      return { multiSelect: value.values };
    case 'link':
      return { link: value.target };
    case 'unset':
      return 'unset';
  }
}

/** F09-R02: a date is a calendar day with no time zone attached. */
export function isCalendarDate(text: string): boolean {
  return parseCalendarDate(text) !== null;
}

/**
 * Parses typed text for a field of `kind`. A blank field is `unset`, never "",
 * except for a text field where the user may deliberately clear to empty.
 */
export function parseInput(kind: PropertyKind, raw: string): ValueParse {
  const text = raw.trim();
  if (kind === 'text') {
    return { ok: true, value: { kind: 'text', text: raw } };
  }
  if (text === '') {
    if (kind === 'multiSelect') {
      return { ok: true, value: { kind: 'multiSelect', values: [] } };
    }
    return { ok: true, value: { kind: 'unset' } };
  }
  switch (kind) {
    case 'number': {
      // A comma decimal separator is a display convention; it is accepted on
      // input but never persisted, and the persisted form is a plain number.
      const normalized = text.replace(',', '.');
      const value = Number(normalized);
      if (!Number.isFinite(value)) {
        return { ok: false, reason: 'invalidNumber' };
      }
      return { ok: true, value: { kind: 'number', value } };
    }
    case 'boolean': {
      const lower = text.toLowerCase();
      if (['true', 'yes', '1', 'on'].includes(lower)) {
        return { ok: true, value: { kind: 'boolean', value: true } };
      }
      if (['false', 'no', '0', 'off'].includes(lower)) {
        return { ok: true, value: { kind: 'boolean', value: false } };
      }
      return { ok: false, reason: 'invalidBoolean' };
    }
    case 'date': {
      if (!isCalendarDate(text)) {
        return { ok: false, reason: 'invalidDate' };
      }
      return { ok: true, value: { kind: 'date', date: text } };
    }
    case 'dateTime': {
      const parsed = new Date(text);
      if (Number.isNaN(parsed.getTime())) {
        return { ok: false, reason: 'invalidDateTime' };
      }
      // Persist a UTC instant so the same moment reads back identically.
      return { ok: true, value: { kind: 'dateTime', at: parsed.toISOString() } };
    }
    case 'select': {
      return { ok: true, value: { kind: 'select', text } };
    }
    case 'multiSelect': {
      const values = text
        .split(',')
        .map((entry) => entry.trim())
        .filter((entry) => entry !== '');
      return { ok: true, value: { kind: 'multiSelect', values } };
    }
    case 'link': {
      const trimmed = text.trim();
      if (trimmed.includes('\n')) {
        return { ok: false, reason: 'invalidLink' };
      }
      return { ok: true, value: { kind: 'link', target: trimmed } };
    }
    default:
      return { ok: true, value: { kind: 'unset' } };
  }
}

/**
 * Reports which values a type change cannot convert, before anything is written
 * (F09-R04). An empty list means every value converts.
 */
export function incompatibleValues(
  from: PropertyKind,
  to: PropertyKind,
  values: readonly string[],
): string[] {
  if (from === to || to === 'text' || from === 'unset') {
    return [];
  }
  return values.filter((value) => !parseInput(to, value).ok);
}

/** Whether the identity key may be edited from this panel (F09 §4). */
export function isEditableName(name: string): boolean {
  const trimmed = name.trim();
  return (
    trimmed !== '' &&
    Array.from(trimmed).length <= 64 &&
    trimmed.toLowerCase() !== 'opennote-id'
  );
}
