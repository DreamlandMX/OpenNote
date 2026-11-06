//! F11 inline embeds: scanning `![[…]]` and turning expansions into blocks.
//!
//! Pure on purpose. The read overlay is HTML, so the whole feature reduces to
//! "find the embed spans" plus "render one block per span"; keeping both here
//! means every rule (code exclusion, cycle placeholder, depth limit, anchor
//! missing) is testable without a DOM.

import type { MessageKey } from '../presentation-i18n';
import type { EmbedExpansion } from './ports';

/** One `![[target#anchor]]` occurrence in a note body. */
export interface EmbedSpan {
  /** Byte offset of the `!`. */
  from: number;
  /** Byte offset just past `]]`. */
  to: number;
  /** Raw target text before `#`. */
  target: string;
  /** Anchor after `#`, without the hash. */
  anchor: string | null;
  /** The exact source text, used as a stable key and for the fallback. */
  raw: string;
}

/** Fenced-code tracking plus inline-code masking, matching the index rules. */
function codeRanges(source: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];
  let fence: string | null = null;
  let offset = 0;
  for (const line of source.split('\n')) {
    const lineStart = offset;
    const lineEnd = offset + line.length;
    offset = lineEnd + 1;
    const trimmed = line.trimStart();
    const marker = /^(`{3,}|~{3,})/.exec(trimmed);
    if (marker !== null) {
      const run = marker[1] as string;
      if (fence === null) {
        fence = run[0] as string;
      } else if (run[0] === fence) {
        fence = null;
      }
      ranges.push([lineStart, lineEnd]);
      continue;
    }
    if (fence !== null) {
      ranges.push([lineStart, lineEnd]);
      continue;
    }
    // Inline code: a run of backticks closed by the same length.
    let index = 0;
    while (index < line.length) {
      if (line[index] !== '`') {
        index += 1;
        continue;
      }
      let run = 0;
      while (line[index + run] === '`') {
        run += 1;
      }
      const close = line.indexOf('`'.repeat(run), index + run);
      if (close < 0) {
        index += run;
        continue;
      }
      ranges.push([lineStart + index, lineStart + close + run]);
      index = close + run;
    }
  }
  return ranges;
}

function inRanges(ranges: Array<[number, number]>, offset: number): boolean {
  return ranges.some(([start, end]) => offset >= start && offset < end);
}

/**
 * Every embed in `source`, in document order.
 *
 * An embed inside a fenced block or an inline code span is an example, not a
 * reference, and is skipped — the same rule the index applies (F10-R01).
 */
export function scanEmbeds(source: string): EmbedSpan[] {
  const ranges = codeRanges(source);
  const spans: EmbedSpan[] = [];
  let index = 0;
  while (index < source.length) {
    const open = source.indexOf('![[', index);
    if (open < 0) {
      break;
    }
    if (inRanges(ranges, open)) {
      index = open + 3;
      continue;
    }
    // An escaped `\![[` is literal text.
    if (open > 0 && source[open - 1] === '\\') {
      index = open + 3;
      continue;
    }
    const close = source.indexOf(']]', open + 3);
    if (close < 0) {
      break;
    }
    const inner = source.slice(open + 3, close);
    // A newline or a nested bracket means this is not one embed.
    if (inner.includes('\n') || inner.includes('[')) {
      index = open + 3;
      continue;
    }
    const pipe = inner.indexOf('|');
    const withoutLabel = pipe >= 0 ? inner.slice(0, pipe) : inner;
    const hash = withoutLabel.indexOf('#');
    const target = (hash >= 0 ? withoutLabel.slice(0, hash) : withoutLabel).trim();
    const anchorText = hash >= 0 ? withoutLabel.slice(hash + 1).trim() : '';
    if (target.length === 0 && anchorText.length === 0) {
      index = close + 2;
      continue;
    }
    spans.push({
      from: open,
      to: close + 2,
      target,
      anchor: anchorText.length === 0 ? null : anchorText,
      raw: source.slice(open, close + 2),
    });
    index = close + 2;
  }
  return spans;
}

/** What to render for one embed. */
export type EmbedBlockKind = 'content' | 'placeholder';

export interface EmbedBlock {
  /** Stable key: the source span, so re-renders do not reorder rows. */
  key: string;
  kind: EmbedBlockKind;
  /** Rendered body when `kind` is `content`; the message when it is not. */
  text: string;
  /** Message key for a placeholder, so the caller localizes it. */
  messageKey: MessageKey | null;
  /** Note path to offer as a jump target, when one is known. */
  targetPath: string | null;
  /** The original syntax, so a placeholder can still show what was written. */
  raw: string;
}

/**
 * Turns one expansion into a block.
 *
 * Every failure keeps a readable placeholder that names the target: a cycle or
 * a deleted anchor must never render as an empty hole (F11-R02, F11-A03).
 */
export function embedBlock(span: EmbedSpan, expansion: EmbedExpansion | null): EmbedBlock {
  const key = `${span.from}:${span.to}`;
  if (expansion === null) {
    return {
      key,
      kind: 'placeholder',
      text: '',
      messageKey: 'embed.loading',
      targetPath: null,
      raw: span.raw,
    };
  }
  const targetPath = expansion.targetPath;
  switch (expansion.status) {
    case 'ok':
      return {
        key,
        kind: 'content',
        text: expansion.content ?? '',
        messageKey: null,
        targetPath,
        raw: span.raw,
      };
    case 'cycle':
      return {
        key,
        kind: 'placeholder',
        text: '',
        messageKey: 'embed.cycle',
        targetPath,
        raw: span.raw,
      };
    case 'depthExceeded':
      return {
        key,
        kind: 'placeholder',
        text: '',
        messageKey: 'embed.depth',
        targetPath,
        raw: span.raw,
      };
    case 'anchorMissing':
      return {
        key,
        kind: 'placeholder',
        text: '',
        messageKey: 'embed.anchorMissing',
        targetPath,
        raw: span.raw,
      };
    case 'ambiguousTarget':
      return {
        key,
        kind: 'placeholder',
        text: '',
        messageKey: 'embed.ambiguous',
        targetPath,
        raw: span.raw,
      };
    case 'brokenLink':
    default:
      return {
        key,
        kind: 'placeholder',
        text: '',
        messageKey: 'embed.broken',
        targetPath,
        raw: span.raw,
      };
  }
}

/**
 * The placeholder for an embed whose expansion request failed.
 *
 * Distinct from `embed.loading`: the request is over and there is nothing to
 * wait for, so showing a spinner text forever would misreport the state. The
 * written syntax is kept so the reference can still be repaired by hand.
 */
export function embedUnavailable(span: EmbedSpan): EmbedBlock {
  return {
    key: `${span.from}:${span.to}`,
    kind: 'placeholder',
    text: '',
    messageKey: 'embed.unavailable',
    targetPath: null,
    raw: span.raw,
  };
}

/**
 * Splits `source` into the pieces a renderer needs: literal markdown and the
 * embed slots in between. The caller renders the literal parts with the normal
 * markdown path and each slot with [`embedBlock`].
 */
export type EmbedSegment =
  | { kind: 'text'; text: string }
  | { kind: 'embed'; span: EmbedSpan };

export function splitEmbedSegments(source: string): EmbedSegment[] {
  const spans = scanEmbeds(source);
  const segments: EmbedSegment[] = [];
  let cursor = 0;
  for (const span of spans) {
    if (span.from > cursor) {
      segments.push({ kind: 'text', text: source.slice(cursor, span.from) });
    }
    segments.push({ kind: 'embed', span });
    cursor = span.to;
  }
  if (cursor < source.length) {
    segments.push({ kind: 'text', text: source.slice(cursor) });
  }
  return segments;
}

/**
 * The deduplicated expansion requests for one note body.
 *
 * Two embeds of the same target and anchor share one request, which keeps a
 * note that embeds the same section repeatedly from fetching it repeatedly.
 */
export function embedRequests(source: string): Array<{ target: string; anchor: string | null }> {
  const seen = new Set<string>();
  const requests: Array<{ target: string; anchor: string | null }> = [];
  for (const span of scanEmbeds(source)) {
    const key = `${span.target}#${span.anchor ?? ''}`;
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    requests.push({ target: span.target, anchor: span.anchor });
  }
  return requests;
}
