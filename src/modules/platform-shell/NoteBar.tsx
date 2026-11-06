import type { ReactNode } from 'react';
import { BookOpen, PenLine, Sigma, Star } from 'lucide-react';
import type { EditorMode } from '../editor';
import type { Translator } from '../presentation-i18n';
import { WorkspaceActions } from './WorkspaceActions';

export interface BarAction {
  key: string;
  label: string;
  onClick(): void;
  disabled?: boolean;
  /** A toggle's state; absent for plain actions. */
  pressed?: boolean;
  title?: string;
}

export interface NoteBarProps {
  translator: Translator;
  vaultName: string;
  relativePath: string;
  /** Save state, already rendered. */
  status: ReactNode;
  favorite: boolean;
  mode: EditorMode;
  busy: boolean;
  /** LaTeX tools for this note; the menu is left out when empty. */
  latexActions: readonly BarAction[];
  /** The note menu, in groups separated by a rule. */
  noteActions: readonly (readonly BarAction[])[];
  /** Extra controls placed in the note menu (the split buttons). */
  noteExtras?: ReactNode;
  onCrumb(folderPath: string): void;
  onToggleFavorite(): void;
  onMode(mode: EditorMode): void;
}

/** Folder crumbs of a vault path: each entry is a folder name and its full path. */
export function crumbsOf(relativePath: string): { name: string; path: string }[] {
  const parts = relativePath.split('/').slice(0, -1);
  return parts.map((name, index) => ({ name, path: parts.slice(0, index + 1).join('/') }));
}

function ActionButton({ action }: { action: BarAction }) {
  return (
    <button type="button" className="secondary" disabled={action.disabled} title={action.title}
      aria-pressed={action.pressed === undefined ? undefined : action.pressed ? 'true' : 'false'} onClick={action.onClick}>
      {action.label}
    </button>
  );
}

/** The bar above a note: where it lives, whether it is saved, and what can be done with it. */
export function NoteBar(props: NoteBarProps) {
  const { translator: t, mode } = props;
  const name = props.relativePath.split('/').pop()?.replace(/\.md$/i, '') ?? props.relativePath;
  const reading = mode === 'read';
  return (
    <div className="mode-row note-bar" role="group" aria-label={t('a11y.editor.modeGroup')}>
      <nav className="note-crumbs" aria-label={t('noteBar.location')} title={props.relativePath}>
        <span>{props.vaultName}</span>
        {crumbsOf(props.relativePath).map(crumb => (
          <span key={crumb.path} className="note-crumb">
            <span aria-hidden="true">/</span>
            <button type="button" onClick={() => props.onCrumb(crumb.path)}>{crumb.name}</button>
          </span>
        ))}
        <span aria-hidden="true">/</span>
        <span className="note-crumb-current">{name}</span>
      </nav>
      {props.status}
      <button type="button" className="mode-button bar-icon favorite-toggle" aria-pressed={props.favorite ? 'true' : 'false'}
        aria-label={t(props.favorite ? 'favorites.remove' : 'favorites.add')} title={t(props.favorite ? 'favorites.remove' : 'favorites.add')}
        disabled={props.busy} onClick={props.onToggleFavorite}>
        <Star size={15} aria-hidden="true" fill={props.favorite ? 'currentColor' : 'none'} />
      </button>
      {props.latexActions.length > 0 ? (
        <WorkspaceActions label={t('noteBar.latex')} icon={<Sigma size={16} aria-hidden="true" />} triggerClassName="bar-icon">
          <section className="workspace-action-group">
            {props.latexActions.map(action => <ActionButton key={action.key} action={action} />)}
          </section>
        </WorkspaceActions>
      ) : null}
      <button type="button" className="mode-button bar-icon" disabled={props.busy}
        aria-label={t(reading ? 'noteBar.edit' : 'noteBar.read')} title={t(reading ? 'noteBar.edit' : 'noteBar.read')}
        onClick={() => props.onMode(reading ? 'live' : 'read')}>
        {reading ? <PenLine size={16} aria-hidden="true" /> : <BookOpen size={16} aria-hidden="true" />}
      </button>
      <WorkspaceActions label={t('workspace.noteActions')} triggerClassName="bar-icon">
        {props.noteActions.map((group, index) => (
          <section className="workspace-action-group" key={index}>
            {group.map(action => <ActionButton key={action.key} action={action} />)}
          </section>
        ))}
        {props.noteExtras ? <section className="workspace-action-group">{props.noteExtras}</section> : null}
      </WorkspaceActions>
    </div>
  );
}
