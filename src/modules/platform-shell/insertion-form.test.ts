import { describe, expect, it } from 'vitest';
import type { Bibliography, InsertParameters, InsertionPlan } from '../editor';
import {
  bibliographySummary,
  canInsert,
  citationItems,
  citableItems,
  DEFAULT_IMAGE_WIDTH,
  dependencyInsertion,
  fieldIssue,
  filterCitations,
  insertionEdits,
  insertionPointConflicts,
  isInsertionStale,
  issuesFor,
  labelPrefix,
  missingDependencyLines,
  parseTableInput,
  sameInsertParameters,
  suggestLabel,
} from './insertion-form';

function plan(overrides: Partial<InsertionPlan> = {}): InsertionPlan {
  return {
    fragment: '\\section{x}\n',
    dependencies: [],
    issues: [],
    label: null,
    ...overrides,
  };
}

function bibliography(): Bibliography {
  return {
    entries: [
      {
        citeKey: 'smith2020',
        entryType: 'article',
        fields: [
          { name: 'author', value: 'Smith' },
          { name: 'title', value: 'A Paper' },
        ],
        unknownType: false,
        offset: 0,
        raw: '',
      },
      {
        citeKey: 'data1',
        entryType: 'dataset',
        fields: [{ name: 'title', value: 'Data' }],
        unknownType: true,
        offset: 40,
        raw: '',
      },
    ],
    errors: [],
    duplicates: [],
  };
}

describe('issue mapping', () => {
  it('points each issue at the field that caused it', () => {
    expect(fieldIssue({ kind: 'missing', field: 'body' }).field).toBe('body');
    expect(fieldIssue({ kind: 'missing', field: 'path' }).field).toBe('path');
    expect(fieldIssue({ kind: 'missing', field: 'title' }).field).toBe('title');
    expect(fieldIssue({ kind: 'invalidLabel', label: 'a b' }).field).toBe('label');
    expect(fieldIssue({ kind: 'duplicateLabel', label: 'eq:1', existing: ['eq:1'] }).field).toBe(
      'label',
    );
    expect(fieldIssue({ kind: 'unsafeImagePath', path: '../x' }).field).toBe('path');
    expect(fieldIssue({ kind: 'invalidWidth', value: 2 }).field).toBe('width');
    expect(fieldIssue({ kind: 'malformedTable', detail: 'row 2' }).field).toBe('rows');
    expect(fieldIssue({ kind: 'invalidLevel', level: 9 }).field).toBe('level');
    expect(fieldIssue({ kind: 'unknownLabel', label: 'eq:9' }).field).toBe('label');
  });

  it('groups issues by field and ignores a null plan', () => {
    const value = plan({
      issues: [
        { kind: 'invalidLabel', label: 'a b' },
        { kind: 'invalidWidth', value: 3 },
      ],
    });
    expect(issuesFor(value, 'label')).toHaveLength(1);
    expect(issuesFor(value, 'width')).toHaveLength(1);
    expect(issuesFor(value, 'body')).toHaveLength(0);
    expect(issuesFor(null, 'label')).toEqual([]);
  });
});

describe('gating', () => {
  it('allows insertion only with no issues and a fragment', () => {
    expect(canInsert(plan())).toBe(true);
    expect(canInsert(plan({ issues: [{ kind: 'invalidLevel', level: 0 }] }))).toBe(false);
    expect(canInsert(plan({ fragment: '   ' }))).toBe(false);
    expect(canInsert(null)).toBe(false);
  });

  it('lists only the dependency lines that are missing', () => {
    const value = plan({
      dependencies: [
        { kind: 'usepackage', name: 'graphicx', line: '\\usepackage{graphicx}', present: false },
        { kind: 'usepackage', name: 'hyperref', line: '\\usepackage{hyperref}', present: true },
      ],
    });
    expect(missingDependencyLines(value)).toEqual(['\\usepackage{graphicx}']);
    expect(missingDependencyLines(null)).toEqual([]);
  });

  it('marks a plan stale when the note changed after the preview', () => {
    // F48 §7 STALE_INSERT_POINT: label checks belong to the previewed text.
    expect(isInsertionStale(null, 'note')).toBe(false);
    expect(isInsertionStale('note', 'note')).toBe(false);
    expect(isInsertionStale('note', 'note with a new \\label{eq:1}')).toBe(true);
  });
});

describe('dependency insertion', () => {
  const document =
    '\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\nBody\n\\end{document}\n';

  it('places confirmed package lines immediately before the document body', () => {
    // F48-R03/R04: a preamble change is computed, never silently applied.
    const edit = dependencyInsertion(document, ['\\usepackage{graphicx}']);
    expect(edit).not.toBeNull();
    expect(edit?.start).toBe(document.indexOf('\\begin{document}'));
    expect(edit?.end).toBe(edit?.start);
    expect(edit?.text).toBe('\\usepackage{graphicx}\n');
  });

  it('trims, deduplicates and drops blank dependency lines', () => {
    const edit = dependencyInsertion(document, [
      '  \\usepackage{graphicx}  ',
      '',
      '\\usepackage{graphicx}',
      '\\usepackage{booktabs}',
    ]);
    expect(edit?.text).toBe('\\usepackage{graphicx}\n\\usepackage{booktabs}\n');
  });

  it('keeps the source newline style', () => {
    const crlf = document.replace(/\n/g, '\r\n');
    const edit = dependencyInsertion(crlf, ['\\usepackage{graphicx}']);
    expect(edit?.text).toBe('\\usepackage{graphicx}\r\n');
    expect(edit?.start).toBe(crlf.indexOf('\\begin{document}'));
  });

  it('offers no automatic point without a document body', () => {
    // A section file has no preamble of its own; the lines stay manual.
    expect(dependencyInsertion('\\section{x}\n', ['\\usepackage{graphicx}'])).toBeNull();
    expect(dependencyInsertion(document, [])).toBeNull();
    expect(dependencyInsertion(document, ['   '])).toBeNull();
  });

  it('refuses a document body that does not start its own line', () => {
    const inline = '\\documentclass{article}\nText \\begin{document}\n';
    expect(dependencyInsertion(inline, ['\\usepackage{graphicx}'])).toBeNull();
    const commented = '\\documentclass{article}\n% \\begin{document}\n';
    expect(dependencyInsertion(commented, ['\\usepackage{graphicx}'])).toBeNull();
  });

  it('detects a point that would split the fragment selection', () => {
    expect(insertionPointConflicts(10, 10, 20)).toBe(true);
    expect(insertionPointConflicts(15, 10, 20)).toBe(true);
    expect(insertionPointConflicts(20, 10, 20)).toBe(false);
    expect(insertionPointConflicts(10, 10, 10)).toBe(true);
    expect(insertionPointConflicts(0, 10, 10)).toBe(false);
  });

  it('returns the fragment and confirmed package lines as one edit list', () => {
    // F48-R04: applying this list is one editor transaction, one undo step.
    const range = { from: document.length, to: document.length };
    const withPackages = insertionEdits(
      document,
      '\\section{x}\n',
      range,
      ['\\usepackage{graphicx}'],
      true,
    );
    expect(withPackages).toHaveLength(2);
    expect(withPackages?.[0]).toEqual({ start: range.from, end: range.to, text: '\\section{x}\n' });
    expect(withPackages?.[1]?.text).toBe('\\usepackage{graphicx}\n');

    const fragmentOnly = insertionEdits(
      document,
      '\\section{x}\n',
      range,
      ['\\usepackage{graphicx}'],
      false,
    );
    expect(fragmentOnly).toEqual([{ start: range.from, end: range.to, text: '\\section{x}\n' }]);
  });

  it('refuses a package change that cannot be placed safely', () => {
    const range = { from: 0, to: 0 };
    expect(insertionEdits('\\section{x}\n', 'x', range, ['\\usepackage{graphicx}'], true)).toBeNull();
    const point = document.indexOf('\\begin{document}');
    expect(
      insertionEdits(document, 'x', { from: point, to: point }, ['\\usepackage{graphicx}'], true),
    ).toBeNull();
  });
});

describe('form helpers', () => {
  it('parses a plain table with one row per line', () => {
    expect(parseTableInput('a | b\n1 | 2\n')).toEqual([
      ['a', 'b'],
      ['1', '2'],
    ]);
    expect(parseTableInput('')).toEqual([]);
    // Blank lines are ignored rather than becoming empty rows.
    expect(parseTableInput('a|b\n\n')).toEqual([['a', 'b']]);
  });

  it('suggests the documented label prefixes', () => {
    expect(labelPrefix('equation')).toBe('eq:');
    expect(labelPrefix('image')).toBe('fig:');
    expect(labelPrefix('table')).toBe('tab:');
    expect(labelPrefix('section')).toBe('');
  });

  it('suggests a label that does not collide', () => {
    expect(suggestLabel('equation', [])).toBe('eq:1');
    expect(suggestLabel('equation', ['eq:1'])).toBe('eq:2');
    expect(suggestLabel('section', ['x'])).toBe('');
  });

  it('uses a width inside the documented range', () => {
    expect(DEFAULT_IMAGE_WIDTH).toBeGreaterThanOrEqual(0.1);
    expect(DEFAULT_IMAGE_WIDTH).toBeLessThanOrEqual(1.0);
  });
});

describe('preview binding', () => {
  const equation: InsertParameters = { kind: 'equation', body: 'x', label: null };

  it('keeps a plan valid only while the form values match', () => {
    expect(sameInsertParameters(null, equation)).toBe(false);
    expect(sameInsertParameters(equation, { kind: 'equation', body: 'x', label: null })).toBe(true);
    expect(sameInsertParameters(equation, { kind: 'equation', body: 'y', label: null })).toBe(false);
    expect(sameInsertParameters(equation, { kind: 'equation', body: 'x', label: 'eq:1' })).toBe(
      false,
    );
    expect(sameInsertParameters(equation, { kind: 'section', title: 'x', level: 1, label: null })).toBe(
      false,
    );
  });

  it('compares table rows by value, not by array identity', () => {
    const table: InsertParameters = {
      kind: 'table',
      rows: [
        ['a', 'b'],
        ['1', '2'],
      ],
      advanced: null,
      caption: null,
      label: null,
    };
    const same: InsertParameters = {
      kind: 'table',
      rows: [
        ['a', 'b'],
        ['1', '2'],
      ],
      advanced: null,
      caption: null,
      label: null,
    };
    expect(sameInsertParameters(table, same)).toBe(true);
    const changed: InsertParameters = {
      ...same,
      rows: [
        ['a', 'b'],
        ['1', '3'],
      ],
    };
    expect(sameInsertParameters(table, changed)).toBe(false);
  });
});

describe('bibliography lists', () => {
  it('labels an entry with author and title so a screen reader can announce it', () => {
    const items = citationItems(bibliography());
    expect(items[0]?.label).toBe('Smith — A Paper');
    // An entry without an author falls back to its title.
    expect(items[1]?.label).toBe('Data');
    expect(items[1]?.unknownType).toBe(true);
  });

  it('falls back to the key when there is nothing else', () => {
    const bare: Bibliography = {
      entries: [
        { citeKey: 'k1', entryType: 'misc', fields: [], unknownType: false, offset: 0, raw: '' },
      ],
      errors: [],
      duplicates: [],
    };
    expect(citationItems(bare)[0]?.label).toBe('k1');
  });

  it('excludes a duplicated key from the citable list', () => {
    // DUPLICATE_CITE_KEY: the user must not pick one silently.
    const duplicated: Bibliography = {
      ...bibliography(),
      duplicates: [{ citeKey: 'smith2020', offsets: [0, 40] }],
    };
    expect(citableItems(duplicated).map((item) => item.citeKey)).toEqual(['data1']);
    expect(citationItems(duplicated)[0]?.duplicate).toBe(true);
  });

  it('keeps every row attached to its exact entry, including duplicates', () => {
    const duplicated: Bibliography = {
      ...bibliography(),
      duplicates: [{ citeKey: 'smith2020', offsets: [0, 40] }],
    };
    expect(citationItems(duplicated).map((item) => item.entryIndex)).toEqual([0, 1]);
    expect(citationItems(duplicated).map((item) => item.offset)).toEqual([0, 40]);
  });

  it('filters by key or label text', () => {
    const items = citationItems(bibliography());
    expect(filterCitations(items, 'smith').map((item) => item.citeKey)).toEqual(['smith2020']);
    expect(filterCitations(items, 'data').map((item) => item.citeKey)).toEqual(['data1']);
    expect(filterCitations(items, '  ')).toHaveLength(2);
    expect(filterCitations(items, 'zzz')).toEqual([]);
  });

  it('summarizes what the parser reported', () => {
    const duplicated: Bibliography = {
      ...bibliography(),
      errors: [{ offset: 99, raw: '@broken', messageKey: 'error.bibEntryUnterminated' }],
      duplicates: [{ citeKey: 'data1', offsets: [40, 80] }],
    };
    expect(bibliographySummary(duplicated)).toEqual({
      entries: 2,
      duplicates: 1,
      errors: 1,
      unknownTypes: 1,
    });
  });
});
