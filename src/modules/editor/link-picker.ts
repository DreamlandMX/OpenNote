//! S17 link picker: trigger detection and candidate ranking.
//!
//! All of this is pure so the rules are testable without CodeMirror or a DOM.
//! The editor only has to ask "should I open the picker here?", "what order
//! should the candidates be in?" and "what replaces the typed query?".

/** Where the `[[` query starts and what the user has typed so far. */
export interface LinkQuery {
  /** Document offset of the first `[` of the trigger (after `!` for an embed). */
  from: number;
  /** Document offset just past the caret (end of the typed query). */
  to: number;
  /** True for `![[…`, so a chosen target stays an embed. */
  embed: boolean;
  /** Text typed after `[[` that names the target, trimmed. */
  query: string;
  /** `#anchor` and/or `|label` exactly as typed, preserved on insertion. */
  suffix: string;
  /** True when the typed text already contains `#`. */
  inAnchor: boolean;
  /** True when the typed text already contains `|`. */
  inLabel: boolean;
  /** Anchor text typed after `#`, without the hash; null when absent. */
  anchorText: string | null;
  /** Label text typed after `|`, without the pipe; null when absent. */
  labelText: string | null;
}

/** Characters that end a link query instead of extending it. */
const TERMINATORS = ['\n', ']', '[', '`'];

/**
 * Detects an open `[[` query ending at `caret`.
 *
 * Returns null unless the caret sits inside an unterminated `[[…` on one line,
 * which is exactly the state S17 opens for. A terminated `[[x]]` and a `[[`
 * inside a code span or fenced block must not open the picker: the first is a
 * finished link, the second is an example (F10-R01).
 */
export function detectLinkQuery(
  source: string,
  caret: number,
  insideCode: (offset: number) => boolean = () => false,
): LinkQuery | null {
  if (caret < 2 || caret > source.length) {
    return null;
  }
  const open = source.lastIndexOf('[[', caret - 1);
  if (open < 0) {
    return null;
  }
  const typed = source.slice(open + 2, caret);
  if (typed.includes(']]') || TERMINATORS.some(terminator => typed.includes(terminator))) {
    return null;
  }
  if (insideCode(open)) {
    return null;
  }
  const embed = open > 0 && source[open - 1] === '!' && source[open - 2] !== '\\';
  const pipe = typed.indexOf('|');
  const hash = typed.indexOf('#');
  const cut = Math.min(
    pipe >= 0 ? pipe : Number.POSITIVE_INFINITY,
    hash >= 0 ? hash : Number.POSITIVE_INFINITY,
  );
  const hasCut = Number.isFinite(cut);
  const suffix = hasCut ? typed.slice(cut) : '';
  // `#anchor` runs to the pipe (or the end); `|label` always runs to the end.
  const anchorText =
    hash >= 0
      ? typed.slice(hash + 1, pipe >= 0 && pipe > hash ? pipe : typed.length)
      : null;
  const labelText = pipe >= 0 ? typed.slice(pipe + 1) : null;
  return {
    from: embed ? open - 1 : open,
    to: caret,
    embed,
    query: (hasCut ? typed.slice(0, cut) : typed).trim(),
    suffix,
    inAnchor: hash >= 0,
    inLabel: pipe >= 0,
    anchorText,
    labelText,
  };
}

/** One selectable link target. */
export interface LinkCandidate {
  noteId: string;
  relativePath: string;
  title: string;
}

export interface RankedCandidate extends LinkCandidate {
  /** True when several notes share this title, so the path must be shown. */
  ambiguous: boolean;
  /** Why this candidate ranked where it did. */
  match: 'titleExact' | 'titlePrefix' | 'pathContains' | 'all';
}

function normalize(value: string): string {
  return value.trim().toLowerCase();
}

/**
 * Ranks candidates for a query.
 *
 * Ordering is deliberate: an exact title beats a title prefix, which beats a
 * path substring; with no query every note is offered. Ambiguous titles keep
 * every candidate and are flagged so the UI shows paths — the spec forbids
 * silently linking one of several same-named notes (F10-A02).
 */
export function rankCandidates(
  candidates: readonly LinkCandidate[],
  query: string,
): RankedCandidate[] {
  const wanted = normalize(query);
  const titleCounts = new Map<string, number>();
  for (const candidate of candidates) {
    const key = normalize(candidate.title);
    titleCounts.set(key, (titleCounts.get(key) ?? 0) + 1);
  }
  const ranked: RankedCandidate[] = [];
  for (const candidate of candidates) {
    const title = normalize(candidate.title);
    const path = normalize(candidate.relativePath);
    let match: RankedCandidate['match'];
    if (wanted.length === 0) {
      match = 'all';
    } else if (title === wanted) {
      match = 'titleExact';
    } else if (title.startsWith(wanted)) {
      match = 'titlePrefix';
    } else if (path.includes(wanted)) {
      match = 'pathContains';
    } else {
      continue;
    }
    ranked.push({
      ...candidate,
      ambiguous: (titleCounts.get(title) ?? 0) > 1,
      match,
    });
  }
  const weight: Record<RankedCandidate['match'], number> = {
    titleExact: 0,
    titlePrefix: 1,
    pathContains: 2,
    all: 3,
  };
  return ranked.sort((left, right) => {
    const byMatch = weight[left.match] - weight[right.match];
    if (byMatch !== 0) return byMatch;
    // A shorter path is the more likely target, then a stable path order.
    const byLength = left.relativePath.length - right.relativePath.length;
    if (byLength !== 0) return byLength;
    return left.relativePath.localeCompare(right.relativePath);
  });
}

/** The readable link target: the path without `.md`, with forward slashes. */
export function linkTargetFor(relativePath: string): string {
  return relativePath.replace(/\\/g, '/').replace(/\.md$/i, '');
}

/**
 * The note a `[[target]]` opens: an exact path (without `.md`) first, then a
 * unique title or file name. Several matches return null rather than guess,
 * the same rule the picker uses for ambiguous titles.
 */
export function resolveLinkTarget(candidates: readonly LinkCandidate[], target: string): LinkCandidate | null {
  const wanted = normalize(target.replace(/\\/g, '/').replace(/\.md$/i, ''));
  if (wanted.length === 0) return null;
  const byPath = candidates.find(candidate => normalize(linkTargetFor(candidate.relativePath)) === wanted);
  if (byPath) return byPath;
  const byName = candidates.filter(candidate =>
    normalize(candidate.title) === wanted
    || normalize(linkTargetFor(candidate.relativePath).split('/').pop() ?? '') === wanted);
  return byName.length === 1 ? byName[0]! : null;
}

/**
 * The markdown to insert for a chosen candidate, replacing the typed query.
 *
 * The target is the readable path, which M04 resolves by path before falling
 * back to title, so the link keeps pointing at this exact note even when
 * another note shares its title. Whatever `#anchor`/`|label` the user already
 * typed is preserved verbatim, and an embed stays an embed.
 */
export function insertLinkText(candidate: LinkCandidate, query: LinkQuery): string {
  const open = query.embed ? '![[' : '[[';
  return `${open}${linkTargetFor(candidate.relativePath)}${query.suffix}]]`;
}

/**
 * Applies a selection to the document, returning the new text and caret.
 *
 * Replacing only `[from, to)` keeps every other byte untouched, so an
 * in-progress note is never rewritten beyond the trigger.
 */
export function applyLinkInsertion(
  source: string,
  query: LinkQuery,
  candidate: LinkCandidate,
): { source: string; caret: number } {
  const text = insertLinkText(candidate, query);
  const next = source.slice(0, query.from) + text + source.slice(query.to);
  return { source: next, caret: query.from + text.length };
}

/** One heading or block anchor the user can point a link at (F11). */
export interface AnchorCandidate {
  /** The anchor text inserted after `#`: a heading slug or a block id. */
  id: string;
  /** Human-readable heading text; null for block anchors. */
  text: string | null;
  /** Heading level, so the list can indent; null for block anchors. */
  level: number | null;
}

/**
 * Ranks anchors for the text typed after `#`.
 *
 * A heading slug and its display text are both matched, because a user may type
 * either; block ids match on the id. Ordering follows document order so the
 * list mirrors the note rather than jumping around.
 */
export function rankAnchors(
  anchors: readonly AnchorCandidate[],
  typed: string,
): AnchorCandidate[] {
  const wanted = typed.trim().toLowerCase();
  if (wanted.length === 0) {
    return [...anchors];
  }
  return anchors.filter(anchor => {
    const id = anchor.id.toLowerCase();
    const text = (anchor.text ?? '').toLowerCase();
    return id.startsWith(wanted) || id.includes(wanted) || text.includes(wanted);
  });
}

/**
 * The markdown to insert when an anchor is chosen, keeping the note target and
 * any label the user already typed.
 */
export function insertAnchorText(
  candidate: LinkCandidate,
  anchor: AnchorCandidate,
  query: LinkQuery,
): string {
  const open = query.embed ? '![[' : '[[';
  const label = query.labelText === null ? '' : `|${query.labelText}`;
  return `${open}${linkTargetFor(candidate.relativePath)}#${anchor.id}${label}]]`;
}

/**
 * Applies an anchor choice. When the user already picked a target (`[[note#…`),
 * only the anchor part is replaced, so the note target is preserved instead of
 * being overwritten by the anchor's own note.
 */
export function applyAnchorInsertion(
  source: string,
  query: LinkQuery,
  target: LinkCandidate,
  anchor: AnchorCandidate,
): { source: string; caret: number } {
  const text = insertAnchorText(target, anchor, query);
  const next = source.slice(0, query.from) + text + source.slice(query.to);
  return { source: next, caret: query.from + text.length };
}

/** True when the picker should be listing anchors rather than notes. */
export function isAnchorMode(query: LinkQuery): boolean {
  // Only after a target has been named: `[[#` with no target is a same-note
  // anchor, which the index resolves to the containing note.
  return query.inAnchor;
}

/**
 * The note whose anchors a `#` query should list.
 *
 * Returns null when the typed target does not identify exactly one note: with
 * two same-named notes there is no honest answer, and offering one note's
 * headings would silently bind the link to the wrong note (F10-A02).
 */
export function anchorTargetFor(
  candidates: readonly LinkCandidate[],
  query: LinkQuery | null,
): LinkCandidate | null {
  if (query === null || !query.inAnchor) {
    return null;
  }
  const ranked = rankCandidates(candidates, query.query);
  const best = ranked[0];
  if (best === undefined) {
    return null;
  }
  // Several equally good matches means the target is ambiguous.
  const tied = ranked.filter(row => row.match === best.match);
  if (tied.length > 1) {
    return null;
  }
  return best;
}
