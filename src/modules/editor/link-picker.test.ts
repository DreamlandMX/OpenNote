import { describe, expect, it } from 'vitest';
import {
  anchorTargetFor,
  applyAnchorInsertion,
  applyLinkInsertion,
  detectLinkQuery,
  insertAnchorText,
  insertLinkText,
  isAnchorMode,
  linkTargetFor,
  rankAnchors,
  rankCandidates,
  resolveLinkTarget,
  type AnchorCandidate,
  type LinkCandidate,
  type LinkQuery,
} from './link-picker';

function candidate(noteId: string, relativePath: string, title: string): LinkCandidate {
  return { noteId, relativePath, title };
}

const NOTES: LinkCandidate[] = [
  candidate('n1', 'notes/alpha.md', 'Alpha'),
  candidate('n2', 'notes/beta.md', 'Beta'),
  candidate('n3', 'other/dup.md', 'Dup'),
  candidate('n4', 'notes/dup.md', 'Dup'),
];

describe('detectLinkQuery', () => {
  it('opens for an unterminated [[ on one line', () => {
    const source = 'see [[al';
    const query = detectLinkQuery(source, source.length);
    expect(query).not.toBeNull();
    expect(query?.from).toBe(4);
    expect(query?.to).toBe(source.length);
    expect(query?.query).toBe('al');
    expect(query?.embed).toBe(false);
  });

  it('opens with an empty query right after the brackets', () => {
    const source = 'see [[';
    expect(detectLinkQuery(source, source.length)?.query).toBe('');
  });

  it('does not open once the link is terminated', () => {
    const source = 'see [[alpha]]';
    expect(detectLinkQuery(source, source.length)).toBeNull();
  });

  it('does not open across a newline, or after a bracket or backtick', () => {
    expect(detectLinkQuery('a [[x\nb', 8)).toBeNull();
    expect(detectLinkQuery('[[a[b', 5)).toBeNull();
    expect(detectLinkQuery('[[a`b', 5)).toBeNull();
  });

  it('does not open inside code', () => {
    const source = 'see [[al';
    expect(detectLinkQuery(source, source.length, () => true)).toBeNull();
  });

  it('marks an embed and includes the bang in the replaced range', () => {
    const source = 'x ![[be';
    const query = detectLinkQuery(source, source.length);
    expect(query?.embed).toBe(true);
    expect(query?.from).toBe(2);
    expect(query?.query).toBe('be');
  });

  it('keeps a typed anchor and label as a suffix instead of the query', () => {
    const anchored = detectLinkQuery('[[alpha#Head', 12);
    expect(anchored?.query).toBe('alpha');
    expect(anchored?.suffix).toBe('#Head');
    expect(anchored?.inAnchor).toBe(true);

    const labelled = detectLinkQuery('[[alpha|Alias', 13);
    expect(labelled?.query).toBe('alpha');
    expect(labelled?.suffix).toBe('|Alias');
    expect(labelled?.inLabel).toBe(true);
  });

  it('returns null before any trigger or at the very start', () => {
    expect(detectLinkQuery('plain text', 10)).toBeNull();
    expect(detectLinkQuery('[[', 1)).toBeNull();
  });
});

describe('rankCandidates', () => {
  it('offers every note with no query, in path-length then path order', () => {
    const ranked = rankCandidates(NOTES, '');
    expect(ranked).toHaveLength(4);
    expect(ranked.every(entry => entry.match === 'all')).toBe(true);
    // Shorter paths first; equal lengths (12) fall back to alphabetical.
    expect(ranked.map(entry => entry.relativePath)).toEqual([
      'notes/dup.md',
      'other/dup.md',
      'notes/beta.md',
      'notes/alpha.md',
    ]);
  });

  it('ranks an exact title above a prefix above a path substring', () => {
    const ranked = rankCandidates(NOTES, 'alpha');
    expect(ranked[0]?.match).toBe('titleExact');
    expect(ranked[0]?.relativePath).toBe('notes/alpha.md');
    const prefix = rankCandidates(NOTES, 'al');
    expect(prefix[0]?.match).toBe('titlePrefix');
    const byPath = rankCandidates(NOTES, 'other');
    expect(byPath[0]?.match).toBe('pathContains');
    expect(byPath[0]?.relativePath).toBe('other/dup.md');
  });

  it('flags every same-titled note as ambiguous so the UI must show paths', () => {
    const ranked = rankCandidates(NOTES, 'dup');
    expect(ranked).toHaveLength(2);
    expect(ranked.every(entry => entry.ambiguous)).toBe(true);
    expect(ranked.map(entry => entry.relativePath).sort()).toEqual([
      'notes/dup.md',
      'other/dup.md',
    ]);
  });

  it('does not mark a unique title as ambiguous', () => {
    expect(rankCandidates(NOTES, 'beta')[0]?.ambiguous).toBe(false);
  });

  it('returns nothing when nothing matches', () => {
    expect(rankCandidates(NOTES, 'zzz')).toEqual([]);
  });
});

describe('insertLinkText', () => {
  const query: LinkQuery = {
    from: 4,
    to: 9,
    embed: false,
    query: 'alpha',
    suffix: '',
    inAnchor: false,
    inLabel: false,
    anchorText: null,
    labelText: null,
  };

  it('inserts the readable path so the link is unambiguous', () => {
    expect(insertLinkText(candidate('n1', 'notes/alpha.md', 'Alpha'), query)).toBe(
      '[[notes/alpha]]',
    );
  });

  it('keeps an embed an embed and preserves a typed anchor or label', () => {
    const embed = { ...query, embed: true };
    expect(insertLinkText(candidate('n1', 'notes/alpha.md', 'Alpha'), embed)).toBe(
      '![[notes/alpha]]',
    );
    const anchored = { ...query, suffix: '#Head', inAnchor: true };
    expect(insertLinkText(candidate('n1', 'notes/alpha.md', 'Alpha'), anchored)).toBe(
      '[[notes/alpha#Head]]',
    );
    const labelled = { ...query, suffix: '|Alias', inLabel: true };
    expect(insertLinkText(candidate('n1', 'notes/alpha.md', 'Alpha'), labelled)).toBe(
      '[[notes/alpha|Alias]]',
    );
  });

  it('normalizes separators and drops the extension', () => {
    expect(linkTargetFor('notes\\deep\\note.MD')).toBe('notes/deep/note');
    expect(linkTargetFor('plain')).toBe('plain');
  });
});

describe('applyLinkInsertion', () => {
  it('replaces only the typed trigger and leaves the rest byte-identical', () => {
    const source = 'before [[al after';
    const query = detectLinkQuery(source, 11);
    expect(query).not.toBeNull();
    const applied = applyLinkInsertion(source, query as LinkQuery, NOTES[0] as LinkCandidate);
    expect(applied.source).toBe('before [[notes/alpha]] after');
    expect(applied.caret).toBe('before [[notes/alpha]]'.length);
  });

  it('replaces an embed trigger including its bang', () => {
    const source = 'x ![[be';
    const query = detectLinkQuery(source, source.length) as LinkQuery;
    const applied = applyLinkInsertion(source, query, NOTES[1] as LinkCandidate);
    expect(applied.source).toBe('x ![[notes/beta]]');
  });

  it('keeps a suffix the user already typed', () => {
    const source = '[[alpha#Head';
    const query = detectLinkQuery(source, source.length) as LinkQuery;
    const applied = applyLinkInsertion(source, query, NOTES[0] as LinkCandidate);
    expect(applied.source).toBe('[[notes/alpha#Head]]');
  });
});

const ANCHORS: AnchorCandidate[] = [
  { id: 'intro', text: 'Intro', level: 1 },
  { id: 'setup', text: 'Setup Steps', level: 2 },
  { id: 'block-a', text: null, level: null },
];

describe('anchor fields on the query', () => {
  it('exposes the typed anchor and label separately', () => {
    const both = detectLinkQuery('[[alpha#Head|Alias', 18) as LinkQuery;
    expect(both.anchorText).toBe('Head');
    expect(both.labelText).toBe('Alias');
    expect(both.suffix).toBe('#Head|Alias');
  });

  it('reports null for the parts that are absent', () => {
    const plain = detectLinkQuery('[[alpha', 7) as LinkQuery;
    expect(plain.anchorText).toBeNull();
    expect(plain.labelText).toBeNull();
    const anchorOnly = detectLinkQuery('[[alpha#Head', 12) as LinkQuery;
    expect(anchorOnly.anchorText).toBe('Head');
    expect(anchorOnly.labelText).toBeNull();
    const labelOnly = detectLinkQuery('[[alpha|Alias', 13) as LinkQuery;
    expect(labelOnly.anchorText).toBeNull();
    expect(labelOnly.labelText).toBe('Alias');
  });

  it('switches to anchor mode as soon as a hash is typed', () => {
    expect(isAnchorMode(detectLinkQuery('[[alpha', 7) as LinkQuery)).toBe(false);
    expect(isAnchorMode(detectLinkQuery('[[alpha#', 8) as LinkQuery)).toBe(true);
    expect(isAnchorMode(detectLinkQuery('[[#Head', 7) as LinkQuery)).toBe(true);
  });
});

describe('rankAnchors', () => {
  it('offers every anchor in document order when nothing is typed', () => {
    expect(rankAnchors(ANCHORS, '').map(anchor => anchor.id)).toEqual([
      'intro',
      'setup',
      'block-a',
    ]);
  });

  it('matches a slug prefix, a slug substring, or the heading text', () => {
    expect(rankAnchors(ANCHORS, 'int').map(anchor => anchor.id)).toEqual(['intro']);
    expect(rankAnchors(ANCHORS, 'up').map(anchor => anchor.id)).toEqual(['setup']);
    // Typed heading text, not the slug.
    expect(rankAnchors(ANCHORS, 'Setup St').map(anchor => anchor.id)).toEqual(['setup']);
    expect(rankAnchors(ANCHORS, 'block').map(anchor => anchor.id)).toEqual(['block-a']);
  });

  it('returns nothing for text that matches no anchor', () => {
    expect(rankAnchors(ANCHORS, 'zzz')).toEqual([]);
  });
});

describe('insertAnchorText and applyAnchorInsertion', () => {
  const query = detectLinkQuery('[[alpha#He', 10) as LinkQuery;
  const target = candidate('n1', 'notes/alpha.md', 'Alpha');
  const anchor: AnchorCandidate = { id: 'head', text: 'Head', level: 2 };

  it('replaces the typed anchor while keeping the chosen target', () => {
    expect(insertAnchorText(target, anchor, query)).toBe('[[notes/alpha#head]]');
    const applied = applyAnchorInsertion('[[alpha#He', query, target, anchor);
    expect(applied.source).toBe('[[notes/alpha#head]]');
    expect(applied.caret).toBe('[[notes/alpha#head]]'.length);
  });

  it('keeps a label the user already typed', () => {
    const labelled = detectLinkQuery('[[alpha#He|Alias', 16) as LinkQuery;
    expect(insertAnchorText(target, anchor, labelled)).toBe('[[notes/alpha#head|Alias]]');
  });

  it('keeps an embed an embed', () => {
    const embed = detectLinkQuery('![[alpha#He', 11) as LinkQuery;
    expect(insertAnchorText(target, anchor, embed)).toBe('![[notes/alpha#head]]');
  });
});

describe('anchorTargetFor', () => {
  it('returns the single note the typed target identifies', () => {
    const query = detectLinkQuery('[[alpha#', 8) as LinkQuery;
    expect(anchorTargetFor(NOTES, query)?.noteId).toBe('n1');
  });

  it('refuses to pick a note when two share the typed title', () => {
    // Listing one note's headings would silently bind the link to it (F10-A02).
    const query = detectLinkQuery('[[dup#', 6) as LinkQuery;
    expect(anchorTargetFor(NOTES, query)).toBeNull();
  });

  it('returns null before a hash is typed or when nothing matches', () => {
    expect(anchorTargetFor(NOTES, detectLinkQuery('[[alpha', 7) as LinkQuery)).toBeNull();
    expect(anchorTargetFor(NOTES, detectLinkQuery('[[zzz#', 6) as LinkQuery)).toBeNull();
    expect(anchorTargetFor(NOTES, null)).toBeNull();
  });
});
describe('resolveLinkTarget', () => {
  const notes = [
    { noteId: '1', relativePath: 'Research/Reading list.md', title: 'Reading list' },
    { noteId: '2', relativePath: 'Ideas.md', title: 'Ideas' },
    { noteId: '3', relativePath: 'Archive/Ideas.md', title: 'Ideas' },
  ];
  it('prefers an exact path, then a unique title or file name, ignoring case', () => {
    expect(resolveLinkTarget(notes, 'Research/Reading list')?.noteId).toBe('1');
    expect(resolveLinkTarget(notes, 'reading LIST')?.noteId).toBe('1');
    expect(resolveLinkTarget(notes, 'Archive/Ideas.md')?.noteId).toBe('3');
  });
  it('refuses to guess between same-named notes or an unknown name', () => {
    expect(resolveLinkTarget(notes, 'Ideas')?.noteId).toBe('2');
    expect(resolveLinkTarget(notes.slice(0, 1).concat(notes[2]!, { noteId: '4', relativePath: 'Old/Ideas.md', title: 'Ideas' }), 'Ideas')).toBeNull();
    expect(resolveLinkTarget(notes, 'Missing')).toBeNull();
    expect(resolveLinkTarget(notes, '  ')).toBeNull();
  });
});
