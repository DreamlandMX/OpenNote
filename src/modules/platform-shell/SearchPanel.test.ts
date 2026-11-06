import { afterEach, describe, expect, it, vi } from 'vitest';
import { foldersOf, snippetParts, searchSourceKey } from './SearchPanel';
import { rememberSearchHistoryQuery } from './device-prefs';
import type { NoteSummary } from '../domain';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('searchSourceKey', () => {
  const a: NoteSummary = { noteId: 'a', vaultId: 'vault', relativePath: 'notes/a.md', contentHash: 'hash-a', localRevision: 1, readOnly: false };
  const b: NoteSummary = { ...a, noteId: 'b', relativePath: 'notes/b.md' };
  it('changes for content, identity, paths, availability and deletion', () => {
    for (const next of [{...a,contentHash:'new'}, {...a,noteId:'other'}, {...a,vaultId:'other'}, {...a,relativePath:'moved/a.md'}, {...a,readOnly:true}]) {
      expect(searchSourceKey([next,b])).not.toBe(searchSourceKey([a,b]));
    }
    expect(searchSourceKey([a])).not.toBe(searchSourceKey([a,b]));
  });
  it('ignores new arrays, ordering and revisions when source bytes and identity did not change', () => {
    expect(searchSourceKey([{...b,localRevision:8},{...a,localRevision:2}])).toBe(searchSourceKey([a,b]));
  });
});

describe('snippetParts', () => {
  it('splits a snippet at UTF-16 highlight offsets', () => {
    expect(snippetParts({ snippet: '好的知识库是系统', highlights: [{ start: 2, end: 5 }] })).toEqual([
      { text: '好的', highlight: false },
      { text: '知识库', highlight: true },
      { text: '是系统', highlight: false },
    ]);
  });

  it('keeps astral characters aligned with the host offsets', () => {
    const parts = snippetParts({
      snippet: 'a😀b hit',
      highlights: [{ start: 5, end: 8 }],
    });
    expect(parts).toEqual([
      { text: 'a😀b ', highlight: false },
      { text: 'hit', highlight: true },
    ]);
  });

  it('clips out-of-range and overlapping highlights without inventing text', () => {
    expect(
      snippetParts({
        snippet: 'short',
        highlights: [
          { start: 2, end: 99 },
          { start: 1, end: 3 },
        ],
      }),
    ).toEqual([
      { text: 's', highlight: false },
      { text: 'hort', highlight: true },
    ]);
  });

  it('marks Turkish text by code unit, not by byte', () => {
    const parts = snippetParts({
      snippet: "İstanbul'da düğün",
      highlights: [{ start: 12, end: 17 }],
    });
    expect(parts.filter((part) => part.highlight).map((part) => part.text)).toEqual(['düğün']);
  });
});

describe('foldersOf', () => {
  it('derives every folder prefix once and sorts them', () => {
    expect(
      foldersOf([
        { relativePath: 'a/b/one.md' },
        { relativePath: 'a/two.md' },
        { relativePath: 'root.md' },
      ] as never),
    ).toEqual(['a', 'a/b']);
  });
});

describe('rememberSearchHistoryQuery', () => {
  it('stores only a real nonempty query and reports the device-local list', () => {
    const values = new Map<string, string>();
    vi.stubGlobal('window', {
      localStorage: {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => values.set(key, value),
      },
    });
    expect(rememberSearchHistoryQuery('  alpha  ')).toEqual(['alpha']);
    expect(rememberSearchHistoryQuery('')).toEqual(['alpha']);
    expect(rememberSearchHistoryQuery('alpha')).toEqual(['alpha']);
    expect(rememberSearchHistoryQuery('😀'.repeat(501))).toEqual(['alpha']);
  });
});
