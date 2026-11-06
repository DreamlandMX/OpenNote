import { describe, expect, it } from 'vitest';
import type {
  BacklinkReport,
  EmbedExpansion,
  LinkReport,
  NoteMetadataReport,
  TagReport,
} from '../editor/ports';
import {
  backlinkRows,
  embedPlaceholder,
  linkIssueRows,
  noteSections,
  tabAvailability,
  tagRows,
  unreadableRows,
} from './view-model';

function tagReport(): TagReport {
  return {
    tags: [
      { tag: 'zeta', noteIds: ['n2'], notePaths: ['notes/z.md'] },
      { tag: 'alpha', noteIds: ['n1', 'n2'], notePaths: ['notes/a.md', 'notes/z.md'] },
    ],
    unreadable: [],
  };
}

describe('tagRows', () => {
  it('sorts by tag name regardless of host order and carries the note count', () => {
    const rows = tagRows(tagReport());
    expect(rows.map(row => row.label)).toEqual(['alpha', 'zeta']);
    expect(rows[0]?.detail).toBe('2');
    expect(rows[0]?.noteId).toBe('n1');
    expect(rows.every(row => row.offset === null)).toBe(true);
  });
});

describe('backlinkRows', () => {
  it('collapses several links from one note into a single row', () => {
    const report: BacklinkReport = {
      noteId: 'target',
      backlinks: [
        {
          fromNoteId: 'n1',
          fromPath: 'notes/a.md',
          kind: 'link',
          label: null,
          anchor: null,
          offset: 4,
        },
        {
          fromNoteId: 'n1',
          fromPath: 'notes/a.md',
          kind: 'link',
          label: null,
          anchor: 'Head',
          offset: 30,
        },
        {
          fromNoteId: 'n2',
          fromPath: 'notes/b.md',
          kind: 'embed',
          label: null,
          anchor: null,
          offset: 0,
        },
      ],
      unreadable: [],
    };
    const rows = backlinkRows(report);
    expect(rows).toHaveLength(2);
    expect(rows.map(row => row.label)).toEqual(['notes/a.md', 'notes/b.md']);
    // Two references from notes/a.md, one of them anchored.
    expect(rows[0]?.detail).toBe('2 (1)');
    expect(rows[0]?.offset).toBe(4);
    expect(rows[1]?.detail).toBe('1');
  });

  it('returns nothing for a note with no backlinks', () => {
    expect(
      backlinkRows({ noteId: 'n', backlinks: [], unreadable: [] }),
    ).toEqual([]);
  });
});

describe('linkIssueRows', () => {
  it('lists broken targets before missing anchors and keeps candidates visible', () => {
    const report: LinkReport = {
      broken: [
        {
          fromNoteId: 'n1',
          fromPath: 'notes/a.md',
          target: 'Dup',
          label: null,
          anchor: null,
          status: 'ambiguousTarget',
          candidates: ['notes/dup.md', 'other/dup.md'],
          offset: 10,
        },
      ],
      missingAnchors: [
        {
          fromNoteId: 'n2',
          fromPath: 'notes/b.md',
          target: 'notes/c',
          label: null,
          anchor: 'Gone',
          status: 'anchorMissing',
          candidates: [],
          offset: 3,
        },
      ],
      unreadable: [],
    };
    const rows = linkIssueRows(report);
    expect(rows).toHaveLength(2);
    expect(rows[0]?.label).toBe('Dup');
    expect(rows[0]?.detail).toBe('notes/dup.md, other/dup.md');
    expect(rows[1]?.label).toBe('#Gone');
    expect(rows[1]?.detail).toBe('notes/b.md');
  });

  it('keeps a written alias visible on a broken target', () => {
    const rows = linkIssueRows({
      broken: [
        {
          fromNoteId: 'n1',
          fromPath: 'notes/a.md',
          target: 'missing-note',
          label: 'Nice label',
          anchor: null,
          status: 'brokenLink',
          candidates: [],
          offset: 7,
        },
      ],
      missingAnchors: [],
      unreadable: [],
    });
    expect(rows[0]?.label).toBe('missing-note');
    expect(rows[0]?.detail).toBe('Nice label · notes/a.md');
  });

  it('does not repeat an alias that equals the target', () => {
    const rows = linkIssueRows({
      broken: [
        {
          fromNoteId: 'n1',
          fromPath: 'notes/a.md',
          target: 'same',
          label: 'same',
          anchor: null,
          status: 'brokenLink',
          candidates: [],
          offset: 0,
        },
      ],
      missingAnchors: [],
      unreadable: [],
    });
    expect(rows[0]?.detail).toBe('notes/a.md');
  });

  it('labels a same-note anchor problem with its anchor text', () => {
    const rows = linkIssueRows({
      broken: [
        {
          fromNoteId: 'n1',
          fromPath: 'a.md',
          target: '',
          label: null,
          anchor: 'Missing',
          status: 'brokenLink',
          candidates: [],
          offset: 0,
        },
      ],
      missingAnchors: [],
      unreadable: [],
    });
    expect(rows[0]?.label).toBe('#Missing');
  });
});

describe('unreadableRows', () => {
  it('surfaces unreadable notes with their message key', () => {
    const rows = unreadableRows([
      { noteId: 'n9', relativePath: 'notes/bad.md', code: 'IO_DENIED', messageKey: 'error.invalidEncoding' },
    ]);
    expect(rows[0]?.label).toBe('notes/bad.md');
    expect(rows[0]?.detail).toBe('error.invalidEncoding');
  });
});

describe('embedPlaceholder', () => {
  const base: EmbedExpansion = {
    status: 'ok',
    targetId: 'n2',
    targetPath: 'notes/b.md',
    anchor: null,
    content: 'body',
    chain: ['n1', 'n2'],
  };

  it('returns null for a successful expansion so real content renders', () => {
    expect(embedPlaceholder(base)).toBeNull();
  });

  it('keeps a readable placeholder for every failure, including cycles', () => {
    expect(embedPlaceholder({ ...base, status: 'brokenLink' })).toBe('embed.broken|notes/b.md');
    expect(embedPlaceholder({ ...base, status: 'anchorMissing' })).toBe(
      'embed.anchorMissing|notes/b.md',
    );
    expect(embedPlaceholder({ ...base, status: 'cycle', content: null })).toBe(
      'embed.cycle|n1 > n2',
    );
    expect(embedPlaceholder({ ...base, status: 'depthExceeded' })).toBe(
      'embed.depth|notes/b.md',
    );
    expect(embedPlaceholder({ ...base, status: 'ambiguousTarget' })).toBe(
      'embed.ambiguous|notes/b.md',
    );
  });
});

describe('tabAvailability', () => {
  it('keeps the vault-wide tabs usable with no note open', () => {
    expect(tabAvailability(null)).toEqual({ tags: true, backlinks: false, issues: true });
  });

  it('enables the per-note tab once a note is selected', () => {
    expect(tabAvailability('n1')).toEqual({ tags: true, backlinks: true, issues: true });
  });
});

describe('noteSections', () => {
  it('always returns the four sections in a fixed order', () => {
    const report: NoteMetadataReport = {
      noteId: 'n1',
      relativePath: 'notes/a.md',
      title: 'a',
      headings: [{ level: 2, text: 'Two', slug: 'two', offset: 0, line: 0 }],
      blockAnchors: [],
      tags: [{ tag: 'x', offset: 12, source: 'body' }],
      links: [
        {
          link: {
            kind: 'link',
            target: 'notes/b',
            label: null,
            anchor: null,
            offset: 20,
            length: 12,
          },
          resolved: {
            targetId: 'n2',
            targetPath: 'notes/b.md',
            anchor: null,
            status: 'resolved',
            candidates: [],
          },
        },
      ],
      backlinks: [],
      unreadable: [],
    };
    const sections = noteSections(report);
    expect(sections.map(section => section.titleKey)).toEqual([
      'metadata.headings',
      'metadata.tags',
      'metadata.links',
      'metadata.backlinks',
    ]);
    expect(sections[0]?.rows[0]?.label).toBe('## Two');
    expect(sections[0]?.rows[0]?.detail).toBe('two');
    expect(sections[2]?.rows[0]?.noteId).toBe('n2');
    expect(sections[3]?.empty).toBe(true);
  });

  it('marks empty sections instead of omitting them', () => {
    const sections = noteSections({
      noteId: 'n1',
      relativePath: 'a.md',
      title: 'a',
      headings: [],
      blockAnchors: [],
      tags: [],
      links: [],
      backlinks: [],
      unreadable: [],
    });
    expect(sections.every(section => section.empty)).toBe(true);
  });
});
