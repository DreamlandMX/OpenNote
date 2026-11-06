import { describe, expect, it } from 'vitest';
import type { CitationPlan } from '../editor';
import type { Translator } from '../presentation-i18n';
import {
  bibliographyState,
  bibErrorMessageKey,
  bibEntrySelection,
  bibErrorSelection,
  canInsertCitation,
  citationIssueKey,
  citationIssueParams,
  citationIssueText,
  isSelectable,
  orderedSelection,
  pruneSelection,
  removeBibEntry,
  utf8OffsetToUtf16Index,
} from './citation-panel';

const translator: Translator = ((key: string, params?: Record<string, unknown>) =>
  params === undefined ? key : `${key}:${JSON.stringify(params)}`) as Translator;

function plan(overrides: Partial<CitationPlan> = {}): CitationPlan {
  return {
    fragment: '\\cite{a}\n',
    issues: [],
    hasBibliographyPackage: true,
    backend: 'bibtex',
    ...overrides,
  };
}

describe('insertion gating', () => {
  it('allows insertion only with no issues and a fragment', () => {
    expect(canInsertCitation(plan())).toBe(true);
    expect(canInsertCitation(plan({ issues: [{ kind: 'noKeys' }] }))).toBe(false);
    expect(canInsertCitation(plan({ fragment: '  ' }))).toBe(false);
    expect(canInsertCitation(null)).toBe(false);
  });

  it('never lets a duplicated key be selected', () => {
    // DUPLICATE_CITE_KEY: there is no single entry it would refer to.
    expect(isSelectable(false)).toBe(true);
    expect(isSelectable(true)).toBe(false);
  });
});

describe('issue text', () => {
  it('maps each issue to its key and parameters', () => {
    expect(citationIssueKey({ kind: 'unknownKey', citeKey: 'x' })).toBe(
      'citation.issue.unknownKey',
    );
    expect(citationIssueParams({ kind: 'duplicateKey', citeKey: 'd', offsets: [1, 2] })).toEqual({
      citeKey: 'd',
      count: 2,
    });
    expect(citationIssueKey({ kind: 'noKeys' })).toBe('citation.issue.noKeys');
    expect(citationIssueParams({ kind: 'noKeys' })).toEqual({});
  });

  it('resolves an issue to a sentence through the translator', () => {
    const text = citationIssueText({ kind: 'unknownKey', citeKey: 'gone' }, translator);
    expect(text).toContain('citation.issue.unknownKey');
    expect(text).toContain('gone');
  });
});

describe('selection handling', () => {
  it('drops a selected key that no longer exists after a re-parse', () => {
    // Keeping it would cite something the bibliography no longer has.
    expect(pruneSelection(['a', 'gone'], ['a', 'b'])).toEqual(['a']);
    expect(pruneSelection(['a'], [])).toEqual([]);
  });

  it('orders the selection by document order so the command is reproducible', () => {
    expect(orderedSelection(['c', 'a'], ['a', 'b', 'c'])).toEqual(['a', 'c']);
    expect(orderedSelection([], ['a'])).toEqual([]);
  });
});

describe('bibliography error location', () => {
  it('converts UTF-8 byte offsets to textarea UTF-16 indices', () => {
    expect(utf8OffsetToUtf16Index('@article', 0)).toBe(0);
    expect(utf8OffsetToUtf16Index('é@article', 2)).toBe(1);
    expect(utf8OffsetToUtf16Index('😀@article', 4)).toBe(2);
    // An offset inside a multi-byte character snaps to its start.
    expect(utf8OffsetToUtf16Index('😀@article', 2)).toBe(0);
    expect(utf8OffsetToUtf16Index('@article', 999)).toBe(8);
    expect(utf8OffsetToUtf16Index('@article', -1)).toBe(0);
  });

  it('selects the whole unparsable block, not just its first byte', () => {
    const prefix = '@comment{é}\n';
    const raw = '@article{broken';
    const source = `${prefix}${raw}`;
    // Rust reports a byte offset; JS indices count UTF-16 code units.
    const byteOffset = new TextEncoder().encode(prefix).length;
    expect(bibErrorSelection(source, { offset: byteOffset, raw })).toEqual({
      start: prefix.length,
      end: source.length,
    });
  });

  it('clamps a stale error range to the current source', () => {
    expect(bibErrorSelection('abc', { offset: 99, raw: 'gone' })).toEqual({ start: 3, end: 3 });
    expect(bibErrorSelection('😀@x', { offset: 4, raw: '@x' })).toEqual({ start: 2, end: 4 });
  });

  it('selects the exact entry block for a duplicate row', () => {
    const prefix = '@comment{😀}\n';
    const raw = '@article{same,\n title={A}}';
    const source = `${prefix}${raw}\n@article{same,\n title={B}}`;
    const offset = new TextEncoder().encode(prefix).length;
    expect(bibEntrySelection(source, { offset, raw })).toEqual({
      start: prefix.length,
      end: prefix.length + raw.length,
    });
  });
});

describe('bibliography parse error messages', () => {
  it('maps each parser key to a localized sentence key', () => {
    expect(bibErrorMessageKey('error.bibEntryMalformed')).toBe('citation.parseError.malformed');
    expect(bibErrorMessageKey('error.bibEntryUnterminated')).toBe(
      'citation.parseError.unterminated',
    );
    expect(bibErrorMessageKey('error.bibEntryNoKey')).toBe('citation.parseError.noKey');
  });

  it('falls back to the generic Issues label for an unknown future key', () => {
    expect(bibErrorMessageKey('error.somethingNew')).toBe('citation.issues');
  });
});

describe('bibliography entry removal', () => {
  it('removes the exact entry after a non-ASCII prefix and absorbs one newline', () => {
    const first = '@article{a,\n title={A}}';
    const second = '@book{b,\n title={B}}';
    const prefix = '% é comment\n';
    const source = `${prefix}${first}\n${second}`;
    const offset = new TextEncoder().encode(prefix).length;
    expect(removeBibEntry(source, { offset, raw: first })).toBe(`${prefix}${second}`);
  });

  it('handles CRLF and a last entry without a trailing newline', () => {
    expect(removeBibEntry('@a{}\r\n@b{}', { offset: 0, raw: '@a{}' })).toBe('@b{}');
    expect(removeBibEntry('@a{}\n@b{}', { offset: 5, raw: '@b{}' })).toBe('@a{}\n');
  });

  it('refuses a stale parse rather than deleting unrelated text', () => {
    const source = '@article{a}\n@book{b}';
    expect(removeBibEntry(source, { offset: 0, raw: '@article{gone}' })).toBeNull();
    expect(removeBibEntry(source, { offset: 99, raw: '@book{b}' })).toBeNull();
    expect(removeBibEntry(source, { offset: 0, raw: '' })).toBeNull();
  });
});

describe('bibliography state', () => {
  it('distinguishes empty, clean and duplicated', () => {
    expect(bibliographyState(0, 0)).toBe('empty');
    expect(bibliographyState(3, 0)).toBe('clean');
    expect(bibliographyState(3, 1)).toBe('duplicated');
  });
});
