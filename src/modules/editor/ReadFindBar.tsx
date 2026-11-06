import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { ChevronDown, ChevronUp, X } from 'lucide-react';
import type { Translator } from '../presentation-i18n';
import { clearMatches, findTextRanges, paintMatches, revealMatch, stepMatch } from './read-find';

interface Props {
  /** The rendered reading pane to search. */
  target: RefObject<HTMLElement | null>;
  /** Changes whenever the pane's HTML is replaced, so matches are recomputed. */
  content: string;
  translator: Translator;
  /** A repeated find press while open focuses and selects the query again. */
  focusNonce: number;
  onClose(): void;
}

/** Find in the reading pane: no replace, since reading mode cannot edit. */
export function ReadFindBar({ target, content, translator, focusNonce, onClose }: Props) {
  const input = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [matchCase, setMatchCase] = useState(false);
  const [ranges, setRanges] = useState<Range[]>([]);
  const [current, setCurrent] = useState(-1);

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, [focusNonce]);

  useLayoutEffect(() => {
    const root = target.current;
    const found = root === null ? [] : findTextRanges(root, query, matchCase);
    setRanges(found);
    setCurrent(found.length > 0 ? 0 : -1);
  }, [target, content, query, matchCase]);

  useEffect(() => {
    paintMatches(ranges, current);
    revealMatch(ranges[current]);
  }, [ranges, current]);

  useEffect(() => clearMatches, []);

  const step = (direction: 1 | -1): void => setCurrent((index) => stepMatch(index, ranges.length, direction));
  const status = query === ''
    ? ''
    : ranges.length === 0
      ? translator('find.none')
      : translator('find.count', { current: current + 1, total: ranges.length });

  return (
    <div className="read-find" role="search" aria-label={translator('find.label')}>
      <input
        ref={input}
        type="search"
        value={query}
        aria-label={translator('find.label')}
        placeholder={translator('find.find')}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === 'Enter') {
            event.preventDefault();
            step(event.shiftKey ? -1 : 1);
          } else if (event.key === 'Escape') {
            event.preventDefault();
            event.stopPropagation();
            onClose();
          }
        }}
      />
      <span className="read-find-status" role="status" aria-live="polite">{status}</span>
      <label className="read-find-case">
        <input type="checkbox" checked={matchCase} onChange={(event) => setMatchCase(event.target.checked)} />
        {translator('find.matchCase')}
      </label>
      <button type="button" className="read-find-button" aria-label={translator('find.previous')} title={translator('find.previous')}
        disabled={ranges.length === 0} onClick={() => step(-1)}>
        <ChevronUp size={16} aria-hidden="true" />
      </button>
      <button type="button" className="read-find-button" aria-label={translator('find.next')} title={translator('find.next')}
        disabled={ranges.length === 0} onClick={() => step(1)}>
        <ChevronDown size={16} aria-hidden="true" />
      </button>
      <button type="button" className="read-find-button" aria-label={translator('find.close')} title={translator('find.close')} onClick={onClose}>
        <X size={16} aria-hidden="true" />
      </button>
    </div>
  );
}
