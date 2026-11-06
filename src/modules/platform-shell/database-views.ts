import type { DatabaseRow, DatabaseTable } from '../editor';
import { parseCalendarDate } from './regional';

/**
 * F23 view presentation over the read-only M06 projection.
 *
 * The table already contains every row and cell, so grouping and calendar
 * placement are pure projections of the same answer: switching a view never
 * copies data, and all four views open the same note ids. Card edits use the
 * normal property-save path.
 */

export type DatabaseViewType = 'table' | 'board' | 'gallery' | 'calendar';

export const DATABASE_VIEWS: readonly DatabaseViewType[] = [
  'table',
  'board',
  'gallery',
  'calendar',
];

export interface BoardGroup {
  /** The select value, or null for the "no value" column (F23-R01). */
  value: string | null;
  rows: DatabaseRow[];
}

/**
 * Groups rows by a select property, mirroring `onote_tasks::group_by_select`.
 *
 * The ungrouped bucket comes last so it cannot be mistaken for a choice, and a
 * cell that is not a single select value is ungrouped rather than guessed.
 */
export function groupRowsBySelect(table: DatabaseTable, key: string): BoardGroup[] {
  const index = table.columns.findIndex((column) => column.key === key);
  if (index < 0) {
    return [{ value: null, rows: [...table.rows] }];
  }
  const groups = new Map<string, DatabaseRow[]>();
  const ungrouped: DatabaseRow[] = [];
  for (const row of table.rows) {
    const cell = row.cells[index];
    let value: string | null = null;
    if (cell?.kind === 'select') {
      value = cell.value;
    } else if (cell?.kind === 'multiselect' && cell.value.length === 1) {
      value = cell.value[0]!;
    }
    if (value === null) {
      ungrouped.push(row);
      continue;
    }
    const existing = groups.get(value);
    if (existing === undefined) {
      groups.set(value, [row]);
    } else {
      existing.push(row);
    }
  }
  const result: BoardGroup[] = [...groups.entries()]
    .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
    .map(([value, rows]) => ({ value, rows }));
  if (ungrouped.length > 0) {
    result.push({ value: null, rows: ungrouped });
  }
  return result;
}

/** Existing select choices for a board column, sorted; non-select cells are ignored. */
export function selectChoices(table: DatabaseTable, key: string): string[] {
  const index = table.columns.findIndex((column) => column.key === key);
  if (index < 0) {
    return [];
  }
  const values = new Set<string>();
  for (const row of table.rows) {
    const cell = row.cells[index];
    if (cell?.kind === 'select' && cell.value !== '') {
      values.add(cell.value);
    }
  }
  return [...values].sort((left, right) => (left < right ? -1 : left > right ? 1 : 0));
}

/** The row's stored calendar day, or null for unset/datetime/text values (F23-A02). */
export function dateCellValue(
  row: DatabaseRow,
  table: DatabaseTable,
  key: string,
): string | null {
  const index = table.columns.findIndex((column) => column.key === key);
  if (index < 0) {
    return null;
  }
  const cell = row.cells[index];
  return cell?.kind === 'date' && isStoredDate(cell.value) ? cell.value : null;
}

export interface DatedRow {
  date: string;
  row: DatabaseRow;
}

export interface CalendarRows {
  dated: DatedRow[];
  /** Rows with no value in the bound date field (F23-A02). */
  unscheduled: DatabaseRow[];
}

function isStoredDate(value: string): boolean {
  return parseCalendarDate(value) !== null;
}

/**
 * Splits rows into dated cells and unscheduled rows for one bound date field.
 *
 * Only the named `date` property is used. A datetime or a text value is not a
 * calendar day, so it stays unscheduled instead of being placed on a guessed
 * day (F23-A02).
 */
export function calendarRows(table: DatabaseTable, key: string): CalendarRows {
  const index = table.columns.findIndex((column) => column.key === key);
  if (index < 0) {
    return { dated: [], unscheduled: [...table.rows] };
  }
  const dated: DatedRow[] = [];
  const unscheduled: DatabaseRow[] = [];
  for (const row of table.rows) {
    const cell = row.cells[index];
    if (cell?.kind === 'date' && isStoredDate(cell.value)) {
      dated.push({ date: cell.value, row });
    } else {
      unscheduled.push(row);
    }
  }
  dated.sort((left, right) => {
    if (left.date !== right.date) {
      return left.date < right.date ? -1 : 1;
    }
    return left.row.noteId < right.row.noteId ? -1 : left.row.noteId > right.row.noteId ? 1 : 0;
  });
  return { dated, unscheduled };
}

function pad(value: number): string {
  return String(value).padStart(2, '0');
}

/** The month of a device-local date, as `YYYY-MM`. */
export function currentMonth(now: Date = new Date()): string {
  return `${String(now.getFullYear()).padStart(4, '0')}-${pad(now.getMonth() + 1)}`;
}

/** Steps a month without carrying a day, so the 31st never skips a month. */
export function shiftMonth(month: string, delta: number): string {
  const at = parseCalendarDate(`${month}-01`);
  if (at === null || !Number.isSafeInteger(delta)) return month;
  at.setUTCMonth(at.getUTCMonth() + delta);
  const year = at.getUTCFullYear();
  return year >= 1 && year <= 9999 ? at.toISOString().slice(0, 7) : month;
}

/**
 * Calendar cells for a month, with nulls for the leading and trailing blanks.
 *
 * `firstDay` is ISO 1=Monday … 7=Sunday, matching the regional preference; the
 * grid is built from UTC parts so a device zone cannot move a day.
 */
export function monthGrid(month: string, firstDay: number): (string | null)[] {
  const first = parseCalendarDate(`${month}-01`);
  if (first === null) return [];
  const last = new Date(first);
  last.setUTCMonth(last.getUTCMonth() + 1, 0);
  const days = last.getUTCDate();
  const weekStartsOn = Number.isInteger(firstDay) && firstDay >= 1 && firstDay <= 7 ? firstDay : 1;
  const firstWeekday = (first.getUTCDay() + 6) % 7;
  const leading = (firstWeekday - (weekStartsOn - 1) + 7) % 7;
  const cells = Math.ceil((leading + days) / 7) * 7;
  return Array.from({ length: cells }, (_, index) => {
    const day = index - leading + 1;
    return day >= 1 && day <= days ? `${month}-${pad(day)}` : null;
  });
}

/** A localized month label; an unparsable month is shown verbatim. */
export function formatMonthLabel(month: string, region: string): string {
  const at = parseCalendarDate(`${month}-01`);
  if (at === null) return month;
  try {
    return new Intl.DateTimeFormat(region, {
      year: 'numeric',
      month: 'long',
      timeZone: 'UTC',
    }).format(at);
  } catch {
    return month;
  }
}
