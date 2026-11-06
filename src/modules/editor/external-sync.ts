import { Transaction, type TransactionSpec } from '@codemirror/state';

/**
 * The smallest change that turns one document string into another.
 *
 * A second view of the same note receives text from the committed buffer while
 * its own editor keeps a separate selection and undo stack. Replacing the whole
 * document for every keystroke would map both selections to the end of the note
 * (and make one edit look like a full-document rewrite), so external sync keeps
 * the unchanged prefix and suffix and replaces only the middle.
 */
export interface ExternalChange {
  from: number;
  to: number;
  insert: string;
}

export const EXTERNAL_REPLACE_EVENT = 'opennote.replace-external';

/**
 * A mirror applying the active pane's edit is not its own edit. Excluding the
 * replacement from history keeps a later undo from reverting text the other
 * pane already committed (F07-R01).
 */
export function externalReplacement(change: ExternalChange): TransactionSpec {
  return {
    changes: change,
    annotations: [
      Transaction.userEvent.of(EXTERNAL_REPLACE_EVENT),
      Transaction.addToHistory.of(false),
    ],
  };
}

/** Returns `null` when the two strings already match. */
export function externalChange(current: string, next: string): ExternalChange | null {
  if (current === next) {
    return null;
  }

  const shared = Math.min(current.length, next.length);
  let from = 0;
  while (from < shared && current.charCodeAt(from) === next.charCodeAt(from)) {
    from += 1;
  }

  let currentEnd = current.length;
  let nextEnd = next.length;
  while (
    currentEnd > from &&
    nextEnd > from &&
    current.charCodeAt(currentEnd - 1) === next.charCodeAt(nextEnd - 1)
  ) {
    currentEnd -= 1;
    nextEnd -= 1;
  }

  return { from, to: currentEnd, insert: next.slice(from, nextEnd) };
}
