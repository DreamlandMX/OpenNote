import { describe, expect, it } from 'vitest';
import type { CellValue, DatabaseColumn } from '../editor';
import {
  ariaSort,
  cellText,
  describeFilters,
  filterProblem,
  isEmptyDueToFilters,
  isUnset,
  nextSort,
  operatorsFor,
  rowKey,
  tableSummary,
  toFilter,
  usableFilters,
} from './table-view';

const unset: CellValue = { kind: 'unset' };

describe('cell rendering', () => {
  it('shows an explicit label for an unset cell instead of 0 or empty', () => {
    // F22-R01: "no value" must not look like a real zero.
    expect(cellText(unset, 'en-US', 'Not set')).toBe('Not set');
    expect(cellText({ kind: 'number', value: 0 }, 'en-US', 'Not set')).toBe('0');
    expect(cellText(unset, 'en-US', 'Not set')).not.toBe('0');
  });

  it('formats numbers and dates per region without shifting the day', () => {
    expect(cellText({ kind: 'number', value: 1234567 }, 'de-DE', 'x')).toBe('1.234.567');
    const date = cellText({ kind: 'date', value: '2026-10-20' }, 'en-US', 'x');
    expect(date).toContain('20');
    expect(date).not.toContain('19');
  });

    it('formats datetime cells in the selected zone without shifting pure dates', () => {
      const instant = { kind: 'datetime' as const, value: '2026-10-20T23:30:00Z' };
      expect(cellText(instant, 'en-GB', 'x', 'UTC')).not.toBe(cellText(instant, 'en-GB', 'x', 'Asia/Tokyo'));
      expect(cellText(instant, 'en-GB', 'x', 'Asia/Tokyo')).toContain('21');
      expect(cellText({ kind: 'date', value: '2026-10-20' }, 'en-GB', 'x', 'Asia/Tokyo')).toContain('20');
    });

    it('renders a boolean as text, not only a colour', () => {
    expect(cellText({ kind: 'boolean', value: true }, 'en', 'x')).toBe('✓');
    expect(cellText({ kind: 'boolean', value: false }, 'en', 'x')).toBe('—');
  });

  it('joins a multi-select and passes text through unchanged', () => {
    expect(cellText({ kind: 'multiselect', value: ['a', 'b'] }, 'en', 'x')).toBe('a, b');
    expect(cellText({ kind: 'text', value: '原文' }, 'en', 'x')).toBe('原文');
  });

  it('isUnset is true only for the unset kind', () => {
    expect(isUnset(unset)).toBe(true);
    expect(isUnset({ kind: 'text', value: '' })).toBe(false);
  });
});

describe('sorting', () => {
  it('cycles ascending, descending, off', () => {
    const first = nextSort(null, 'score');
    expect(first).toEqual({ key: 'score', direction: 'ascending' });
    const second = nextSort(first, 'score');
    expect(second).toEqual({ key: 'score', direction: 'descending' });
    expect(nextSort(second, 'score')).toBeNull();
  });

  it('switching column starts ascending on the new column', () => {
    expect(nextSort({ key: 'a', direction: 'descending' }, 'b')).toEqual({
      key: 'b',
      direction: 'ascending',
    });
  });

  it('exposes aria-sort for the header', () => {
    expect(ariaSort(null, 'a')).toBe('none');
    expect(ariaSort({ key: 'a', direction: 'ascending' }, 'a')).toBe('ascending');
    expect(ariaSort({ key: 'a', direction: 'ascending' }, 'b')).toBe('none');
  });
});

describe('filters', () => {
  it('offers value operators only where a value makes sense', () => {
    expect(operatorsFor('boolean')).toEqual(['equals', 'notEquals', 'isSet', 'isUnset']);
    expect(operatorsFor('date')).toContain('contains');
    expect(operatorsFor('unknown')).toEqual(['isSet', 'isUnset']);
  });

  it('refuses a value operator with an empty value', () => {
    expect(toFilter({ key: 'a', op: 'equals', value: '   ' })).toBeNull();
    expect(toFilter({ key: 'a', op: 'isSet', value: '' })).toEqual({
      key: 'a',
      op: 'isSet',
      value: '',
    });
  });

  it('trims a supplied value', () => {
    expect(toFilter({ key: 'a', op: 'contains', value: '  work  ' })).toEqual({
      key: 'a',
      op: 'contains',
      value: 'work',
    });
  });

  it('describes the active filters with their column keys', () => {
    const columns: DatabaseColumn[] = [{ key: 'status', kind: 'select' }];
    expect(
      describeFilters([{ key: 'status', op: 'equals', value: 'doing' }], columns),
    ).toEqual([
      {
        key: 'status',
        column: 'status',
        op: 'equals',
        value: 'doing',
        problem: null,
      },
    ]);
  });
});

describe('invalid filters', () => {
  const columns: DatabaseColumn[] = [
    { key: 'status', kind: 'select' },
    { key: 'done', kind: 'boolean' },
  ];

  it('flags a condition whose column no longer exists', () => {
    // F23 §7 FILTER_INVALID: a deleted field must be shown, not silently kept.
    expect(filterProblem({ key: 'gone', op: 'isSet', value: '' }, columns)).toBe(
      'missing-column',
    );
    const described = describeFilters(
      [{ key: 'gone', op: 'isSet', value: '' }],
      columns,
    );
    expect(described[0]?.problem).toBe('missing-column');
  });

  it('flags an operator the column type does not offer', () => {
    expect(
      filterProblem({ key: 'done', op: 'contains', value: 'tr' }, columns),
    ).toBe('operator-mismatch');
    expect(
      filterProblem({ key: 'done', op: 'equals', value: 'true' }, columns),
    ).toBeNull();
  });

  it('sends only usable conditions and keeps the remaining order', () => {
    const filters = [
      { key: 'gone', op: 'equals' as const, value: 'x' },
      { key: 'status', op: 'equals' as const, value: 'doing' },
      { key: 'done', op: 'contains' as const, value: 'tr' },
      { key: 'done', op: 'isSet' as const, value: '' },
    ];
    expect(usableFilters(filters, columns)).toEqual([
      { key: 'status', op: 'equals', value: 'doing' },
      { key: 'done', op: 'isSet', value: '' },
    ]);
  });

  it('treats an unknown column type as supporting only set/unset', () => {
    const unknown: DatabaseColumn[] = [{ key: 'x', kind: 'unknown' }];
    expect(filterProblem({ key: 'x', op: 'equals', value: '1' }, unknown)).toBe(
      'operator-mismatch',
    );
    expect(filterProblem({ key: 'x', op: 'isUnset', value: '' }, unknown)).toBeNull();
  });

  it('keeps every operator its column type offers', () => {
    for (const kind of [
      'text',
      'number',
      'boolean',
      'date',
      'datetime',
      'select',
      'multiselect',
      'link',
    ]) {
      for (const op of operatorsFor(kind)) {
        expect(
          filterProblem({ key: 'x', op, value: 'v' }, [{ key: 'x', kind }]),
        ).toBeNull();
      }
    }
  });
});

describe('table summary', () => {
  const rows = [{ noteId: 'a' }] as never[];
  it('reports shown, hidden and total', () => {
    expect(tableSummary({ rows, hidden: 2 })).toEqual({ shown: 1, hidden: 2, total: 3 });
  });

  it('distinguishes "all filtered out" from "no notes at all"', () => {
    expect(isEmptyDueToFilters({ rows: [], hidden: 3 })).toBe(true);
    expect(isEmptyDueToFilters({ rows: [], hidden: 0 })).toBe(false);
  });

  it('keys a row by note id so a rename does not remount it', () => {
    expect(rowKey({ noteId: 'n1' } as never)).toBe('n1');
  });
});
