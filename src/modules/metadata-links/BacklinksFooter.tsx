import { useEffect, useState } from 'react';
import type { BacklinkReport, NoteService } from '../editor';
import type { Translator } from '../presentation-i18n';
import { backlinkRows, type MetadataRow } from './view-model';

interface Props {
  noteId: string;
  service: Pick<NoteService, 'listBacklinks'>;
  translator: Translator;
  /** Changes whenever the vault catalog changes, so a new link shows up without reopening. */
  refreshKey: string;
  onOpenNote(noteId: string): void;
}

/**
 * Notes that link here, under the page as in Obsidian and Notion. Collapsed by
 * default and absent when nothing links here; a failed lookup also renders
 * nothing, because the metadata panel still reports the error.
 */
export function BacklinksFooter({ noteId, service, translator, refreshKey, onOpenNote }: Props) {
  const [report, setReport] = useState<BacklinkReport | null>(null);

  useEffect(() => {
    let cancelled = false;
    void service.listBacklinks(noteId).then(outcome => {
      if (!cancelled) setReport(outcome.ok && outcome.value.noteId === noteId ? outcome.value : null);
    });
    return () => { cancelled = true; };
  }, [noteId, service, refreshKey]);

  const rows = report?.noteId === noteId ? backlinkRows(report) : [];
  return <BacklinksList rows={rows} translator={translator} onOpenNote={onOpenNote} />;
}

export function BacklinksList({ rows, translator, onOpenNote }: {
  rows: readonly MetadataRow[];
  translator: Translator;
  onOpenNote(noteId: string): void;
}) {
  if (rows.length === 0) return null;
  return (
    <details className="backlinks-footer">
      <summary>{translator('backlinks.footer', { count: rows.length })}</summary>
      <ul>
        {rows.map(row => (
          <li key={row.key}>
            <button type="button" onClick={() => { if (row.noteId) onOpenNote(row.noteId); }}>
              {row.label.replace(/\.md$/i, '')}
            </button>
          </li>
        ))}
      </ul>
    </details>
  );
}
