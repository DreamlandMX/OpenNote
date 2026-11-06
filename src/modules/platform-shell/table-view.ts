import type { CellValue, DatabaseColumn, DatabaseFilter, DatabaseRow, DatabaseSort } from '../editor';
import { formatCalendarDate, formatInstant, formatNumber } from './regional';

/**
 * S21 table-view logic.
 *
 * The projection itself runs in M06; this module decides presentation only. The
 * rules it owns are the ones a cell renderer must not get wrong:
 *
 * * an unset cell renders as an explicit "not set" label, never as `0` or an
 *   empty string (F22-R01);
 * * a hidden row is counted, never silently dropped (F23-R04);
 * * a date cell is formatted from its own digits, so a device zone cannot move
 *   it (F31/F23-R02).
 */

export interface FilterDraft {
  key: string;
  op: DatabaseFilter['op'];
  value: string;
}

/** Filter operators offered per column type, so an invalid pair cannot be built. */
export function operatorsFor(kind: string): DatabaseFilter['op'][] {
  switch (kind) {
    case 'boolean':
      return ['equals', 'notEquals', 'isSet', 'isUnset'];
    case 'number':
    case 'date':
    case 'datetime':
    case 'select':
    case 'multiselect':
    case 'link':
    case 'text':
      return ['equals', 'notEquals', 'contains', 'isSet', 'isUnset'];
    default:
      return ['isSet', 'isUnset'];
  }
}

/** Builds a filter only when the operator supports the value. */
export function toFilter(draft: FilterDraft): DatabaseFilter | null {
  const needsValue = draft.op === 'equals' || draft.op === 'notEquals' || draft.op === 'contains';
  if (needsValue && draft.value.trim() === '') {
    return null;
  }
  return {
    key: draft.key,
    op: draft.op,
    value: needsValue ? draft.value.trim() : '',
  };
}

/** The label a cell shows, with `unsetLabel` for a missing value. */
export function cellText(
  cell: CellValue,
  region: string,
  unsetLabel: string,
  timeZone = 'UTC',
): string {
  switch (cell.kind) {
    case 'unset':
      return unsetLabel;
    case 'text':
    case 'select':
    case 'link':
      return cell.value;
    case 'number':
      return formatNumber(cell.value, region);
    case 'boolean':
      // A boolean shows a word, not only a colour or an icon (F23-R05/ARCH-05).
      return cell.value ? '✓' : '—';
    case 'date':
      return formatCalendarDate(cell.value, region, 'short');
    case 'datetime':
      return formatInstant(cell.value, { region, timeZone });
    case 'multiselect':
      return cell.value.join(', ');
    default:
      return unsetLabel;
  }
}

/** True when the cell has no value, for styling and sorting hints. */
export function isUnset(cell: CellValue): boolean {
  return cell.kind === 'unset';
}

/**
 * Cycles a sort through ascending, descending and off.
 *
 * Returning null for "off" matters: F23 §4 defaults to modified-desc, so an
 * explicit third state lets the user get back to the default order instead of
 * being stuck in one direction.
 */
export function nextSort(current: DatabaseSort | null, key: string): DatabaseSort | null {
  if (current === null || current.key !== key) {
    return { key, direction: 'ascending' };
  }
  if (current.direction === 'ascending') {
    return { key, direction: 'descending' };
  }
  return null;
}

/** The `aria-sort` value for a column header. */
export function ariaSort(
  current: DatabaseSort | null,
  key: string,
): 'ascending' | 'descending' | 'none' {
  if (current === null || current.key !== key) {
    return 'none';
  }
  return current.direction;
}

/**
 * Why a filter cannot run against the current projection.
 *
 * `missing-column` covers a deleted property; `operator-mismatch` covers a
 * column whose type changed so the stored condition is no longer offered.
 */
export type FilterProblem = 'missing-column' | 'operator-mismatch';

/** Null when the condition still applies to a live column of the right type. */
export function filterProblem(
  filter: DatabaseFilter,
  columns: readonly DatabaseColumn[],
): FilterProblem | null {
  const column = columns.find((entry) => entry.key === filter.key);
  if (column === undefined) {
    return 'missing-column';
  }
  return operatorsFor(column.kind).includes(filter.op) ? null : 'operator-mismatch';
}

/**
 * The conditions M06 should apply.
 *
 * A filter on a deleted property used to be sent anyway and silently ignored by
 * the projection, so the panel showed it as active while it did nothing. Sending
 * only usable conditions keeps the request and the displayed conditions equal
 * (F23 §7 FILTER_INVALID).
 */
export function usableFilters(
  filters: readonly DatabaseFilter[],
  columns: readonly DatabaseColumn[],
): DatabaseFilter[] {
  return filters.filter((filter) => filterProblem(filter, columns) === null);
}

/** A filter summary, so active and inactive conditions stay visible (F23 §4). */
export function describeFilters(
  filters: readonly DatabaseFilter[],
  columns: readonly DatabaseColumn[],
): {
  key: string;
  column: string;
  op: DatabaseFilter['op'];
  value: string;
  problem: FilterProblem | null;
}[] {
  return filters.map((filter) => ({
    key: filter.key,
    column: columns.find((column) => column.key === filter.key)?.key ?? filter.key,
    op: filter.op,
    value: filter.value,
    problem: filterProblem(filter, columns),
  }));
}

/** Rows shown versus rows hidden, for the footer (F23-R04). */
export function tableSummary(table: { rows: readonly DatabaseRow[]; hidden: number }): {
  shown: number;
  hidden: number;
  total: number;
} {
  return {
    shown: table.rows.length,
    hidden: table.hidden,
    total: table.rows.length + table.hidden,
  };
}

/** True when filters removed everything, so the UI can offer to clear them. */
export function isEmptyDueToFilters(table: {
  rows: readonly DatabaseRow[];
  hidden: number;
}): boolean {
  return table.rows.length === 0 && table.hidden > 0;
}

/** A stable row key: the note id, so a title change does not remount the row. */
export function rowKey(row: DatabaseRow): string {
  return row.noteId;
}
