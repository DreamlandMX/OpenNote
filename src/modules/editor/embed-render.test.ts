import { describe, expect, it } from 'vitest';
import type { EmbedExpansion } from './ports';
import {
  embedBlock,
  embedRequests,
  embedUnavailable,
  scanEmbeds,
  splitEmbedSegments,
} from './embed-render';

function expansion(overrides: Partial<EmbedExpansion>): EmbedExpansion {
  return {
    status: 'ok',
    targetId: 'n2',
    targetPath: 'notes/b.md',
    anchor: null,
    content: 'body',
    chain: ['n1', 'n2'],
    ...overrides,
  };
}

describe('scanEmbeds', () => {
  it('finds a plain embed and reports the exact span', () => {
    const source = 'see ![[notes/b]] end';
    const spans = scanEmbeds(source);
    expect(spans).toHaveLength(1);
    const span = spans[0];
    expect(span?.raw).toBe('![[notes/b]]');
    expect(span?.target).toBe('notes/b');
    expect(span?.anchor).toBeNull();
    expect(source.slice(span?.from, span?.to)).toBe('![[notes/b]]');
  });

  it('splits an anchor and ignores a label', () => {
    const spans = scanEmbeds('![[notes/b#Two|Alias]]');
    expect(spans[0]?.target).toBe('notes/b');
    expect(spans[0]?.anchor).toBe('Two');
  });

  it('keeps a same-note embed with only an anchor', () => {
    const spans = scanEmbeds('![[#Local]]');
    expect(spans[0]?.target).toBe('');
    expect(spans[0]?.anchor).toBe('Local');
  });

  it('skips embeds inside fenced and inline code', () => {
    const fenced = '```\n![[a]]\n```\n![[b]]\n';
    expect(scanEmbeds(fenced).map(span => span.target)).toEqual(['b']);
    const inline = 'text `![[a]]` more ![[b]]';
    expect(scanEmbeds(inline).map(span => span.target)).toEqual(['b']);
  });

  it('skips an escaped embed', () => {
    expect(scanEmbeds('\\![[a]]')).toEqual([]);
  });

  it('ignores an unterminated or multi-line embed', () => {
    expect(scanEmbeds('![[unclosed')).toEqual([]);
    expect(scanEmbeds('![[a\nb]]')).toEqual([]);
  });

  it('finds several embeds in document order', () => {
    const spans = scanEmbeds('![[a]] x ![[b#c]]');
    expect(spans.map(span => span.target)).toEqual(['a', 'b']);
    expect(spans[1]?.anchor).toBe('c');
  });
});

describe('embedBlock', () => {
  const span = scanEmbeds('![[notes/b]]')[0]!;

  it('renders content for a successful expansion', () => {
    const block = embedBlock(span, expansion({}));
    expect(block.kind).toBe('content');
    expect(block.text).toBe('body');
    expect(block.messageKey).toBeNull();
    expect(block.targetPath).toBe('notes/b.md');
  });

  it('gives every failure a localized message key and keeps the target', () => {
    const cases: Array<[EmbedExpansion['status'], string]> = [
      ['cycle', 'embed.cycle'],
      ['depthExceeded', 'embed.depth'],
      ['anchorMissing', 'embed.anchorMissing'],
      ['ambiguousTarget', 'embed.ambiguous'],
      ['brokenLink', 'embed.broken'],
    ];
    for (const [status, key] of cases) {
      const block = embedBlock(span, expansion({ status, content: null }));
      expect(block.kind).toBe('placeholder');
      expect(block.messageKey).toBe(key);
      // The raw syntax survives so the placeholder can show what was written.
      expect(block.raw).toBe('![[notes/b]]');
      expect(block.targetPath).toBe('notes/b.md');
    }
  });

  it('shows a loading placeholder before an expansion arrives', () => {
    const block = embedBlock(span, null);
    expect(block.kind).toBe('placeholder');
    expect(block.messageKey).toBe('embed.loading');
  });

  it('marks a failed expansion as unavailable rather than loading forever', () => {
    const block = embedUnavailable(span);
    expect(block.kind).toBe('placeholder');
    expect(block.messageKey).toBe('embed.unavailable');
    expect(block.raw).toBe('![[notes/b]]');
    expect(block.targetPath).toBeNull();
  });

  it('keys blocks by their source span so rows stay stable', () => {
    const first = scanEmbeds('![[a]] ![[b]]');
    expect(embedBlock(first[0]!, expansion({})).key).not.toBe(
      embedBlock(first[1]!, expansion({})).key,
    );
  });
});

describe('splitEmbedSegments', () => {
  it('alternates literal text and embed slots without losing bytes', () => {
    const source = 'before ![[a]] middle ![[b]] after';
    const segments = splitEmbedSegments(source);
    expect(segments.map(segment => segment.kind)).toEqual([
      'text',
      'embed',
      'text',
      'embed',
      'text',
    ]);
    const rebuilt = segments
      .map(segment => (segment.kind === 'text' ? segment.text : segment.span.raw))
      .join('');
    expect(rebuilt).toBe(source);
  });

  it('returns one text segment when there is nothing to embed', () => {
    expect(splitEmbedSegments('plain')).toEqual([{ kind: 'text', text: 'plain' }]);
  });
});

describe('embedRequests', () => {
  it('deduplicates the same target and anchor', () => {
    const requests = embedRequests('![[a]] ![[a]] ![[a#x]] ![[b#x]]');
    expect(requests).toEqual([
      { target: 'a', anchor: null },
      { target: 'a', anchor: 'x' },
      { target: 'b', anchor: 'x' },
    ]);
  });

  it('returns nothing when a note has no embeds', () => {
    expect(embedRequests('just text')).toEqual([]);
  });
});
