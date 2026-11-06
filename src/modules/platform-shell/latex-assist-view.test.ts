import { describe, expect, it } from 'vitest';
import type { AssistResult, CompletionCandidate } from '../editor';
import {
  applyResult,
  completionRange,
  describeResult,
  editsAreSafe,
  findCandidate,
  groupCandidates,
  isBlockedByComposition,
  isValidRange,
  languageLabel,
  previewEdit,
} from './latex-assist-view';

function result(overrides: Partial<AssistResult> = {}): AssistResult {
  return {
    edits: [{ range: { start: 0, end: 3 }, text: 'X', kind: 'test' }],
    selection: { start: 1, end: 1 },
    warnings: [],
    ...overrides,
  };
}

describe('range validation', () => {
  it('rejects an out-of-bounds or inverted range', () => {
    expect(isValidRange('abc', { start: 0, end: 3 })).toBe(true);
    expect(isValidRange('abc', { start: 0, end: 4 })).toBe(false);
    expect(isValidRange('abc', { start: -1, end: 2 })).toBe(false);
    expect(isValidRange('abc', { start: 2, end: 1 })).toBe(false);
  });

  it('rejects a range that splits a surrogate pair', () => {
    // "😀" is two UTF-16 units; offset 1 is inside it.
    const text = '😀x';
    expect(isValidRange(text, { start: 0, end: 2 })).toBe(true);
    expect(isValidRange(text, { start: 1, end: 2 })).toBe(false);
  });

  it('refuses a result with any unsafe range', () => {
    expect(editsAreSafe('abc', [{ range: { start: 0, end: 1 }, text: 'x', kind: 'k' }])).toBe(true);
    expect(editsAreSafe('abc', [{ range: { start: 0, end: 9 }, text: 'x', kind: 'k' }])).toBe(false);
    expect(
      editsAreSafe('abc', [
        { range: { start: 0, end: 1 }, text: 'x', kind: 'k' },
        { range: { start: 5, end: 6 }, text: 'y', kind: 'k' },
      ]),
    ).toBe(false);
  });
});

describe('applying and previewing', () => {
  it('applies edits back to front so offsets stay valid', () => {
    const applied = applyResult('aaa bbb', {
      edits: [
        { range: { start: 0, end: 3 }, text: 'X', kind: 'k' },
        { range: { start: 4, end: 7 }, text: 'Y', kind: 'k' },
      ],
      selection: { start: 0, end: 0 },
      warnings: [],
    });
    expect(applied).toBe('X Y');
  });

  it('previews what each edit replaces', () => {
    const preview = previewEdit('alpha beta', result());
    expect(preview).toHaveLength(1);
    expect(preview[0]?.before).toBe('alp');
    expect(preview[0]?.after).toBe('X');
    expect(preview[0]?.kind).toBe('test');
  });

  it('describes a result for the status line', () => {
    expect(describeResult(result())).toEqual({ edits: 1, kind: 'test' });
    expect(
      describeResult({ edits: [], selection: { start: 0, end: 0 }, warnings: [] }),
    ).toEqual({ edits: 0, kind: 'none' });
  });
});

describe('composition guard', () => {
  it('blocks every assist action during a composition', () => {
    // F47-R03: pairing or wrapping mid-composition swallows typed text.
    expect(isBlockedByComposition(true)).toBe(true);
    expect(isBlockedByComposition(false)).toBe(false);
  });
});

describe('language label', () => {
  it('names the detected language without claiming more than detection did', () => {
    expect(languageLabel('tex')).toBe('TeX');
    expect(languageLabel('bib')).toBe('BibTeX');
    expect(languageLabel('plain')).toBe('Plain text');
  });
});

describe('completions', () => {
  const candidates: CompletionCandidate[] = [
    { label: '\\section', insert: '\\section{}', source: 'lexicon' },
    { label: '\\custom', insert: '\\custom', source: 'project' },
  ];

  it('finds a candidate by exact label so a re-preview cannot pick the wrong one', () => {
    expect(findCandidate(candidates, '\\section')?.insert).toBe('\\section{}');
    expect(findCandidate(candidates, '\\sect')).toBeNull();
  });

  it('covers only the typed prefix, never earlier text', () => {
    const text = 'text \\sec';
    const found = completionRange(text, text.length);
    expect(found?.prefix).toBe('\\sec');
    expect(found?.range).toEqual({ start: 5, end: 9 });
  });

  it('returns null when the caret is not in a command', () => {
    expect(completionRange('plain', 5)).toBeNull();
    // A single backslash is not yet a command prefix.
    expect(completionRange('\\', 1)).toBeNull();
    expect(completionRange('text', 99)).toBeNull();
  });

  it('groups candidates by origin so the list is explainable', () => {
    const groups = groupCandidates(candidates);
    expect(groups.map((group) => group.source)).toEqual(['lexicon', 'project']);
    expect(groups[1]?.labels).toEqual(['\\custom']);
  });

  it('omits an empty group', () => {
    expect(groupCandidates([candidates[1]!])).toEqual([
      { source: 'project', labels: ['\\custom'] },
    ]);
  });
});
