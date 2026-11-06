import { describe, expect, it } from 'vitest';
import type { DatabaseRow, DatabaseTable } from '../editor';
import {
  calendarRows,
  currentMonth,
  dateCellValue,
  formatMonthLabel,
  groupRowsBySelect,
  monthGrid,
  selectChoices,
  shiftMonth,
} from './database-views';

function row(noteId: string, cells: DatabaseRow['cells']): DatabaseRow {
  return {
    noteId,
    title: noteId,
    relativePath: `${noteId}.md`,
    sourceRevision: 1,
    sourceHash: 'hash-1',
    readOnly: false,
    cells,
  };
}

function table(rows: readonly DatabaseRow[]): DatabaseTable {
  return {
    columns: [
      { key: 'status', kind: 'select' },
      { key: 'tags', kind: 'multiselect' },
      { key: 'due', kind: 'date' },
    ],
    rows,
    hidden: 0,
  };
}

const rows: readonly DatabaseRow[] = [
  row('n1', [
    { kind: 'select', value: 'doing' },
    { kind: 'multiselect', value: [] },
    { kind: 'date', value: '2026-10-20' },
  ]),
  row('n2', [
    { kind: 'select', value: 'todo' },
    { kind: 'multiselect', value: ['one'] },
    { kind: 'unset' },
  ]),
  row('n3', [
    { kind: 'select', value: 'doing' },
    { kind: 'multiselect', value: ['one'] },
    { kind: 'date', value: '2026-10-01' },
  ]),
  row('n4', [
    { kind: 'text', value: 'doing' },
    { kind: 'multiselect', value: ['a', 'b'] },
    { kind: 'datetime', value: '2026-10-21T10:00:00' },
  ]),
];

describe('board grouping', () => {
  it('groups select values and puts the ungrouped column last', () => {
    // F23-R01: no value goes to its own group, never into a real choice.
    const groups = groupRowsBySelect(table(rows), 'status');
    expect(groups.map((group) => group.value)).toEqual(['doing', 'todo', null]);
    expect(groups[0]?.rows.map((entry) => entry.noteId)).toEqual(['n1', 'n3']);
    expect(groups[1]?.rows.map((entry) => entry.noteId)).toEqual(['n2']);
    expect(groups[2]?.rows.map((entry) => entry.noteId)).toEqual(['n4']);
  });

  it('groups a single multi-select choice but not a multi-value cell', () => {
    const groups = groupRowsBySelect(table(rows), 'tags');
    expect(groups[0]).toMatchObject({ value: 'one' });
    expect(groups[0]?.rows.map((entry) => entry.noteId)).toEqual(['n2', 'n3']);
    expect(groups.at(-1)?.value).toBeNull();
    expect(groups.at(-1)?.rows.map((entry) => entry.noteId)).toEqual(['n1', 'n4']);
  });

  it('treats a missing column as one ungrouped bucket', () => {
    const groups = groupRowsBySelect(table(rows), 'missing');
    expect(groups).toEqual([{ value: null, rows: [...rows] }]);
  });
});

describe('writeback inputs', () => {
  it('offers each existing select value once, sorted, ignoring other kinds', () => {
    // The text cell in n4 spells "doing" but is not a select property.
    expect(selectChoices(table(rows), 'status')).toEqual(['doing', 'todo']);
    expect(selectChoices(table(rows), 'tags')).toEqual([]);
    expect(selectChoices(table(rows), 'missing')).toEqual([]);
  });

  it('reads only a real stored date for the bound field', () => {
    const source = table(rows);
    expect(dateCellValue(rows[0]!, source, 'due')).toBe('2026-10-20');
    expect(dateCellValue(rows[1]!, source, 'due')).toBeNull();
    expect(dateCellValue(rows[3]!, source, 'due')).toBeNull();
    expect(dateCellValue(rows[0]!, source, 'missing')).toBeNull();
  });
});

describe('calendar rows', () => {
  it('binds only the named date field and sorts by date then note id', () => {
    const calendar = calendarRows(table(rows), 'due');
    expect(calendar.dated.map((entry) => `${entry.date}:${entry.row.noteId}`)).toEqual([
      '2026-10-01:n3',
      '2026-10-20:n1',
    ]);
    // A datetime is not a calendar day; unset and text stay unscheduled too.
    expect(calendar.unscheduled.map((entry) => entry.noteId)).toEqual(['n2', 'n4']);
  });

  it('puts every row in the unscheduled list when the field is absent', () => {
    const calendar = calendarRows(table(rows), 'missing');
    expect(calendar.dated).toEqual([]);
    expect(calendar.unscheduled.map((entry) => entry.noteId)).toEqual(['n1', 'n2', 'n3', 'n4']);
  });
});

describe('month grid', () => {
  it('uses the actual early year for weekday layout and labels', () => {
    expect(monthGrid('0001-01', 1).slice(0, 3)).toEqual(['0001-01-01', '0001-01-02', '0001-01-03']);
    expect(monthGrid('0004-02', 1).filter(cell => cell !== null)).toHaveLength(29);
    expect(formatMonthLabel('0042-06', 'en-US')).toBe('June 42');
    expect(shiftMonth('0099-12', 1)).toBe('0100-01');
    expect(currentMonth(new Date('0042-06-01T12:00:00'))).toBe('0042-06');
    const early = row('early', [{ kind: 'select', value: '' }, { kind: 'multiselect', value: [] }, { kind: 'date', value: '0004-02-29' }]);
    expect(dateCellValue(early, table([early]), 'due')).toBe('0004-02-29');
    expect(calendarRows(table([early]), 'due').dated.map(entry => entry.date)).toEqual(['0004-02-29']);
  });

  it.each([
    ['2026-13', 1], ['9999-12', 1], ['0001-01', -1],
    ['2026-10', Number.NaN], ['2026-10', 1.5],
  ] as const)('refuses invalid month navigation %s by %s', (month, step) => {
    expect(shiftMonth(month, step)).toBe(month);
  });

  it('falls back to Monday for fractional week starts', () => {
    expect(monthGrid('2026-10', 1.5)).toEqual(monthGrid('2026-10', 1));
  });

  it('starts on Monday when the region says so', () => {
    const cells = monthGrid('2026-10', 1);
    expect(cells.slice(0, 4)).toEqual([null, null, null, '2026-10-01']);
    expect(cells[33]).toBe('2026-10-31');
    expect(cells).toHaveLength(35);
  });

  it('starts on Sunday when the region says so', () => {
    const cells = monthGrid('2026-10', 7);
    expect(cells.slice(0, 5)).toEqual([null, null, null, null, '2026-10-01']);
    expect(cells[34]).toBe('2026-10-31');
  });

  it('does not leak days from an adjacent month', () => {
    const cells = monthGrid('2026-02', 1);
    expect(cells.filter((cell) => cell !== null)).toHaveLength(28);
    expect(cells[cells.length - 1]).toBeNull();
  });

  it('rejects an impossible month instead of inventing cells', () => {
    expect(monthGrid('2026-13', 1)).toEqual([]);
    expect(monthGrid('October', 1)).toEqual([]);
  });
});

describe('month navigation', () => {
  it('crosses a year boundary in both directions', () => {
    expect(shiftMonth('2026-12', 1)).toBe('2027-01');
    expect(shiftMonth('2026-01', -1)).toBe('2025-12');
    expect(shiftMonth('2026-10', 0)).toBe('2026-10');
    expect(shiftMonth('bad', 1)).toBe('bad');
  });

  it('reads the current month from the device date', () => {
    expect(currentMonth(new Date(2026, 9, 21))).toBe('2026-10');
  });

  it('labels the month in the chosen region and falls back verbatim', () => {
    expect(formatMonthLabel('2026-10', 'en-US')).toContain('October');
    expect(formatMonthLabel('2026-13', 'en-US')).toBe('2026-13');
  });
});
