import { useEffect, useMemo, useRef, useState } from 'react';
import type { Translator } from '../presentation-i18n';
import {
  isAnchorMode,
  rankAnchors,
  rankCandidates,
  type AnchorCandidate,
  type LinkCandidate,
  type LinkQuery,
} from './link-picker';

interface Props {
  /** The query the editor detected; null closes the popover. */
  query: LinkQuery | null;
  candidates: readonly LinkCandidate[];
  /**
   * Anchors of the note being targeted, offered once the user types `#`.
   * Empty means the note has no headings or block ids yet.
   */
  anchors?: readonly AnchorCandidate[];
  /** The note the typed target resolves to; needed to insert an anchor link. */
  target?: LinkCandidate | null;
  translator: Translator;
  /** Called with the chosen target; the editor performs the insertion. */
  onChoose(candidate: LinkCandidate): void;
  /** Called with the chosen anchor and its note. */
  onChooseAnchor(target: LinkCandidate, anchor: AnchorCandidate): void;
  onClose(): void;
}

/**
 * S17 link/embed picker.
 *
 * Keyboard contract from the screen spec: Tab and the arrow keys move the
 * highlight, Enter chooses, Escape cancels, and focus stays visible. The
 * popover never inserts on its own — it reports the choice and the editor owns
 * the document edit.
 */
export function LinkPicker({
  query,
  candidates,
  anchors = [],
  target = null,
  translator,
  onChoose,
  onChooseAnchor,
  onClose,
}: Props) {
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLUListElement | null>(null);
  // Once a `#` is typed the list switches from notes to that note's anchors.
  const anchorMode = query !== null && isAnchorMode(query);
  const noteRows = useMemo(
    () => (query === null || anchorMode ? [] : rankCandidates(candidates, query.query)),
    [anchorMode, candidates, query],
  );
  const anchorRows = useMemo(
    () => (query === null || !anchorMode ? [] : rankAnchors(anchors, query.anchorText ?? '')),
    [anchorMode, anchors, query],
  );
  const rowCount = anchorMode ? anchorRows.length : noteRows.length;

  // A new query resets the highlight so Enter always takes the best match.
  useEffect(() => {
    setActive(0);
  }, [query?.from, query?.query, query?.anchorText]);

  useEffect(() => {
    if (query === null) {
      return;
    }
    const choose = (index: number): void => {
      if (anchorMode) {
        const anchor = anchorRows[index];
        if (anchor !== undefined && target !== null) {
          onChooseAnchor(target, anchor);
        }
        return;
      }
      const note = noteRows[index];
      if (note !== undefined) {
        onChoose(note);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
        return;
      }
      if (rowCount === 0) {
        return;
      }
      if (event.key === 'ArrowDown' || (event.key === 'Tab' && !event.shiftKey)) {
        event.preventDefault();
        setActive(current => (current + 1) % rowCount);
      } else if (event.key === 'ArrowUp' || (event.key === 'Tab' && event.shiftKey)) {
        event.preventDefault();
        setActive(current => (current - 1 + rowCount) % rowCount);
      } else if (event.key === 'Enter') {
        event.preventDefault();
        choose(active);
      }
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [
    active,
    anchorMode,
    anchorRows,
    noteRows,
    onChoose,
    onChooseAnchor,
    onClose,
    query,
    rowCount,
    target,
  ]);

  // Keep the highlighted row in view without stealing focus from the editor.
  useEffect(() => {
    const list = listRef.current;
    if (list === null) {
      return;
    }
    const item = list.children[active];
    if (item instanceof HTMLElement) {
      item.scrollIntoView({ block: 'nearest' });
    }
  }, [active]);

  if (query === null) {
    return null;
  }

  const empty = rowCount === 0;

  return (
    <div
      className="link-picker"
      role="listbox"
      aria-label={translator(anchorMode ? 'linkPicker.anchorTitle' : 'linkPicker.title')}
    >
      {empty ? (
        <p className="link-picker-empty">
          {translator(anchorMode ? 'linkPicker.noAnchor' : 'linkPicker.empty')}
        </p>
      ) : anchorMode ? (
        <ul ref={listRef}>
          {anchorRows.map((row, index) => (
            <li
              key={`anchor:${row.id}`}
              role="option"
              aria-selected={index === active}
              className={index === active ? 'is-active' : undefined}
              onMouseEnter={() => setActive(index)}
              onMouseDown={event => {
                event.preventDefault();
                if (target !== null) {
                  onChooseAnchor(target, row);
                }
              }}
            >
              <span className="link-picker-title">
                {row.text === null ? row.id : `${'#'.repeat(row.level ?? 1)} ${row.text}`}
              </span>
              {/* The slug is what gets inserted, so show it when it differs. */}
              {row.text !== null && row.text !== row.id ? (
                <span className="link-picker-path">{row.id}</span>
              ) : null}
            </li>
          ))}
        </ul>
      ) : (
        <ul ref={listRef}>
          {noteRows.map((row, index) => (
            <li
              key={row.noteId}
              role="option"
              aria-selected={index === active}
              className={index === active ? 'is-active' : undefined}
              onMouseEnter={() => setActive(index)}
              onMouseDown={event => {
                // Keep the editor's selection: the insertion is applied after.
                event.preventDefault();
                onChoose(row);
              }}
            >
              <span className="link-picker-title">{row.title}</span>
              {/* A shared title must show its path, or the user cannot tell
                  which note they are linking (F10-A02). */}
              {row.ambiguous ? (
                <span className="link-picker-path">{row.relativePath}</span>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
