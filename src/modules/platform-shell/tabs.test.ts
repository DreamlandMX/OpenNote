import { describe, expect, it } from 'vitest';
import type { NoteSummary } from '../domain';
import {
  closeTab,
  cycleTab,
  isFavorite,
  recentLabel,
  removeRecent,
  resolveFavorites,
  resolveRecent,
  tabNeedsGuard,
  toggleFavorite,
} from './tabs';

function note(id: string, path: string): NoteSummary {
  return {
    noteId: id,
    vaultId: 'v1',
    relativePath: path,
    contentHash: 'h',
    localRevision: 1,
    readOnly: false,
  };
}

const a = note('a', 'notes/a.md');
const b = note('b', 'notes/b.md');
const c = note('c', 'notes/c.md');

describe('workspace tabs', () => {
  it('closes an inactive tab without changing the active note', () => {
    const next = closeTab({ tabs: [a, b, c], activeId: 'a' }, 'b');
    expect(next.tabs.map((entry) => entry.noteId)).toEqual(['a', 'c']);
    expect(next.activeId).toBe('a');
  });

  it('activates the right neighbour, then the left, then none', () => {
    expect(closeTab({ tabs: [a, b, c], activeId: 'b' }, 'b')).toMatchObject({
      activeId: 'c',
    });
    expect(closeTab({ tabs: [a, b], activeId: 'b' }, 'b')).toMatchObject({ activeId: 'a' });
    expect(closeTab({ tabs: [a], activeId: 'a' }, 'a')).toEqual({ tabs: [], activeId: null });
  });

  it('ignores an unknown id and never invents a tab', () => {
    const state = { tabs: [a, b], activeId: 'a' };
    expect(closeTab(state, 'missing')).toBe(state);
  });

  it('cycles forward and backward with wraparound', () => {
    expect(cycleTab([a, b, c], 'a', 1)).toBe('b');
    expect(cycleTab([a, b, c], 'c', 1)).toBe('a');
    expect(cycleTab([a, b, c], 'a', -1)).toBe('c');
    expect(cycleTab([a, b, c], null, 1)).toBe('a');
    expect(cycleTab([], 'a', 1)).toBeNull();
  });

  it('keeps a remembered id available after the file is renamed', () => {
    const renamed = note('a', 'archive/renamed.md');
    const entries = resolveRecent(['a'], [renamed]);
    expect(entries[0]).toMatchObject({ noteId: 'a', available: true });
    expect(entries[0]?.note?.relativePath).toBe('archive/renamed.md');
    expect(recentLabel(entries[0]!)).toBe('renamed');
  });

  it('keeps an unavailable entry instead of dropping it silently', () => {
    const entries = resolveRecent(['gone', 'a'], [a]);
    expect(entries.map((entry) => entry.available)).toEqual([false, true]);
    expect(recentLabel(entries[0]!)).toBe('gone');
    expect(removeRecent(['gone', 'a'], 'gone')).toEqual(['a']);
  });

  it('de-duplicates remembered ids and preserves their order', () => {
    expect(resolveRecent(['a', 'a', 'b'], [a, b]).map((entry) => entry.noteId)).toEqual([
      'a',
      'b',
    ]);
  });

  it('flags only tabs with pending or failed work for the close guard', () => {
    expect(tabNeedsGuard(undefined)).toBe(false);
    expect(tabNeedsGuard({ status: 'clean' })).toBe(false);
    expect(tabNeedsGuard({ status: 'dirty' })).toBe(true);
    expect(tabNeedsGuard({ status: 'failed' })).toBe(true);
    expect(tabNeedsGuard({ status: 'conflict' })).toBe(true);
  });
});

describe('favorites', () => {
  it('keeps opening the same note after its path changes', () => {
    const renamed = note('a', 'archive/renamed.md');
    const entries = resolveFavorites(['a'], [renamed]);
    expect(entries[0]).toMatchObject({ noteId: 'a', available: true });
    expect(recentLabel(entries[0]!)).toBe('renamed');
  });

  it('keeps a deleted favorite visible as unavailable until removed', () => {
    const entries = resolveFavorites(['gone', 'a'], [a]);
    expect(entries.map((entry) => entry.available)).toEqual([false, true]);
    expect(toggleFavorite(['gone', 'a'], 'gone', 100)).toEqual(['a']);
  });

  it('adds newest first, de-duplicates and caps only on add', () => {
    expect(toggleFavorite(['a'], 'b', 3)).toEqual(['b', 'a']);
    expect(toggleFavorite(['b', 'a'], 'a', 3)).toEqual(['b']);
    expect(toggleFavorite(['a', 'b', 'c'], 'd', 3)).toEqual(['d', 'a', 'b']);
    expect(toggleFavorite(['a'], '', 3)).toEqual(['a']);
    expect(isFavorite(['a'], 'a')).toBe(true);
    expect(isFavorite(['a'], 'b')).toBe(false);
  });

  it('never converts a favorite into a tab or a second note copy', () => {
    const entries = resolveFavorites(['a', 'a'], [a]);
    expect(entries).toHaveLength(1);
    expect(entries[0]?.note?.contentHash).toBe('h');
  });
});
