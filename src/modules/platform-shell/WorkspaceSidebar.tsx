import { useState, type ReactNode } from 'react';
import {
  CalendarDays, ChevronDown, ChevronRight, ChevronsUpDown, FileInput, FilePlus, FileText, FolderInput, FolderPlus,
  PanelLeftClose, PanelLeftOpen, Plus, Search as SearchIcon, Settings, Sigma, Star, Trash2, X, type LucideIcon,
} from 'lucide-react';
import type { Translator } from '../presentation-i18n';
import { WorkspaceActions } from './WorkspaceActions';

export interface SidebarTool {
  key: string;
  label: string;
  icon: LucideIcon;
  onClick(): void;
  disabled?: boolean;
}

export interface SidebarFavorite {
  noteId: string;
  label: string;
  title: string;
  available: boolean;
}

/** A LaTeX project, listed in its own sidebar section. */
export interface SidebarProject {
  projectId: string;
  /** The project folder's name, or the main file's for a project at the vault root. */
  name: string;
  /** Vault-relative folder, shown as the row's tooltip. */
  folder: string;
}

export interface WorkspaceSidebarProps {
  translator: Translator;
  vaultName: string;
  writable: boolean;
  /** Shortcut text shown beside the search row, if bound. */
  searchHint: string;
  newNoteHint: string;
  busy: boolean;
  newNoteDisabled: boolean;
  todayDisabled: boolean;
  trashDisabled: boolean;
  favorites: readonly SidebarFavorite[];
  /** The other days and the daily settings, shown from the calendar button. */
  dailyOptions: ReactNode;
  tools: readonly SidebarTool[];
  latexProjects: readonly SidebarProject[];
  onSwitchVault?: () => void;
  onSettings?: () => void;
  onCollapse(): void;
  onSearch(): void;
  onToday(): void;
  onNewNote(): void;
  onNewFolder(): void;
  onOpenFavorite(noteId: string): void;
  onRemoveFavorite(noteId: string): void;
  /** Importing a LaTeX folder and the LaTeX environment, listed under More. */
  onLatex(): void;
  onNewLatexProject(): void;
  onOpenProject(projectId: string): void;
  onTrash(): void;
  /** Called while the edge is dragged; `done` is true on release. */
  onResize?: (deltaX: number, done: boolean) => void;
  /** Notices and the note tree. */
  children: ReactNode;
}

/** The workspace sidebar: a short list of places, favorites, the notes, and the rest tucked under "More". */
export function WorkspaceSidebar(props: WorkspaceSidebarProps) {
  const { translator: t, favorites, tools } = props;
  const [favoritesOpen, setFavoritesOpen] = useState(true);
  const [toolsOpen, setToolsOpen] = useState(false);
  const [projectsOpen, setProjectsOpen] = useState(true);
  return (
    <aside className="notes-pane" aria-label={t('a11y.workspace.notesRegion')}>
      <div className="sidebar-top">
        {props.onSwitchVault ? (
          <button type="button" className="sidebar-vault" title={t('sidebar.switchVault')} onClick={props.onSwitchVault}>
            <span className="vault-avatar" aria-hidden="true">{initialOf(props.vaultName)}</span>
            <strong>{props.vaultName}</strong>
            <ChevronsUpDown size={14} aria-hidden="true" />
          </button>
        ) : <span className="sidebar-vault"><span className="vault-avatar" aria-hidden="true">{initialOf(props.vaultName)}</span><strong>{props.vaultName}</strong></span>}
        <button type="button" className="sidebar-icon" aria-label={t('sidebar.collapse')} title={t('sidebar.collapse')} onClick={props.onCollapse}>
          <PanelLeftClose size={16} aria-hidden="true" />
        </button>
      </div>
      {/* Says up front that a vault holds both kinds of writing. */}
      <p className="sidebar-kinds">{t('sidebar.kinds')}</p>
      {props.writable ? (
        <WorkspaceActions label={t('sidebar.new')} align="start" triggerClassName="sidebar-new"
          icon={<><Plus size={16} aria-hidden="true" /><span>{t('sidebar.new')}</span><ChevronDown size={14} aria-hidden="true" /></>}>
          <button type="button" className="secondary menu-row" disabled={props.newNoteDisabled} onClick={props.onNewNote}>
            <FileText size={16} aria-hidden="true" />{t('workspace.create.heading')}
          </button>
          <button type="button" className="secondary menu-row" onClick={props.onNewLatexProject}>
            <Sigma size={16} aria-hidden="true" />{t('sidebar.newLatexProject')}
          </button>
          <button type="button" className="secondary menu-row" onClick={props.onNewFolder}>
            <FolderPlus size={16} aria-hidden="true" />{t('workspace.tree.newFolder')}
          </button>
        </WorkspaceActions>
      ) : null}
      <div className="sidebar-rows">
        <button type="button" className="sidebar-row" onClick={props.onSearch}>
          <SearchIcon size={16} aria-hidden="true" />
          <span>{t('sidebar.search')}</span>
          {props.searchHint !== '' ? <kbd>{props.searchHint}</kbd> : null}
        </button>
        {props.writable ? (
          <div className="sidebar-row-group">
            <button type="button" className="sidebar-row" disabled={props.todayDisabled} onClick={props.onToday}>
              <CalendarDays size={16} aria-hidden="true" />
              <span>{t('daily.today')}</span>
            </button>
            <WorkspaceActions label={t('sidebar.otherDays')} align="start" triggerClassName="sidebar-icon"
              icon={<ChevronRight size={14} aria-hidden="true" />}>
              {props.dailyOptions}
            </WorkspaceActions>
          </div>
        ) : null}
      </div>
      {/* Only this middle part scrolls: the vault, search and the bottom row stay put at any window size. */}
      <div className="sidebar-scroll">
      {favorites.length > 0 ? (
        <section className="sidebar-section" aria-labelledby="favorites-heading">
          <h2 id="favorites-heading">
            <button type="button" className="sidebar-heading" aria-expanded={favoritesOpen ? 'true' : 'false'} onClick={() => setFavoritesOpen(open => !open)}>
              <ChevronRight className="sidebar-chevron" size={12} aria-hidden="true" />
              {t('favorites.title')}
            </button>
          </h2>
          {favoritesOpen ? (
            <ul className="sidebar-favorites">
              {favorites.map(entry => (
                <li key={entry.noteId}>
                  <button type="button" className="sidebar-row" disabled={!entry.available || props.busy}
                    title={entry.available ? entry.title : t('favorites.unavailable')} onClick={() => props.onOpenFavorite(entry.noteId)}>
                    <Star size={15} aria-hidden="true" />
                    <span>{entry.label}</span>
                    {entry.available ? null : <small>{t('favorites.unavailable')}</small>}
                  </button>
                  <button type="button" className="sidebar-icon sidebar-hover" aria-label={t('favorites.remove')} title={t('favorites.remove')}
                    onClick={() => props.onRemoveFavorite(entry.noteId)}>
                    <X size={14} aria-hidden="true" />
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
        </section>
      ) : null}
      <section className="sidebar-section sidebar-projects" aria-labelledby="projects-heading">
        <div className="sidebar-section-head">
          <h2 id="projects-heading">
            <button type="button" className="sidebar-heading" aria-expanded={projectsOpen ? 'true' : 'false'} onClick={() => setProjectsOpen(open => !open)}>
              <ChevronRight className="sidebar-chevron" size={12} aria-hidden="true" />
              {t('sidebar.latexProjects')}
            </button>
          </h2>
          {props.writable ? (
            <button type="button" className="sidebar-icon sidebar-hover" aria-label={t('sidebar.newLatexProject')} title={t('sidebar.newLatexProject')}
              onClick={props.onNewLatexProject}>
              <Plus size={14} aria-hidden="true" />
            </button>
          ) : null}
        </div>
        {projectsOpen ? (
          <ul className="sidebar-favorites">
            {props.latexProjects.map(project => (
              <li key={project.projectId}>
                <button type="button" className="sidebar-row" title={project.folder || project.name} disabled={props.busy}
                  onClick={() => props.onOpenProject(project.projectId)}>
                  <Sigma size={15} aria-hidden="true" />
                  <span>{project.name}</span>
                </button>
              </li>
            ))}
            {props.writable && props.latexProjects.length === 0 ? (
              <li>
                <button type="button" className="sidebar-row sidebar-row-muted" onClick={props.onNewLatexProject}>
                  <Plus size={15} aria-hidden="true" />
                  <span>{t('sidebar.newLatexProject')}</span>
                </button>
              </li>
            ) : null}
            <li>
              <button type="button" className="sidebar-row sidebar-row-muted" onClick={props.onLatex}>
                <FileInput size={15} aria-hidden="true" />
                <span>{t('sidebar.importLatex')}</span>
              </button>
            </li>
          </ul>
        ) : null}
      </section>
      <div className="notes-head">
        <h2>{t('workspace.notes.heading')}</h2>
        {props.writable ? (
          <div className="notes-head-actions">
            <button type="button" className="secondary icon-button" disabled={props.newNoteDisabled}
              aria-label={t('workspace.tree.newNote')} title={[t('workspace.tree.newNote'), props.newNoteHint].filter(Boolean).join(' ')}
              onClick={props.onNewNote}>
              <FilePlus size={16} aria-hidden="true" />
            </button>
            <button type="button" className="secondary icon-button" aria-label={t('workspace.tree.newFolder')} title={t('workspace.tree.newFolder')}
              onClick={props.onNewFolder}>
              <FolderPlus size={16} aria-hidden="true" />
            </button>
          </div>
        ) : null}
      </div>
      {props.children}
      </div>
      <div className="sidebar-footer">
        <button type="button" className="sidebar-row" aria-expanded={toolsOpen ? 'true' : 'false'} onClick={() => setToolsOpen(open => !open)}>
          <ChevronRight className="sidebar-chevron" size={16} aria-hidden="true" />
          <span>{t('sidebar.moreTools')}</span>
        </button>
        {toolsOpen ? (
          <ul className="sidebar-tools">
            {[...tools, { key: 'latex', label: t('sidebar.latexManage'), icon: FolderInput, onClick: props.onLatex }].map(tool => (
              <li key={tool.key}>
                <button type="button" className="sidebar-row" disabled={tool.disabled} onClick={tool.onClick}>
                  <tool.icon size={16} aria-hidden="true" />
                  <span>{tool.label}</span>
                </button>
              </li>
            ))}
          </ul>
        ) : null}
        <div className="sidebar-row-pair">
          <button type="button" className="sidebar-row" disabled={props.trashDisabled} onClick={props.onTrash}>
            <Trash2 size={16} aria-hidden="true" />
            <span>{t('trash.title')}</span>
          </button>
          {props.onSettings ? (
            <button type="button" className="sidebar-row" onClick={props.onSettings}>
              <Settings size={16} aria-hidden="true" />
              <span>{t('shell.nav.settings')}</span>
            </button>
          ) : null}
        </div>
      </div>
      {props.onResize ? <ResizeEdge label={t('sidebar.resize')} onResize={props.onResize} /> : null}
    </aside>
  );
}

/** The first letter of a vault name, for its little square. */
export function initialOf(name: string): string {
  const first = Array.from(name.trim())[0];
  return first === undefined ? '?' : first.toLocaleUpperCase();
}

/** A thin handle on the sidebar's inner edge; arrow keys move it too. */
function ResizeEdge({ label, onResize }: { label: string; onResize(deltaX: number, done: boolean): void }) {
  const flip = () => (document.dir === 'rtl' ? -1 : 1);
  return (
    <div className="sidebar-resizer" role="separator" aria-orientation="vertical" aria-label={label} title={label} tabIndex={0}
      onKeyDown={event => {
        if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
        event.preventDefault();
        onResize((event.key === 'ArrowRight' ? 16 : -16) * flip(), true);
      }}
      onPointerDown={event => {
        if (event.button !== 0) return;
        event.preventDefault();
        const handle = event.currentTarget;
        const startX = event.clientX;
        handle.setPointerCapture(event.pointerId);
        const move = (next: PointerEvent) => onResize((next.clientX - startX) * flip(), false);
        const up = (next: PointerEvent) => {
          handle.removeEventListener('pointermove', move);
          handle.removeEventListener('pointerup', up);
          handle.removeEventListener('pointercancel', up);
          onResize((next.clientX - startX) * flip(), true);
        };
        handle.addEventListener('pointermove', move);
        handle.addEventListener('pointerup', up);
        handle.addEventListener('pointercancel', up);
      }} />
  );
}

/** The collapsed sidebar: one button that brings it back. */
export function SidebarRail({ translator: t, onExpand }: { translator: Translator; onExpand(): void }) {
  return (
    <aside className="sidebar-rail" aria-label={t('a11y.workspace.notesRegion')}>
      <button type="button" className="sidebar-icon" aria-label={t('sidebar.expand')} title={t('sidebar.expand')} onClick={onExpand}>
        <PanelLeftOpen size={16} aria-hidden="true" />
      </button>
    </aside>
  );
}
