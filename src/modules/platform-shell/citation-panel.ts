import type { BibEntry, BibError, CitationIssue, CitationPlan } from '../editor';
import type { Translator } from '../presentation-i18n';

/**
 * S75 citation panel logic.
 *
 * The citation plan comes from M13. This module owns the parts a component must
 * not get wrong:
 *
 * * a duplicated key can never be cited, because there is no single entry it
 *   would refer to (DUPLICATE_CITE_KEY);
 * * an insertion is only possible when the plan carries no issues, so a
 *   `\cite{?}` can never be produced by accident;
 * * every issue is explained in a sentence, not just flagged.
 */

/** True when the plan may be inserted. */
export function canInsertCitation(plan: CitationPlan | null): boolean {
  return plan !== null && plan.issues.length === 0 && plan.fragment.trim() !== '';
}

/**
 * Whether a key may be selected in the list.
 *
 * A duplicated key is refused at the source so the user cannot build a citation
 * that would resolve to two different entries.
 */
export function isSelectable(duplicate: boolean): boolean {
  return !duplicate;
}

/**
 * The UI message for a Rust parser `messageKey`.
 *
 * The parser reports stable keys, not sentences. Mapping them here keeps the
 * panel localized while leaving the Rust error contract unchanged. An unknown
 * future key falls back to the generic Issues label rather than showing a raw
 * `error.*` identifier to the user.
 */
export function bibErrorMessageKey(messageKey: string):
  | 'citation.parseError.malformed'
  | 'citation.parseError.unterminated'
  | 'citation.parseError.noKey'
  | 'citation.issues' {
  switch (messageKey) {
    case 'error.bibEntryMalformed':
      return 'citation.parseError.malformed';
    case 'error.bibEntryUnterminated':
      return 'citation.parseError.unterminated';
    case 'error.bibEntryNoKey':
      return 'citation.parseError.noKey';
    default:
      return 'citation.issues';
  }
}

/** The message key an issue maps to. */
export function citationIssueKey(issue: CitationIssue):
  | 'citation.issue.unknownKey'
  | 'citation.issue.duplicateKey'
  | 'citation.issue.invalidKey'
  | 'citation.issue.noKeys' {
  switch (issue.kind) {
    case 'unknownKey':
      return 'citation.issue.unknownKey';
    case 'duplicateKey':
      return 'citation.issue.duplicateKey';
    case 'invalidKey':
      return 'citation.issue.invalidKey';
    case 'noKeys':
    default:
      return 'citation.issue.noKeys';
  }
}

/** The parameters an issue message needs. */
export function citationIssueParams(issue: CitationIssue): Record<string, string | number> {
  switch (issue.kind) {
    case 'unknownKey':
      return { citeKey: issue.citeKey };
    case 'duplicateKey':
      return { citeKey: issue.citeKey, count: issue.offsets.length };
    case 'invalidKey':
      return { citeKey: issue.citeKey };
    case 'noKeys':
    default:
      return {};
  }
}

/** Resolves an issue to its sentence, so the component only renders text. */
export function citationIssueText(issue: CitationIssue, translator: Translator): string {
  return translator(citationIssueKey(issue), citationIssueParams(issue));
}

/**
 * A stable selection after the list changes.
 *
 * When the bibliography is re-parsed a key may disappear; keeping it selected
 * would produce a citation to something no longer there, so unknown keys are
 * dropped instead of carried forward.
 */
export function pruneSelection(
  selected: readonly string[],
  availableKeys: readonly string[],
): string[] {
  return selected.filter((key) => availableKeys.includes(key));
}

/** The ordered key list a citation command should carry. */
export function orderedSelection(
  selected: readonly string[],
  availableKeys: readonly string[],
): string[] {
  // Document order, so the same selection always produces the same command.
  return availableKeys.filter((key) => selected.includes(key));
}

/**
 * Converts a parser byte offset into the UTF-16 index a textarea expects.
 *
 * Rust string slices report UTF-8 byte offsets, while `setSelectionRange` counts
 * UTF-16 code units. Treating them as equal shifts the selection after any
 * non-ASCII author name. An offset inside a multi-byte character snaps to that
 * character's start, and an offset past the end clamps to the source length.
 */
export function utf8OffsetToUtf16Index(source: string, byteOffset: number): number {
  if (byteOffset <= 0) {
    return 0;
  }
  let bytes = 0;
  let index = 0;
  while (index < source.length) {
    const codePoint = source.codePointAt(index);
    if (codePoint === undefined) {
      break;
    }
    const width = codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
    if (bytes + width > byteOffset) {
      return index;
    }
    bytes += width;
    index += codePoint > 0xffff ? 2 : 1;
    if (bytes === byteOffset) {
      return index;
    }
  }
  return source.length;
}

/**
 * The source range of one parser-reported block.
 *
 * The parser's `raw` block is the authoritative extent, so the whole block is
 * selected rather than only the `@`. Both endpoints are converted from UTF-8
 * bytes before the textarea sees them.
 */
function bibBlockSelection(
  source: string,
  block: Pick<BibEntry, 'offset' | 'raw'>,
): { start: number; end: number } {
  const start = utf8OffsetToUtf16Index(source, block.offset);
  // `raw` is the exact source slice, so its UTF-16 length is the right width once
  // `start` has been converted; adding the parser's byte length would over-select.
  const end = Math.min(start + block.raw.length, source.length);
  return { start, end: Math.max(start, end) };
}

/** The source range to select when a parse error is located. */
export function bibErrorSelection(
  source: string,
  error: Pick<BibError, 'offset' | 'raw'>,
): { start: number; end: number } {
  return bibBlockSelection(source, error);
}

/**
 * The source range of a parsed entry, used to tell duplicate rows apart.
 *
 * DUPLICATE_CITE_KEY requires both sources to be distinguishable, so each
 * duplicate row can select its exact block instead of showing only the key.
 */
export function bibEntrySelection(
  source: string,
  entry: Pick<BibEntry, 'offset' | 'raw'>,
): { start: number; end: number } {
  return bibBlockSelection(source, entry);
}

/**
 * Removes one parsed entry from the `.bib` text.
 *
 * The parser's `raw` block is the authoritative extent (F49-R01), so the same
 * UTF-8 to UTF-16 conversion used for error location applies. A stale parse must
 * not delete unrelated text, so a mismatch returns null instead of editing.
 * The note source is never touched (F49-A02).
 */
export function removeBibEntry(
  source: string,
  entry: Pick<BibEntry, 'offset' | 'raw'>,
): string | null {
  if (entry.raw === '') {
    return null;
  }
  const start = utf8OffsetToUtf16Index(source, entry.offset);
  const end = start + entry.raw.length;
  if (source.slice(start, end) !== entry.raw) {
    return null;
  }
  // Absorb the newline that separated the entry from the next one, so a middle
  // entry does not leave an empty line behind.
  let after = end;
  if (source.startsWith('\r\n', after)) {
    after += 2;
  } else if (source[after] === '\n') {
    after += 1;
  }
  return source.slice(0, start) + source.slice(after);
}

/** A one-line description of the bibliography state. */
export function bibliographyState(
  entries: number,
  duplicates: number,
): 'empty' | 'clean' | 'duplicated' {
  if (entries === 0) {
    return 'empty';
  }
  return duplicates > 0 ? 'duplicated' : 'clean';
}
