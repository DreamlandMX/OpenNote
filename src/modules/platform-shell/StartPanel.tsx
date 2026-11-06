import { CalendarDays, FileText, Sigma } from 'lucide-react';
import type { Translator } from '../presentation-i18n';

export interface StartRecent {
  noteId: string;
  label: string;
  title: string;
  available: boolean;
}

export interface StartPanelProps {
  translator: Translator;
  vaultName: string;
  writable: boolean;
  /** Shortcut text for a new note, if bound. */
  newNoteHint: string;
  newNoteDisabled: boolean;
  todayDisabled: boolean;
  recent: readonly StartRecent[];
  onNewNote(): void;
  onNewLatexProject(): void;
  onImportLatex(): void;
  onToday(): void;
  onOpenRecent(noteId: string): void;
}

/** At most this many recent notes are offered. */
export const START_RECENT_LIMIT = 5;

/**
 * Shown while no note is open. A note and a LaTeX project are offered side by
 * side, as equal ways to start, so the vault reads as a place for both.
 */
export function StartPanel(props: StartPanelProps) {
  const { translator: t } = props;
  const recent = props.recent.slice(0, START_RECENT_LIMIT);
  return (
    <div className="start-panel">
      <h2>{props.writable ? t('start.title') : props.vaultName}</h2>
      <p className="start-lede">{props.writable ? t('start.lede') : t('workspace.select')}</p>
      {props.writable ? (
        <div className="start-cards">
          <button type="button" className="start-card" disabled={props.newNoteDisabled} onClick={props.onNewNote}>
            <FileText size={22} aria-hidden="true" />
            <strong>{t('workspace.create.heading')}</strong>
            <span>{t('start.noteBody')}</span>
            {props.newNoteHint !== '' ? <kbd>{props.newNoteHint}</kbd> : null}
          </button>
          <div className="start-card start-card-latex">
            <button type="button" className="start-card-main" onClick={props.onNewLatexProject}>
              <span className="start-card-top">
                <Sigma size={22} aria-hidden="true" />
                <small className="start-tag">{t('start.livePdf')}</small>
              </span>
              <strong>{t('sidebar.newLatexProject')}</strong>
              <span>{t('start.latexBody')}</span>
            </button>
            <button type="button" className="start-card-link" onClick={props.onImportLatex}>{t('sidebar.importLatex')}</button>
          </div>
        </div>
      ) : null}
      {props.writable ? (
        <button type="button" className="start-today" disabled={props.todayDisabled} onClick={props.onToday}>
          <CalendarDays size={16} aria-hidden="true" />{t('daily.today')}
        </button>
      ) : null}
      {recent.length > 0 ? (
        <section className="start-recent" aria-labelledby="start-recent-heading">
          <h3 id="start-recent-heading">{t('start.recent')}</h3>
          <ul>
            {recent.map(entry => (
              <li key={entry.noteId}>
                <button type="button" disabled={!entry.available} title={entry.title} onClick={() => props.onOpenRecent(entry.noteId)}>
                  <FileText size={16} aria-hidden="true" />
                  <span>{entry.label}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
