import type { DailyOutcome } from '../editor';
import { parseCalendarDate } from './regional';

/**
 * S25 daily-note decisions.
 *
 * The host resolves the date; this module only decides what the UI does with the
 * answer. The rules that must not be re-invented in a component:
 *
 * * `created` and `opened` are different results, so the UI can say whether it
 *   made a file (F17-R01);
 * * `ambiguous` is a question, never an automatic merge (F17-R04);
 * * the date shown is the requested calendar day, not "now" (F17-R02).
 */

export type DailyAction =
  | { kind: 'open'; noteId: string; created: boolean }
  | { kind: 'choose'; candidates: DailyOutcome['candidates'] }
  | { kind: 'template-missing' }
  | { kind: 'failed'; reason: string };

export function actionFor(outcome: DailyOutcome): DailyAction {
  if (outcome.status === 'templateMissing') {
    // The host did not create a note. The caller must offer blank or another
    // template instead of treating the empty id as a failed open (F17 §7).
    return { kind: 'template-missing' };
  }
  if (outcome.status === 'ambiguous') {
    return { kind: 'choose', candidates: outcome.candidates };
  }
  if (outcome.noteId === '') {
    // A non-ambiguous result without an id cannot be opened; report it instead
    // of opening an empty editor.
    return { kind: 'failed', reason: 'error.dailyUnavailable' };
  }
  return {
    kind: 'open',
    noteId: outcome.noteId,
    created: outcome.status === 'created',
  };
}

/** A status line for the outcome, so "created" is never shown as "opened". */
export function statusKey(
  action: DailyAction,
): 'daily.created' | 'daily.opened' | 'daily.ambiguous' | 'daily.templateMissingPrompt' | 'daily.openFailed' {
  if (action.kind === 'choose') {
    return 'daily.ambiguous';
  }
  if (action.kind === 'template-missing') {
    return 'daily.templateMissingPrompt';
  }
  if (action.kind === 'failed') {
    return 'daily.openFailed';
  }
  return action.created ? 'daily.created' : 'daily.opened';
}

/**
 * Steps a date by whole days on the calendar, not by milliseconds.
 *
 * Adding 24 hours would produce the same date across a daylight-saving change,
 * and subtracting them would skip one; this walks the calendar instead.
 */
export function shiftDate(date: string, days: number): string {
  const stepped = parseCalendarDate(date);
  if (stepped === null || !Number.isSafeInteger(days)) return date;
  stepped.setUTCDate(stepped.getUTCDate() + days);
  const year = stepped.getUTCFullYear();
  return year >= 1 && year <= 9999 ? stepped.toISOString().slice(0, 10) : date;
}

/**
 * Weekday message keys, Monday = 0 … Sunday = 6.
 *
 * A table rather than a template literal keeps every key a literal, so the
 * message-key union typechecks the string actually passed to the translator.
 */
export const WEEKDAY_KEYS = [
  'daily.weekday.0',
  'daily.weekday.1',
  'daily.weekday.2',
  'daily.weekday.3',
  'daily.weekday.4',
  'daily.weekday.5',
  'daily.weekday.6',
] as const;

/**
 * The weekday name key for a stored date, for the S25 header.
 *
 * The message table starts on Monday, while `Date.getUTCDay()` starts on Sunday,
 * so the raw value would show the label for the next day (and Sunday would show
 * Monday). The conversion keeps the table and the calendar consistent.
 */
export function weekdayIndex(date: string): number {
  const at = parseCalendarDate(date);
  return at === null ? 0 : (at.getUTCDay() + 6) % 7;
}

/** True when the requested date is a real calendar day. */
export function isCalendarDay(date: string): boolean {
  return parseCalendarDate(date) !== null;
}

/**
 * A date chosen in the calendar field, or null when the field is empty or the
 * input is not a real day. The caller keeps its current date on null, so a
 * half-typed value never opens the wrong note (F17 §4).
 */
export function chosenDate(value: string): string | null {
  return isCalendarDay(value) ? value : null;
}
