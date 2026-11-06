import { runSearchTask } from './search-task';
import { SearchProgressNotice } from './SearchProgressNotice';
import { serviceErrorFromThrown } from './port';
import {
  clearSearchHistory,
  readDevicePreferences,
  rememberSearchHistoryQuery,
} from './device-prefs';
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { Clock3, FileText, Folder, Search, Tag, X } from 'lucide-react';
import { PanelHeader } from './PanelHeader';
import type { NoteSummary, ServiceError } from '../domain';
import type { NoteService, SearchFilters, SearchItem, SearchJob, SearchPage } from '../editor';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';

const DEBOUNCE_MS = 200;

export function searchSourceKey(notes: readonly NoteSummary[]): string {
  return `[${notes.map(note => JSON.stringify([note.vaultId, note.noteId, note.relativePath, note.contentHash, note.readOnly])).sort().join(',')}]`;
}

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  notes: readonly NoteSummary[];
  recentNotes: readonly NoteSummary[];
  onOpenNote(noteId: string, sourceOffset: number): void;
  onClose(): void;
}

export function foldersOf(notes: readonly Pick<NoteSummary, 'relativePath'>[]): string[] {
  const folders = new Set<string>();
  for (const note of notes) {
    const segments = note.relativePath.replace(/\\/g, '/').split('/');
    segments.pop();
    let prefix = '';
    for (const segment of segments) {
      prefix = prefix === '' ? segment : `${prefix}/${segment}`;
      if (prefix !== '') folders.add(prefix);
    }
  }
  return [...folders].sort((left, right) => left.localeCompare(right));
}

function titleOf(relativePath: string): string {
  const name = relativePath.split(/[/\\]/).pop() ?? relativePath;
  return name.replace(/\.md$/i, '');
}

/**
 * S19 full-text search. The panel is a dialog over the workspace: it never
 * edits notes and only asks the host for a page. A query is sent 200 ms after
 * typing stops; every response carries a sequence number so an older answer
 * can never replace a newer one (F12-A03), including when the query changed
 * while a page was loading.
 */
export function SearchPanel({
  service,
  translator,
  locale,
  notes,
  recentNotes,
  onOpenNote,
  onClose,
}: Props) {
  const [query, setQuery] = useState('');
  const [searchHistory, setSearchHistory] = useState(() => readDevicePreferences().searchHistory);
  const [folder, setFolder] = useState('');
  const [tag, setTag] = useState('');
  const [type, setType] = useState('');
  const [extraFolders, setExtraFolders] = useState<readonly string[]>([]);
  const [extraTags, setExtraTags] = useState<readonly string[]>([]);
  const [extraTypes, setExtraTypes] = useState<readonly string[]>([]);
  const [tags, setTags] = useState<readonly string[]>([]);
  const [tagLoading, setTagLoading] = useState(false);
  const [tagError, setTagError] = useState<ServiceError | null>(null);
  const [tagUnreadable, setTagUnreadable] = useState(0);
  const [tagRetry, setTagRetry] = useState(0);
  const [page, setPage] = useState<SearchPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<ServiceError | null>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const [job, setJob] = useState<SearchJob | null>(null);
  const [cancelled, setCancelled] = useState(false);
  const [cancelPending, setCancelPending] = useState(false);
  const [cancelError, setCancelError] = useState(false);
  const [monitorError, setMonitorError] = useState(false);
  const activeTaskRef = useRef<ReturnType<typeof runSearchTask> | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const openerRef = useRef<Element | null>(null);
  const sequenceRef = useRef(0);
  const pageRef = useRef<SearchPage | null>(null);
  const latestRequestKeyRef = useRef<string | null>(null);
  /** Query + filters that produced `page`; stale pages stay visible but inert. */
  const resultsKeyRef = useRef<string | null>(null);
  pageRef.current = page;

  const folders = useMemo(() => foldersOf(notes), [notes]);
  const sourceKey = useMemo(() => searchSourceKey(notes), [notes]);
  const latestSourceKeyRef = useRef(sourceKey);
  latestSourceKeyRef.current = sourceKey;

  useEffect(() => {
    openerRef.current = document.activeElement;
    inputRef.current?.focus();
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    setTagLoading(true); setTagError(null);
    const current = () => !cancelled && sourceKey === latestSourceKeyRef.current;
    void service.listTags().then((outcome) => {
      if (!current()) return;
      setTagLoading(false);
      if (outcome.ok) {
        setTags(outcome.value.tags.map((entry) => entry.tag).sort((a, b) => a.localeCompare(b)));
        setTagUnreadable(outcome.value.unreadable.length);
      } else { setTagError(outcome.error); }
    }, error => { if (current()) { setTagLoading(false); setTagError(serviceErrorFromThrown(error)); } });
    return () => { cancelled = true; };
  }, [service, sourceKey, tagRetry]);

  const trimmed = query.trim();

  function filters(): SearchFilters {
    // Empty values are ignored, duplicates collapse through the Set. The same
    // class is OR (F12 §4); different classes stay AND on the host.
    const folders = [folder, ...extraFolders].filter((value) => value !== '');
    const tagValues = [tag, ...extraTags].filter((value) => value !== '');
    const typeValues = [type, ...extraTypes]
      .map((value) => value.trim())
      .filter((value) => value !== '');
    return {
      folders: [...new Set(folders)],
      tags: [...new Set(tagValues.map((value) => value.replace(/^#/, '')))],
      types: [...new Set(typeValues)],
    };
  }

  function addFilter(
    value: string,
    current: readonly string[],
    update: (next: readonly string[]) => void,
  ): void {
    const trimmedValue = value.trim();
    if (trimmedValue === '' || current.includes(trimmedValue)) {
      return;
    }
    update([...current, trimmedValue]);
  }

  const currentRequestKey = useMemo(() => {
    const active = filters();
    return JSON.stringify([trimmed, active.folders, active.tags, active.types, locale, sourceKey]);
  }, [trimmed, folder, tag, type, extraFolders, extraTags, extraTypes, locale, sourceKey]);
  latestRequestKeyRef.current = currentRequestKey;
  const resultsCurrent = resultsKeyRef.current === currentRequestKey;

  async function fetchPage(
    session: number,
    cursor: string | null,
    append: boolean,
    key: string,
  ): Promise<void> {
    activeTaskRef.current?.stop();
    resultsKeyRef.current = null;
    setJob(null); setCancelled(false); setCancelPending(false); setCancelError(false); setMonitorError(false);
    const current = () => session === sequenceRef.current && key === latestRequestKeyRef.current;
    const task = runSearchTask(service, {
      query: trimmed,
      filters: filters(),
      locale,
      limit: 50,
      cursor,
    }, value => { if (current()) { setJob(value); setMonitorError(false); } }, () => { if (current()) setMonitorError(true); });
    if (cursor === null) setSearchHistory(rememberSearchHistoryQuery(trimmed));
    activeTaskRef.current = task;
    const outcome = await task.result;
    if (activeTaskRef.current === task) activeTaskRef.current = null;
    if (outcome === null || !current()) {
      return;
    }
    if (!outcome.ok) {
      setJob(null);
      if (outcome.error.code === 'OPERATION_CANCELLED') {
        resultsKeyRef.current = null; setCancelled(true); setError(null); setLoading(false); setLoadingMore(false); return;
      }
      if (outcome.error.messageKey === 'error.searchCursorExpired' && cursor !== null) {
        // The note set (and therefore the index version) changed between
        // pages, so the cursor is correctly refused. Restart page one for the
        // same request instead of showing a failure the user cannot act on.
        setPage(null);
        setActiveIndex(0);
        setLoading(true);
        setLoadingMore(false);
        setError(null);
        void fetchPage(session, null, false, key);
        return;
      }
      setError(outcome.error);
      setLoading(false);
      setLoadingMore(false);
      return;
    }
    setError(null);
    resultsKeyRef.current = key;
    setPage((previous) => {
      if (!append || previous === null) {
        return outcome.value;
      }
      return {
        ...outcome.value,
        items: [...previous.items, ...outcome.value.items],
      };
    });
    if (!append) {
      setActiveIndex(0);
    }
    setLoading(false);
    setLoadingMore(false);
  }

  async function cancelSearch(): Promise<void> {
    const task = activeTaskRef.current;
    if (task === null) return;
    setCancelPending(true); setCancelError(false);
    const outcome = await task.cancel();
    if (activeTaskRef.current !== task) return;
    setCancelPending(false);
    if (!outcome.ok) setCancelError(true);
  }

  useEffect(() => () => { sequenceRef.current += 1; activeTaskRef.current?.stop(); activeTaskRef.current = null; }, []);

  useEffect(() => {
    setJob(null); setCancelled(false); setCancelPending(false); setCancelError(false); setMonitorError(false);
    if (trimmed === '') {
      sequenceRef.current += 1;
      resultsKeyRef.current = null;
      setPage(null);
      setError(null);
      setLoading(false);
      setLoadingMore(false);
      return;
    }
    const session = ++sequenceRef.current;
    const key = currentRequestKey;
    setLoading(true);
    setError(null);
    const timer = window.setTimeout(() => {
      void fetchPage(session, null, false, key);
    }, DEBOUNCE_MS);
    return () => {
      window.clearTimeout(timer);
      sequenceRef.current += 1;
      activeTaskRef.current?.stop(); activeTaskRef.current = null;
    };
    // The key includes catalog identity/content, so old pages and cursors cannot cross versions.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentRequestKey, service]);

  function retry(): void {
    if (trimmed === '') {
      return;
    }
    const session = ++sequenceRef.current;
    setLoading(true);
    setError(null);
    void fetchPage(session, null, false, currentRequestKey);
  }

  function loadMore(): void {
    const current = pageRef.current;
    if (current === null || current.nextCursor === null || loadingMore) {
      return;
    }
    if (resultsKeyRef.current !== currentRequestKey) {
      return;
    }
    const session = ++sequenceRef.current;
    setLoadingMore(true);
    setError(null);
    void fetchPage(session, current.nextCursor, true, currentRequestKey);
  }

  function open(item: SearchItem | undefined): void {
    if (item === undefined || resultsKeyRef.current !== currentRequestKey) {
      return;
    }
    onOpenNote(item.noteId, item.sourceOffset);
  }

  function handleSearchKeyDown(event: KeyboardEvent<HTMLElement>): void {
    // An IME composition owns these keys; Enter commits the composition, it
    // does not open a result (S19 input contract).
    if (event.nativeEvent.isComposing) {
      return;
    }
    const items = page?.items ?? [];
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (items.length === 0) {
        return;
      }
      event.preventDefault();
      const step = event.key === 'ArrowDown' ? 1 : -1;
      const next = (activeIndex + step + items.length) % items.length;
      setActiveIndex(next);
      // Focus stays in the query field (combobox pattern), so the user can keep
      // typing; the active option is announced through aria-activedescendant.
      document.getElementById(`search-result-${next}`)?.scrollIntoView({ block: 'nearest' });
      return;
    }
    if (event.key === 'Enter' && event.target === inputRef.current) {
      if (resultsKeyRef.current !== currentRequestKey) {
        // Enter before the debounce (or before a new query returned) should
        // search, not open a result from the previous query.
        event.preventDefault();
        retry();
        return;
      }
      if (items.length === 0) {
        return;
      }
      event.preventDefault();
      open(items[activeIndex] ?? items[0]);
    }
  }

  function clearFilters(): void {
    setFolder('');
    setTag('');
    setType('');
    setExtraFolders([]);
    setExtraTags([]);
    setExtraTypes([]);
  }

  function clearHistory(): void {
    clearSearchHistory();
    setSearchHistory([]);
    inputRef.current?.focus();
  }

  const filtersActive =
    folder !== '' || tag !== '' || type.trim() !== '' || extraFolders.length > 0 || extraTags.length > 0 || extraTypes.length > 0;
  const items = page?.items ?? [];
  const staleIndex = page !== null && page.indexStatus.unreadableNotes > 0;

  return (
    <section
      className="search-screen"
      aria-labelledby="search-heading"
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          onClose();
          return;
        }
        handleSearchKeyDown(event);
      }}
    >
      <PanelHeader id="search-heading" icon={Search} title={translator('search.title')}
        purpose={translator('search.purpose')} closeLabel={translator('search.close')} onClose={onClose} />

      <div className="search-field">
        <Search size={20} aria-hidden="true" />
        <input
          ref={inputRef}
          type="search"
          value={query}
          autoComplete="off"
          spellCheck={false}
          role="combobox"
          aria-expanded={items.length > 0 ? 'true' : 'false'}
          aria-autocomplete="list"
          aria-label={translator('search.inputLabel')}
          placeholder={translator('search.placeholder')}
          aria-controls="search-results"
          aria-activedescendant={resultsCurrent && items.length > 0 ? `search-result-${activeIndex}` : undefined}
          onChange={(event) => setQuery(event.target.value)}
        />
        {query !== '' ? (
          <button
            type="button"
            className="search-clear"
            aria-label={translator('search.clear')}
            title={translator('search.clear')}
            onClick={() => {
              setQuery('');
              inputRef.current?.focus();
            }}
          >
            <X size={18} aria-hidden="true" />
          </button>
        ) : null}
      </div>

      <div className="search-filters">
        <label className="search-filter">
          <Folder size={16} aria-hidden="true" />
          <span>{translator('search.folder')}</span>
          <select value={folder} onChange={(event) => setFolder(event.target.value)}>
            <option value="">{translator('search.allFolders')}</option>
            {folder !== '' && !folders.includes(folder) ? <option value={folder}>{translator('search.filterUnavailable', { name: folder })}</option> : null}
            {folders.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </select>
        </label>
        <label className="search-filter">
          <Tag size={16} aria-hidden="true" />
          <span>{translator('search.tag')}</span>
          <select value={tag} onChange={(event) => setTag(event.target.value)}>
            <option value="">{translator('search.allTags')}</option>
            {tag !== '' && !tags.includes(tag) ? <option value={tag}>{translator('search.filterUnavailable', { name: tag })}</option> : null}
            {tags.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </select>
        </label>
        <label className="search-filter">
          <span>{translator('search.type')}</span>
          <input
            type="text"
            value={type}
            maxLength={100}
            placeholder={translator('search.anyType')}
            onChange={(event) => setType(event.target.value)}
          />
        </label>
        {filtersActive ? null : <span className="panel-note search-optional">{translator('search.filtersOptional')}</span>}
        {filtersActive ? (
          <button type="button" className="search-clear-filters" onClick={clearFilters}>
            {translator('search.clearFilters')}
          </button>
        ) : null}
        <div className="search-filter-actions">
          {folder !== '' ? (
            <button
              type="button"
              className="search-chip-add"
              onClick={() => {
                addFilter(folder, extraFolders, setExtraFolders);
                setFolder('');
              }}
            >
              + {folder}
            </button>
          ) : null}
          {tag !== '' ? (
            <button
              type="button"
              className="search-chip-add"
              onClick={() => {
                addFilter(tag, extraTags, setExtraTags);
                setTag('');
              }}
            >
              + {tag}
            </button>
          ) : null}
          {type.trim() !== '' ? (
            <button
              type="button"
              className="search-chip-add"
              onClick={() => {
                addFilter(type, extraTypes, setExtraTypes);
                setType('');
              }}
            >
              + {type.trim()}
            </button>
          ) : null}
        </div>
        {extraFolders.length > 0 || extraTags.length > 0 || extraTypes.length > 0 ? (
          <ul className="search-chips" aria-label={translator('search.activeFilters')}>
            {[
              ...extraFolders.map((value) => ({ kind: 'folder' as const, value, remove: () => setExtraFolders((current) => current.filter((entry) => entry !== value)) })),
              ...extraTags.map((value) => ({ kind: 'tag' as const, value, remove: () => setExtraTags((current) => current.filter((entry) => entry !== value)) })),
              ...extraTypes.map((value) => ({ kind: 'type' as const, value, remove: () => setExtraTypes((current) => current.filter((entry) => entry !== value)) })),
            ].map((chip) => (
              <li key={`${chip.kind}:${chip.value}`}>
                <span>{chip.kind === 'folder' ? translator('search.folder') : chip.kind === 'tag' ? translator('search.tag') : translator('search.type')}: {chip.value}</span>
                <button
                  type="button"
                  aria-label={translator('search.removeFilter', { name: chip.value })}
                  title={translator('search.removeFilter', { name: chip.value })}
                  onClick={chip.remove}
                >
                  <X size={14} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        ) : null}
        {tagLoading ? <p role="status">{translator('tags.loading')}</p> : null}
        {tagError ? <div role="alert"><p>{translator('search.tag')}: {serviceErrorMessage(locale, tagError.code)}</p><button type="button" className="secondary" onClick={() => setTagRetry(value => value + 1)}>{translator('tags.retry')}</button></div> : null}
        {tagUnreadable > 0 ? <p role="status">{translator('metadata.unreadable')}: {tagUnreadable}</p> : null}
        <p className="search-hint">{translator('search.hint')}</p>
      </div>

      <p className="search-status" role="status">
        {loading && page === null ? translator('search.loading') : null}
        {resultsCurrent && !loading && !cancelled && error === null && page !== null
          ? translator('search.results', { count: page.items.length })
          : null}
        {loading && page !== null ? translator('search.loading') : null}
        {resultsCurrent && page !== null
          ? ` · ${translator('search.indexReady', { count: page.indexStatus.indexedNotes })}`
          : null}
      </p>
      {page !== null && !resultsCurrent ? <p className="search-warning" role="status">{translator('search.staleResults')}</p> : null}
      <SearchProgressNotice job={job} translator={translator} cancelPending={cancelPending} cancelFailed={cancelError} monitorFailed={monitorError} onCancel={() => { void cancelSearch(); }} />
      {cancelled ? <div role="status"><p>{translator('search.cancelled')}</p><button type="button" className="secondary" onClick={retry}>{translator('search.retry')}</button></div> : null}
      {staleIndex && page !== null ? (
        <p className="search-warning" role="status">
          {translator('search.indexIncomplete', { count: page.indexStatus.unreadableNotes })}
        </p>
      ) : null}
      {resultsCurrent && page?.indexStatus.cachePersisted === false ? <div className="search-warning" role="status"><p>{translator('search.memoryOnly')}</p><button type="button" className="secondary" onClick={retry}>{translator('search.retry')}</button></div> : null}

      {error !== null ? (
        <div className="search-error" role="alert">
          <p>
            <strong>
              {error.messageKey === 'error.searchIndexUnavailable'
                ? translator('search.indexUnavailable')
                : error.messageKey === 'error.searchQueryTooLong'
                  ? translator('search.queryTooLong')
                  : translator('search.error')}
            </strong>
          </p>
          <p>{serviceErrorMessage(locale, error.code)}</p>
          <button type="button" className="secondary" onClick={retry}>
            {translator('search.retry')}
          </button>
        </div>
      ) : null}

      {error === null && trimmed === '' ? (
        <div className="search-empty" role="status">
          <p>{translator('search.empty')}</p>
          {searchHistory.length > 0 ? (
            <section aria-labelledby="search-history-heading">
              <div className="search-history-heading">
                <h3 id="search-history-heading">{translator('search.history')}</h3>
                <button type="button" className="secondary" onClick={clearHistory}>
                  {translator('search.clearHistory')}
                </button>
              </div>
              <ul className="search-history">
                {searchHistory.map((value) => (
                  <li key={value}>
                    <button type="button" onClick={() => setQuery(value)}>
                      <Clock3 size={16} aria-hidden="true" />
                      <span>{value}</span>
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
          {recentNotes.length > 0 ? (
            <>
              <h3>{translator('search.recent')}</h3>
              <ul className="search-recent">
                {recentNotes.slice(0, 8).map((note) => (
                  <li key={note.noteId}>
                    <button type="button" onClick={() => onOpenNote(note.noteId, 0)}>
                      <FileText size={16} aria-hidden="true" />
                      <span>
                        <strong>{titleOf(note.relativePath)}</strong>
                        <small>{note.relativePath}</small>
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </>
          ) : null}
        </div>
      ) : null}

      {resultsCurrent && error === null && !cancelled && !loading && page !== null && items.length === 0 ? (
        <p className="search-empty" role="status">
          {translator('search.noResults')}
        </p>
      ) : null}

      {items.length > 0 ? (
        <ul
          className="search-results"
          id="search-results"
          role="listbox"
          aria-label={translator('search.resultsLabel')}
          aria-busy={loading || loadingMore}
        >
          {items.map((item, index) => (
            <li
              key={item.noteId}
              id={`search-result-${index}`}
              role="option"
              aria-disabled={!resultsCurrent}
              aria-selected={resultsCurrent && index === activeIndex ? 'true' : 'false'}
              className="search-result"
              onMouseEnter={() => setActiveIndex(index)}
              onClick={() => open(item)}
            >
              <FileText size={20} aria-hidden="true" />
              <span className="search-result-main">
                <strong>{item.title}</strong>
                <small>{item.relativePath}</small>
                <span className="search-snippet">
                  {snippetParts(item).map((part, partIndex) =>
                    part.highlight ? (
                      <mark key={partIndex}>{part.text}</mark>
                    ) : (
                      <span key={partIndex}>{part.text}</span>
                    ),
                  )}
                </span>
              </span>
            </li>
          ))}
        </ul>
      ) : null}

      {page !== null && page.nextCursor !== null && error === null ? (
        <div className="search-more">
          <button type="button" className="secondary" disabled={loading || loadingMore || !resultsCurrent} onClick={loadMore}>
            {loadingMore ? translator('search.loadingMore') : translator('search.loadMore')}
          </button>
        </div>
      ) : null}
    </section>
  );
}

interface SnippetPart {
  text: string;
  highlight: boolean;
}

/**
 * Splits a snippet by UTF-16 offsets. JavaScript string indexing uses the same
 * units as the host, so Turkish and astral characters cannot shift a mark.
 */
export function snippetParts(
  item: Pick<SearchItem, 'snippet' | 'highlights'>,
): SnippetPart[] {
  const length = item.snippet.length;
  const merged: { start: number; end: number }[] = [];
  for (const highlight of [...item.highlights].sort((left, right) => left.start - right.start)) {
    const start = Math.max(0, Math.min(length, highlight.start));
    const end = Math.min(length, Math.max(start, highlight.end));
    if (end <= start) {
      continue;
    }
    const previous = merged[merged.length - 1];
    if (previous !== undefined && start <= previous.end) {
      previous.end = Math.max(previous.end, end);
    } else {
      merged.push({ start, end });
    }
  }
  const parts: SnippetPart[] = [];
  let cursor = 0;
  for (const highlight of merged) {
    if (highlight.start > cursor) {
      parts.push({ text: item.snippet.slice(cursor, highlight.start), highlight: false });
    }
    parts.push({ text: item.snippet.slice(highlight.start, highlight.end), highlight: true });
    cursor = highlight.end;
  }
  if (cursor < length) {
    parts.push({ text: item.snippet.slice(cursor), highlight: false });
  }
  return parts;
}
