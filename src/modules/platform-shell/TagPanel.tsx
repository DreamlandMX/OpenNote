import { useEffect, useMemo, useRef, useState } from 'react';
import { Hash, RefreshCw, X } from 'lucide-react';
import type { NoteService, TagReport, TagBatchResult, TagWriteRunner } from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  filterTags,
  hasMissingNotes,
  notesForTag,
  tagRows,
  tagSummary,
  validateTag,
} from './tags';
import { TagBatchStatus } from './TagBatchStatus';
import { TagRenameDialog } from './TagRenameDialog';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  knownNoteIds: readonly string[];
  onOpenNote(noteId: string, offset: number | null): void;
  onClose(): void;
  canWrite: boolean;
  prepareWrite(): Promise<boolean>;
  runWrite: TagWriteRunner;
}

/** S15 derives tags from note sources and exposes resumable per-note updates. */
export function TagPanel({
  service,
  translator,
  locale,
  knownNoteIds,
  canWrite,
  prepareWrite,
  runWrite,
  onOpenNote,
  onClose,
}: Props) {
  const [report, setReport] = useState<TagReport | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [loading, setLoading] = useState(false);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [noteQuery, setNoteQuery] = useState('');
  const [renameTag, setRenameTag] = useState<string | null>(null);
  const [batches, setBatches] = useState<readonly TagBatchResult[]>([]);
  const [batchBusy, setBatchBusy] = useState(false);
  const [batchRunId, setBatchRunId] = useState<string | null>(null);
  const openerRef = useRef<Element | null>(null);
  const sessionRef = useRef(0);

  async function load(): Promise<void> {
    const session = ++sessionRef.current;
    setLoading(true);
    const outcome = await service.listTags();
    if (session !== sessionRef.current) {
      return;
    }
    const pending = await service.listTagUpdates();
    if (session !== sessionRef.current) return;
    if (pending.ok) setBatches(pending.value);
    setLoading(false);
    if (outcome.ok) {
      setReport(outcome.value);
      setError(null);
    } else {
      setError(outcome.error);
    }
  }

  useEffect(() => {
    openerRef.current = document.activeElement;
    void load();
    return () => {
      sessionRef.current += 1;
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [service]);

  const rows = useMemo(() => (report === null ? [] : tagRows(report)), [report]);
  const visible = useMemo(() => filterTags(rows, query), [rows, query]);
  const active = visible.find((row) => row.tag === selected) ?? visible[0] ?? null;
  const notes = notesForTag(active, noteQuery);
  const summary = tagSummary(rows);
  const validation = validateTag(query);
  const missing =
    report !== null && knownNoteIds.length > 0 && hasMissingNotes(rows, knownNoteIds);

  return (
    <aside
      className="tag-panel"
      aria-labelledby="tag-heading"
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          if (!batchBusy && renameTag === null) onClose();
        }
      }}
    >
      <header className="tag-head">
        <h2 id="tag-heading">{translator('tags.title')}</h2>
        <button
          type="button"
          className="tag-close"
          aria-label={translator('tags.close')}
          title={translator('tags.close')}
          disabled={batchBusy || renameTag !== null}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <div className="tag-filter">
        <Hash size={16} aria-hidden="true" />
        <input
          type="search"
          value={query}
          aria-label={translator('tags.filter')}
          placeholder={translator('tags.filterPlaceholder')}
          onChange={(event) => setQuery(event.target.value)}
        />
        {query !== '' ? (
          <button
            type="button"
            className="tag-clear"
            aria-label={translator('tags.clearFilter')}
            title={translator('tags.clearFilter')}
            onClick={() => setQuery('')}
          >
            <X size={16} aria-hidden="true" />
          </button>
        ) : null}
      </div>

      {query !== '' && !validation.ok ? (
        <p className="tag-warning" role="status">
          {validation.reason === 'tooLong' ? translator('tags.tooLong') : translator('tags.empty')}
        </p>
      ) : null}

      <p className="tag-status" role="status">
        {loading
          ? translator('tags.loading')
          : translator('tags.summary', { tags: summary.tags, notes: summary.notes })}
      </p>

      {error !== null ? (
        <div className="tag-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
          <button type="button" className="secondary" onClick={() => void load()}>
            {translator('tags.retry')}
          </button>
        </div>
      ) : null}

      {missing ? (
        <p className="tag-warning" role="status">
          {translator('tags.missingNote')}
          <button type="button" className="tag-refresh" onClick={() => void load()}>
            <RefreshCw size={14} aria-hidden="true" />
            {translator('tags.refresh')}
          </button>
        </p>
      ) : null}

      {error === null && !loading && rows.length === 0 ? (
        <p className="tag-empty" role="status">
          {translator('tags.emptyState')}
        </p>
      ) : null}

      {error === null && rows.length > 0 ? (
        <div className="tag-layout">
          <ul className="tag-list" aria-label={translator('tags.listLabel')}>
            {visible.length === 0 ? (
              <li className="tag-empty" role="status">
                {translator('tags.noMatch')}
              </li>
            ) : (
              visible.map((row) => (
                <li key={row.tag}>
                  <button
                    type="button"
                    className="tag-row"
                    aria-current={active?.tag === row.tag ? 'true' : undefined}
                    onClick={() => {
                      setSelected(row.tag);
                      setNoteQuery('');
                    }}
                  >
                    <span className="tag-name">{row.tag}</span>
                    <span className="tag-count">{row.count}</span>
                  </button>
                </li>
              ))
            )}
          </ul>
          <div className="tag-notes">
            {active === null ? null : (
              <>
                <div className="tag-notes-head">
                  <h3>{`#${active.tag}`}</h3>
                  <button
                    type="button"
                    className="secondary"
                    disabled={!canWrite || batchBusy || renameTag !== null}
                    onClick={() => void (async () => {
                      setBatchBusy(true);
                      try { if (await prepareWrite()) setRenameTag(active.tag); }
                      finally { setBatchBusy(false); }
                    })()}
                  >
                    {translator('tags.renameAction')}
                  </button>
                  <input
                    type="search"
                    value={noteQuery}
                    aria-label={translator('tags.filterNotes')}
                    placeholder={translator('tags.filterNotes')}
                    onChange={(event) => setNoteQuery(event.target.value)}
                  />
                </div>
                {notes.length === 0 ? (
                  <p className="tag-empty" role="status">
                    {translator('tags.noNotes')}
                  </p>
                ) : (
                  <ul>
                    {notes.map((note) => (
                      <li key={note.noteId}>
                        <button type="button" disabled={batchBusy || renameTag !== null} onClick={() => onOpenNote(note.noteId, null)}>
                          {note.relativePath}
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </>
            )}
          </div>
        </div>
      ) : null}

      {renameTag !== null ? (
        <TagRenameDialog service={service} translator={translator} locale={locale} tag={renameTag}
          runWrite={runWrite} onApplied={() => void load()} onClose={() => { setRenameTag(null); void load(); }} />
      ) : null}
      {batches.map(batch => <section key={batch.operationId}>
        <h3>{batch.newTag === null ? `#${batch.oldTag}` : `#${batch.oldTag} → #${batch.newTag}`}</h3>
        <TagBatchStatus result={batch} translator={translator} locale={locale} />
        {!batch.completed && !batch.cancelled ? <div className="panel-actions">
          <button type="button" disabled={!canWrite || batchBusy || renameTag !== null} onClick={() => void (async () => {
            setBatchBusy(true); setBatchRunId(batch.operationId);
            try {
              const outcome = await runWrite(() => service.resumeTagUpdate(batch.operationId), batch.items.map(item => item.noteId));
              if (outcome.ok) setBatches(previous => previous.map(item => item.operationId === batch.operationId ? outcome.value : item));
              else setError(outcome.error);
              const tags = await service.listTags(); if (tags.ok) setReport(tags.value);
            } finally { setBatchBusy(false); setBatchRunId(null); }
          })()}>{translator('tags.resume')}</button>
          <button type="button" disabled={renameTag !== null || (!canWrite && batchRunId !== batch.operationId)} onClick={() => void (async () => {
            const outcome = await service.cancelTagUpdate(batch.operationId);
            if (outcome.ok) setBatches(previous => previous.map(item => item.operationId === batch.operationId ? outcome.value : item));
            else setError(outcome.error);
          })()}>{translator('tags.cancelBatch')}</button>
        </div> : null}
      </section>)}


    </aside>
  );
}
