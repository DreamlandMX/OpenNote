import { useEffect, useMemo, useRef, useState } from 'react';
import { Trash2, X } from 'lucide-react';
import type { BibBackend, BibError, Bibliography, CitationPlan, NoteService } from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  bibErrorMessageKey,
  bibEntrySelection,
  bibErrorSelection,
  canInsertCitation,
  citationIssueText,
  removeBibEntry,
} from './citation-panel';
import {
  bibliographySummary,
  citationItems,
  citableItems,
  filterCitations,
} from './insertion-form';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** The buffer the citation is inserted into. */
  source: string;
  /** Inserts the `\cite` command through the editor as one undo step. */
  apply(fragment: string): boolean;
  onClose(): void;
}

const BACKENDS: readonly BibBackend[] = ['bibtex', 'biber'];

/**
 * S75 citation list.
 *
 * The bibliography is authoritative: this panel parses it and derives a list, so
 * deleting an entry never removes a `\cite` from the source (F49-A02). A duplicate
 * key is refused with both offsets rather than silently resolved
 * (DUPLICATE_CITE_KEY), and the backend is part of the request so a tool mismatch
 * stays visible (F49-A03).
 */
export function CitationPanel({ service, translator, locale, source, apply, onClose }: Props) {
  const [bibText, setBibText] = useState('');
  const [parsedText, setParsedText] = useState<string | null>(null);
  const [bibliography, setBibliography] = useState<Bibliography | null>(null);
  const [backend, setBackend] = useState<BibBackend>('bibtex');
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string[]>([]);
  const [plan, setPlan] = useState<CitationPlan | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [removalWarning, setRemovalWarning] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const [unicodeKeys, setUnicodeKeys] = useState<readonly string[]>([]);
  const bibRef = useRef<HTMLTextAreaElement | null>(null);
  const openerRef = useRef<Element | null>(null);
  // Late responses from an older parse, preview or backend check are discarded.
  const parseRequest = useRef(0);
  const previewRequest = useRef(0);
  const unicodeRequest = useRef(0);
  const backendRef = useRef<BibBackend>('bibtex');

  // §8: the overlay takes focus so Escape works, then hands it back to its trigger.
  useEffect(() => {
    openerRef.current = document.activeElement;
    bibRef.current?.focus();
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);

  const items = useMemo(
    () => (bibliography === null ? [] : filterCitations(citationItems(bibliography), query)),
    [bibliography, query],
  );
  const summary = bibliography === null ? null : bibliographySummary(bibliography);
  const citable = bibliography === null ? 0 : citableItems(bibliography).length;
  // Editing the .bib text invalidates the parsed list until Parse is run again.
  const parsedCurrent = parsedText === bibText;

  async function refreshUnicode(target: Bibliography, engine: BibBackend): Promise<void> {
    const request = ++unicodeRequest.current;
    const unicode = await service.bibliographyUnicode(target, engine);
    if (request === unicodeRequest.current) {
      setUnicodeKeys(unicode.ok ? unicode.value : []);
    }
  }

  async function runParse(text: string): Promise<Bibliography | null> {
    const request = ++parseRequest.current;
    // A plan belongs to the bibliography it was previewed from.
    previewRequest.current += 1;
    setSelected([]);
    setPlan(null);
    const outcome = await service.parseBibliography(text);
    if (request !== parseRequest.current) {
      return null;
    }
    if (!outcome.ok) {
      setError(outcome.error);
      return null;
    }
    setBibliography(outcome.value);
    setParsedText(text);
    setError(null);
    setNotice(null);
    setRemovalWarning(null);
    // F49-R04: the backend decides whether Unicode is safe, so the list is asked
    // again whenever the bibliography or the backend changes.
    void refreshUnicode(outcome.value, backendRef.current);
    return outcome.value;
  }

  async function parse(): Promise<void> {
    if (bibText.trim() === '') {
      return;
    }
    await runParse(bibText);
  }

  /**
   * Removes one entry from the `.bib` text, never from the note source.
   *
   * F49-A02 requires a cited entry to survive in the source: the deletion is
   * checked against the source first, so a key that is still cited is reported as
   * unresolved instead of pretending the citation is gone.
   */
  async function removeEntry(entryIndex: number): Promise<void> {
    if (bibliography === null || removing) {
      return;
    }
    const entry = bibliography.entries[entryIndex];
    if (entry === undefined) {
      return;
    }
    const nextText = removeBibEntry(bibText, entry);
    if (nextText === null) {
      setNotice(translator('citation.deleteStale'));
      setRemovalWarning(null);
      return;
    }
    const remaining: Bibliography = {
      entries: bibliography.entries.filter((_, index) => index !== entryIndex),
      errors: bibliography.errors,
      duplicates: [],
    };
    // The bibliography is about to change, so a plan from it is no longer valid.
    previewRequest.current += 1;
    setPlan(null);
    setSelected([]);
    setRemoving(true);
    const check = await service.bibliographyDangling(remaining, [source]);
    if (!check.ok) {
      setRemoving(false);
      setError(check.error);
      return;
    }
    setError(null);
    setNotice(null);
    setRemovalWarning(null);
    setBibText(nextText);
    const reparsed = await runParse(nextText);
    setRemoving(false);
    if (reparsed === null) {
      return;
    }
    if (check.value.includes(entry.citeKey)) {
      setRemovalWarning(translator('citation.deleteCited', { citeKey: entry.citeKey }));
    } else {
      setNotice(translator('citation.deleteDone', { citeKey: entry.citeKey }));
    }
  }

  async function preview(keys: string[]): Promise<void> {
    if (bibliography === null) {
      return;
    }
    const request = ++previewRequest.current;
    const outcome = await service.planCitation(bibliography, keys, backendRef.current, source);
    if (request !== previewRequest.current) {
      return;
    }
    if (!outcome.ok) {
      setError(outcome.error);
      return;
    }
    setPlan(outcome.value);
    setError(null);
  }

  function toggle(citeKey: string): void {
    // An in-flight preview still describes the old selection.
    previewRequest.current += 1;
    setSelected((current) =>
      current.includes(citeKey)
        ? current.filter((key) => key !== citeKey)
        : [...current, citeKey],
    );
    setPlan(null);
  }

  /**
   * Selects the unparsable block in the `.bib` textarea.
   *
   * F49-R01 requires the original text to stay visible and be locatable; the
   * parser offset is a UTF-8 byte position, so it is converted before use.
   */
  function locate(error: BibError): void {
    const field = bibRef.current;
    if (field === null) {
      return;
    }
    const { start, end } = bibErrorSelection(bibText, error);
    field.focus();
    // A stale parse can survive an edit; selecting an unrelated range would be
    // worse than leaving the caret where it was.
    if (bibText.slice(start, end) === error.raw) {
      field.setSelectionRange(start, end);
    }
  }

  /** DUPLICATE_CITE_KEY: each row must identify its own source block. */
  function locateEntry(entryIndex: number): void {
    if (bibliography === null) {
      return;
    }
    const entry = bibliography.entries[entryIndex];
    const field = bibRef.current;
    if (entry === undefined || field === null) {
      return;
    }
    const { start, end } = bibEntrySelection(bibText, entry);
    field.focus();
    if (bibText.slice(start, end) === entry.raw) {
      field.setSelectionRange(start, end);
    }
  }

  function locateOffset(offset: number): void {
    if (bibliography === null) {
      return;
    }
    const index = bibliography.entries.findIndex((entry) => entry.offset === offset);
    if (index >= 0) {
      locateEntry(index);
    }
  }

  function insert(): void {
    if (plan === null || !canInsertCitation(plan)) {
      return;
    }
    if (apply(plan.fragment)) {
      setNotice(translator('citation.done'));
      setPlan(null);
      setSelected([]);
    } else {
      setNotice(translator('citation.failed'));
    }
  }

  return (
    <aside
      className="citation-panel"
      aria-labelledby="citation-heading"
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="citation-head">
        <h2 id="citation-heading">{translator('citation.title')}</h2>
        <button
          type="button"
          className="citation-close"
          aria-label={translator('citation.close')}
          title={translator('citation.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <p className="citation-body">{translator('citation.body')}</p>

      <label className="citation-bib">
        <span>{translator('citation.bibLabel')}</span>
        <textarea
          ref={bibRef}
          rows={6}
          spellCheck={false}
          value={bibText}
          placeholder={translator('citation.bibPlaceholder')}
          onChange={(event) => {
            setBibText(event.target.value);
            // The parsed list and any plan are about the previous text, and an
            // in-flight parse must not publish a result for text that is gone.
            parseRequest.current += 1;
            previewRequest.current += 1;
            unicodeRequest.current += 1;
            setSelected([]);
            setPlan(null);
            setUnicodeKeys([]);
          }}
        />
      </label>

      <div className="citation-controls">
        <button type="button" className="primary" disabled={bibText.trim() === ''} onClick={() => void parse()}>
          {translator('citation.parse')}
        </button>
        <label>
          <span>{translator('citation.backend')}</span>
          <select
            value={backend}
            onChange={(event) => {
              const next = event.target.value as BibBackend;
              backendRef.current = next;
              // A plan names the backend it was planned for (F49-A03).
              previewRequest.current += 1;
              setBackend(next);
              setPlan(null);
              setUnicodeKeys([]);
              // The Unicode check depends on the backend, so it is re-run.
              if (bibliography !== null) {
                void refreshUnicode(bibliography, next);
              }
            }}
          >
            {BACKENDS.map((option) => (
              <option key={option} value={option}>
                {translator(`citation.backend.${option}`)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{translator('citation.filter')}</span>
          <input
            type="search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
      </div>

      {error !== null ? (
        <div className="citation-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
        </div>
      ) : null}
      {notice !== null ? (
        <p className="citation-notice" role="status">
          {notice}
        </p>
      ) : null}
      {removalWarning !== null ? (
        <p className="citation-warning" role="status">
          {removalWarning}
        </p>
      ) : null}

      {summary !== null ? (
        <p className="citation-summary" role="status">
          {translator('citation.summary', {
            entries: summary.entries,
            duplicates: summary.duplicates,
            errors: summary.errors,
            unknown: summary.unknownTypes,
          })}
        </p>
      ) : null}
      {bibliography !== null && bibliography.errors.length > 0 ? (
        <ul className="citation-errors" aria-label={translator('citation.issues')}>
          {bibliography.errors.map((error, index) => (
            <li key={`${error.offset}:${index}`} className="citation-error-item">
              <button
                type="button"
                className="citation-error-locate"
                aria-label={`${translator('citation.issues')}: ${error.offset}`}
                onClick={() => locate(error)}
              >
                #{error.offset}
              </button>
              <div className="citation-error-body">
                <p className="citation-error-message">
                  {translator(bibErrorMessageKey(error.messageKey))}
                </p>
                <pre className="citation-error-raw">{error.raw}</pre>
              </div>
            </li>
          ))}
        </ul>
      ) : null}
      {unicodeKeys.length > 0 ? (
        <p className="citation-warning" role="status">
          {translator('citation.unicode', {
            count: unicodeKeys.length,
            backend: translator(`citation.backend.${backend}`),
          })}
        </p>
      ) : null}

      {bibliography !== null ? (
        <>
          <ul className="citation-list" aria-label={translator('citation.listLabel')}>
            {items.map((item) => (
              <li
                key={`${item.entryIndex}:${item.citeKey}`}
                className="citation-row"
                data-duplicate={item.duplicate ? 'true' : undefined}
              >
                <input
                  type="checkbox"
                  checked={selected.includes(item.citeKey)}
                  // A duplicated key is not selectable: it must be fixed first
                  // (DUPLICATE_CITE_KEY). A stale parse is not selectable either.
                  disabled={item.duplicate || !parsedCurrent}
                  aria-label={translator('citation.select', { name: item.label })}
                  onChange={() => toggle(item.citeKey)}
                />
                <span className="citation-label">{item.label}</span>
                <span className="citation-key">{item.citeKey}</span>
                {item.duplicate || item.unknownType ? (
                  <span className="citation-badges">
                    {item.duplicate ? (
                      <span className="citation-badge">{translator('citation.duplicate')}</span>
                    ) : null}
                    {item.duplicate ? (
                      <button
                        type="button"
                        className="citation-offset"
                        aria-label={translator('citation.locateEntry', { offset: item.offset })}
                        title={translator('citation.locateEntry', { offset: item.offset })}
                        onClick={() => locateEntry(item.entryIndex)}
                      >
                        #{item.offset}
                      </button>
                    ) : null}
                    {item.unknownType ? (
                      <span className="citation-badge">{translator('citation.unknownType')}</span>
                    ) : null}
                  </span>
                ) : null}
                <button
                  type="button"
                  className="citation-row-delete"
                  aria-label={translator('citation.delete', { citeKey: item.citeKey })}
                  title={translator('citation.delete', { citeKey: item.citeKey })}
                  disabled={removing}
                  onClick={() => void removeEntry(item.entryIndex)}
                >
                  <Trash2 size={16} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
          {items.length === 0 ? (
            <p className="citation-empty" role="status">
              {translator('citation.empty')}
            </p>
          ) : null}

          <div className="citation-controls">
            <button
              type="button"
              className="secondary"
              disabled={selected.length === 0 || !parsedCurrent || removing}
              onClick={() => void preview(selected)}
            >
              {translator('citation.preview')}
            </button>
            <button
              type="button"
              className="primary"
              disabled={!parsedCurrent || removing || !canInsertCitation(plan)}
              onClick={insert}
            >
              {translator('citation.insert')}
            </button>
            <span className="citation-note">
              {translator('citation.selected', { count: selected.length, total: citable })}
            </span>
          </div>

          {plan !== null && plan.issues.length > 0 ? (
            <ul className="citation-issues" aria-label={translator('citation.issues')}>
              {plan.issues.map((issue, index) => (
                <li key={`${issue.kind}:${index}`}>
                  {citationIssueText(issue, translator)}
                  {issue.kind === 'duplicateKey' ? (
                    <span className="citation-offsets">
                      {issue.offsets.map((offset) => (
                        <button
                          key={offset}
                          type="button"
                          className="citation-offset"
                          aria-label={translator('citation.locateEntry', { offset })}
                          title={translator('citation.locateEntry', { offset })}
                          onClick={() => locateOffset(offset)}
                        >
                          #{offset}
                        </button>
                      ))}
                    </span>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : null}
          {plan !== null && plan.fragment !== '' ? (
            <pre className="citation-fragment">{plan.fragment}</pre>
          ) : null}
          {plan !== null && !plan.hasBibliographyPackage ? (
            <p className="citation-warning" role="status">
              {translator('citation.noPackage')}
            </p>
          ) : null}
        </>
      ) : null}

      <p className="citation-note">{translator('citation.hint')}</p>
    </aside>
  );
}
