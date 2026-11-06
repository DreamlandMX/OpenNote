import { useEffect, useRef, useState } from 'react';
import type {
  BacklinkReport,
  LinkReport,
  NoteMetadataReport,
  NoteService,
  TagReport,
} from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  backlinkRows,
  linkIssueRows,
  noteSections,
  tabAvailability,
  tagRows,
  unreadableRows,
  type MetadataRow,
} from './view-model';

type Tab = 'tags' | 'backlinks' | 'issues';

interface Props {
  noteId: string | null;
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** Opens a note by id; the panel never navigates on its own. */
  onOpenNote(noteId: string, offset: number | null): void;
  onClose(): void;
}

/**
 * Metadata side panel: vault tags, the current note's backlinks, and every link
 * problem. Every list is rebuilt from the host on open, so it can never show a
 * stale association. The panel is read-only: it never edits a note.
 */
export function MetadataPanel({
  noteId,
  service,
  translator,
  locale,
  onOpenNote,
  onClose,
}: Props) {
  const [tab, setTab] = useState<Tab>('tags');
  const [tags, setTags] = useState<TagReport | null>(null);
  const [backlinks, setBacklinks] = useState<BacklinkReport | null>(null);
  const [issues, setIssues] = useState<LinkReport | null>(null);
  const [metadata, setMetadata] = useState<NoteMetadataReport | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [loading, setLoading] = useState(false);
  const dialogRef = useRef<HTMLDialogElement | null>(null);
  const openerRef = useRef<Element | null>(null);

  useEffect(() => {
    openerRef.current = document.activeElement;
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) {
      dialog.showModal();
    }
    // Focus returns to the trigger on close, which keeps keyboard users in
    // place instead of dropping them at the document start.
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    void (async () => {
      const tagOutcome = await service.listTags();
      if (cancelled) return;
      if (tagOutcome.ok) {
        setTags(tagOutcome.value);
      } else {
        setError(tagOutcome.error);
      }
      const issueOutcome = await service.listLinkIssues();
      if (cancelled) return;
      if (issueOutcome.ok) {
        setIssues(issueOutcome.value);
      }
      if (noteId !== null) {
        const backlinkOutcome = await service.listBacklinks(noteId);
        if (cancelled) return;
        if (backlinkOutcome.ok) {
          setBacklinks(backlinkOutcome.value);
        }
        const metaOutcome = await service.readNoteMetadata(noteId);
        if (cancelled) return;
        if (metaOutcome.ok) {
          setMetadata(metaOutcome.value);
        }
      }
      if (!cancelled) setLoading(false);
    })();
    return () => {
      cancelled = true;
    };
  }, [noteId, service]);

  const available = tabAvailability(noteId);

  const rows: readonly MetadataRow[] =
    tab === 'tags'
      ? tags
        ? tagRows(tags)
        : []
      : tab === 'backlinks'
        ? backlinks
          ? backlinkRows(backlinks)
          : []
        : issues
          ? linkIssueRows(issues)
          : [];

  const unreadable = [
    ...unreadableRows(tags?.unreadable ?? []),
    ...unreadableRows(backlinks?.unreadable ?? []),
    ...unreadableRows(issues?.unreadable ?? []),
  ];

  const emptyKey =
    tab === 'tags'
      ? 'metadata.tagsEmpty'
      : tab === 'backlinks'
        ? 'metadata.backlinksEmpty'
        : 'metadata.issuesEmpty';

  return (
    <dialog
      ref={dialogRef}
      className="metadata-panel"
      aria-label={translator('metadata.title')}
      onClose={onClose}
      onCancel={event => {
        event.preventDefault();
        onClose();
      }}
    >
      <header className="metadata-panel-header">
        <h2>{translator('metadata.title')}</h2>
        <button type="button" className="metadata-close" onClick={onClose}>
          {translator('metadata.close')}
        </button>
      </header>

      <div role="tablist" aria-label={translator('metadata.title')} className="metadata-tabs">
        {(
          [
            ['tags', 'metadata.tabTags'],
            ['backlinks', 'metadata.tabBacklinks'],
            ['issues', 'metadata.tabIssues'],
          ] as const
        ).map(([id, key]) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            aria-pressed={tab === id}
            className={tab === id ? 'metadata-tab is-active' : 'metadata-tab'}
            disabled={!available[id]}
            title={available[id] ? undefined : translator('metadata.noNoteOpen')}
            onClick={() => setTab(id)}
          >
            {translator(key)}
          </button>
        ))}
      </div>

      {error !== null && (
        <p role="alert" className="metadata-error">
          {serviceErrorMessage(locale, error.code)}
        </p>
      )}

      {loading && <p className="metadata-status">{translator('metadata.loading')}</p>}

      {!loading && rows.length === 0 && (
        <p className="metadata-empty">{translator(emptyKey)}</p>
      )}

      {rows.length > 0 && (
        <ul className="metadata-list">
          {rows.map(row => (
            <li key={row.key} className="metadata-row">
              <button
                type="button"
                className="metadata-row-button"
                onClick={() => {
                  if (row.noteId !== null) {
                    onOpenNote(row.noteId, row.offset);
                  }
                }}
                disabled={row.noteId === null}
              >
                <span className="metadata-row-label">{row.label}</span>
                {row.detail.length > 0 && (
                  <span className="metadata-row-detail">{row.detail}</span>
                )}
              </button>
            </li>
          ))}
        </ul>
      )}

      {unreadable.length > 0 && (
        <section className="metadata-unreadable" aria-live="polite">
          <h3>{translator('metadata.unreadable')}</h3>
          <ul>
            {unreadable.map(row => (
              <li key={row.key}>
                <span>{row.label}</span>
                <span>{row.detail}</span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {metadata !== null && (
        <section className="metadata-sections">
          {noteSections(metadata).map(section => (
            <div key={section.titleKey}>
              <h3>{translator(section.titleKey as never)}</h3>
              {section.empty ? (
                <p className="metadata-empty">{translator('metadata.sectionEmpty')}</p>
              ) : (
                <ul>
                  {section.rows.map(row => (
                    <li key={row.key}>{row.label}</li>
                  ))}
                </ul>
              )}
            </div>
          ))}
        </section>
      )}
    </dialog>
  );
}
