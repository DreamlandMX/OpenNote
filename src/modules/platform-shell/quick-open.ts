/**
 * Quick open (F13 / S20) matching. Titles only, because S20 shows "按标题选择";
 * same-title notes are still distinguishable because every row prints its full
 * path (F13-R02). Matching is a case-insensitive substring on the title and the
 * path, never a rewrite of either (F13-R01).
 */
export interface QuickOpenCandidate {
  noteId: string;
  relativePath: string;
  title: string;
}

export interface QuickOpenMatch {
  candidate: QuickOpenCandidate;
  score: number;
}

export const MAX_QUICK_OPEN_QUERY = 200;

/** File stem for display and matching; `.md` is not part of a title. */
export function quickOpenTitle(relativePath: string): string {
  const name = relativePath.split(/[/\\]/).pop() ?? relativePath;
  return name.replace(/\.md$/i, '');
}

function normalize(value: string): string {
  return value.trim().toLocaleLowerCase();
}

/**
 * Scores one candidate. Lower is better: exact title, title prefix, title
 * substring, path substring. `null` means no match, so the caller shows an
 * empty state instead of a fabricated row (F13-R04).
 */
export function scoreQuickOpen(candidate: QuickOpenCandidate, query: string): number | null {
  const needle = normalize(query);
  if (needle === '') {
    return 0;
  }
  const title = normalize(candidate.title);
  if (title === needle) {
    return 0;
  }
  if (title.startsWith(needle)) {
    return 1;
  }
  if (title.includes(needle)) {
    return 2;
  }
  if (normalize(candidate.relativePath).includes(needle)) {
    return 3;
  }
  return null;
}

/**
 * Ranks candidates. Ties break on path then id so two same-titled notes keep a
 * stable order across renders and both remain visible (F13-R02).
 */
export function rankQuickOpen(
  candidates: readonly QuickOpenCandidate[],
  query: string,
): QuickOpenCandidate[] {
  const scored: QuickOpenMatch[] = [];
  for (const candidate of candidates) {
    const score = scoreQuickOpen(candidate, query);
    if (score !== null) {
      scored.push({ candidate, score });
    }
  }
  scored.sort((left, right) => {
    if (left.score !== right.score) {
      return left.score - right.score;
    }
    const byPath = left.candidate.relativePath.localeCompare(right.candidate.relativePath);
    return byPath !== 0 ? byPath : left.candidate.noteId.localeCompare(right.candidate.noteId);
  });
  return scored.map((entry) => entry.candidate);
}

/** `学习 / 知识管理` style breadcrumb used by S20 under each title. */
export function breadcrumbOf(relativePath: string): string {
  const segments = relativePath.split(/[/\\]/).filter((segment) => segment !== '');
  segments.pop();
  return segments.length === 0 ? '' : segments.join(' / ');
}

export function truncateQuery(query: string): { value: string; truncated: boolean } {
  const characters = Array.from(query);
  if (characters.length <= MAX_QUICK_OPEN_QUERY) {
    return { value: query, truncated: false };
  }
  return { value: characters.slice(0, MAX_QUICK_OPEN_QUERY).join(''), truncated: true };
}
