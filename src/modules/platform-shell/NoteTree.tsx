import { useState, type DragEvent, type MouseEvent } from 'react';
import type { NoteSummary } from '../domain';
import { ChevronRight, FileCode, FileText, Folder, MoreHorizontal, Plus, Sigma } from 'lucide-react';

/**
 * The notes and folders directly under `prefix`. `folders` adds folders that
 * hold no notes, so an empty folder still has a row.
 */
export function groupNotes(notes: readonly NoteSummary[], prefix = '', folders: readonly string[] = []) {
  const grouped = new Map<string, NoteSummary[]>();
  const files: NoteSummary[] = [];
  for (const folder of folders) {
    if (!folder.startsWith(prefix)) continue;
    const name = folder.slice(prefix.length).split('/')[0];
    if (name && !grouped.has(name)) grouped.set(name, []);
  }
  for (const note of notes) {
    const suffix = note.relativePath.slice(prefix.length);
    const slash = suffix.indexOf('/');
    if (slash < 0) files.push(note);
    else {
      const folder = suffix.slice(0, slash);
      const group = grouped.get(folder) ?? [];
      group.push(note);
      grouped.set(folder, group);
    }
  }
  return { folders: [...grouped].sort(([a], [b]) => a.localeCompare(b)), files: files.sort((a, b) => a.relativePath.localeCompare(b.relativePath)) };
}

/** What a right-click or drag starts from; the root folder is `''`. */
export type TreeTarget = { kind: 'folder'; path: string } | { kind: 'note'; noteId: string; path: string };

/** A folder being named in place: a new one in `parent`, or `path` renamed. */
export type TreeNaming = { mode: 'create'; parent: string } | { mode: 'rename'; path: string };

/** A LaTeX project living in a folder of the tree. */
export interface TreeProject {
  projectId: string;
  /** The main file's name, shown as the row that opens the project. */
  entry: string;
}

const DRAG_TYPE = 'application/x-opennote-tree';

/** A stable key for a row, used by selection and by the rows' `data-tree-key`. */
export function treeKey(target: TreeTarget): string {
  return target.kind === 'note' ? `note:${target.noteId}` : `folder:${target.path}`;
}

/** Ctrl/Cmd adds or removes one row; Shift takes the range from the last one. */
function selectMode(event: MouseEvent): 'toggle' | 'range' | null {
  if (event.shiftKey) return 'range';
  if (event.ctrlKey || event.metaKey) return 'toggle';
  return null;
}

interface Props {
  notes: readonly NoteSummary[];
  /** Every folder path, empty ones included. */
  folders: readonly string[];
  prefix?: string;
  activeId: string | null;
  disabled: boolean;
  naming: TreeNaming | null;
  namingLabel: string;
  namingError: string | null;
  onOpen(noteId: string): void;
  /** A name typed for `naming`: the new folder's name, or the renamed one's. */
  onName(name: string): void;
  onCancelName(): void;
  onMenu(target: TreeTarget, x: number, y: number): void;
  /** A note or folder dropped on a folder (`''` is the root). */
  onDrop(source: TreeTarget, folder: string): void;
  /** LaTeX projects by folder path (`''` is the vault root). */
  projects?: ReadonlyMap<string, TreeProject>;
  onOpenProject?: (projectId: string) => void;
  /** Keys of the selected rows (see `treeKey`). */
  selected?: ReadonlySet<string>;
  onSelect?: (target: TreeTarget, mode: 'toggle' | 'range') => void;
  /**
   * Row buttons shown on hover or keyboard focus, as in Notion: "+" makes a
   * note in that folder, "…" opens the same menu as a right-click.
   */
  rowActions?: { newNote: string; more: string; newNoteDisabled: boolean; onNewNote(folder: string): void };
}

/** The hover buttons of one row. They never fold a folder or open a note themselves. */
function RowActions({ target, actions, disabled, onMenu }: { target: TreeTarget; actions: NonNullable<Props['rowActions']>; disabled: boolean; onMenu: Props['onMenu'] }) {
  const stop = (event: MouseEvent) => { event.preventDefault(); event.stopPropagation(); };
  return <span className="tree-row-actions">
    {target.kind === 'folder' ? <button type="button" className="tree-row-action" aria-label={actions.newNote} title={actions.newNote}
      disabled={disabled || actions.newNoteDisabled} onClick={(event) => { stop(event); actions.onNewNote(target.path); }}>
      <Plus size={14} aria-hidden="true" />
    </button> : null}
    <button type="button" className="tree-row-action" aria-label={actions.more} title={actions.more} aria-haspopup="menu" disabled={disabled}
      onClick={(event) => {
        stop(event);
        const rect = event.currentTarget.getBoundingClientRect();
        onMenu(target, rect.left, rect.bottom + 2);
      }}>
      <MoreHorizontal size={14} aria-hidden="true" />
    </button>
  </span>;
}

function dragFrom(target: TreeTarget) {
  return (event: DragEvent) => {
    event.stopPropagation();
    event.dataTransfer.setData(DRAG_TYPE, JSON.stringify(target));
    event.dataTransfer.effectAllowed = 'move';
  };
}

/** Handlers that make an element a drop target for `folder`, marked while hovered. */
function dropOn(folder: string, onDrop: Props['onDrop']) {
  const mark = (event: DragEvent, on: boolean) => {
    if (on) (event.currentTarget as HTMLElement).dataset.drop = 'true';
    else delete (event.currentTarget as HTMLElement).dataset.drop;
  };
  return {
    onDragOver: (event: DragEvent) => {
      if (!event.dataTransfer.types.includes(DRAG_TYPE)) return;
      event.preventDefault();
      event.stopPropagation();
      event.dataTransfer.dropEffect = 'move';
      mark(event, true);
    },
    onDragLeave: (event: DragEvent) => mark(event, false),
    onDrop: (event: DragEvent) => {
      mark(event, false);
      const raw = event.dataTransfer.getData(DRAG_TYPE);
      if (raw === '') return;
      event.preventDefault();
      event.stopPropagation();
      onDrop(JSON.parse(raw) as TreeTarget, folder);
    },
  };
}

export function NoteTree(props: Props) {
  const { notes, folders, prefix = '', activeId, disabled, naming, onOpen, onMenu, onDrop } = props;
  const grouped = groupNotes(notes, prefix, folders);
  const parent = prefix.replace(/\/$/, '');
  const menu = (target: TreeTarget) => (event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    onMenu(target, event.clientX, event.clientY);
  };
  const root = prefix === '';
  const here = props.projects?.get(parent);
  // The root list fills the sidebar, so a right-click or drop below the last row still lands on the root.
  return <ul className={root ? 'note-list file-tree file-tree-root' : 'note-list file-tree'}
    onContextMenu={root ? menu({ kind: 'folder', path: '' }) : undefined}
    {...(root ? dropOn('', onDrop) : {})}>
    {naming?.mode === 'create' && naming.parent === parent ? <li><NameRow {...props} initial="" /></li> : null}
    {/* A project folder starts with its main file, which opens the LaTeX workspace. */}
    {here !== undefined ? <li><button type="button" className="note-item project-entry" disabled={disabled} title={here.entry}
      onClick={() => props.onOpenProject?.(here.projectId)} onContextMenu={menu({ kind: 'folder', path: parent })}>
      <FileCode size={16} aria-hidden="true" /><span>{here.entry}</span>
    </button></li> : null}
    {grouped.folders.map(([name, children]) => {
      const path = `${prefix}${name}`;
      if (naming?.mode === 'rename' && naming.path === path) return <li key={`folder:${name}`}><NameRow {...props} initial={name} /></li>;
      const folderTarget: TreeTarget = { kind: 'folder', path };
      return <li key={`folder:${name}`}><details open>
        <summary data-path={path} data-tree-key={treeKey(folderTarget)} data-selected={props.selected?.has(treeKey(folderTarget)) ? 'true' : undefined}
          onClick={(event) => {
            const mode = selectMode(event);
            if (mode === null || props.onSelect === undefined) return;
            // A modified click selects; it does not fold the folder.
            event.preventDefault();
            props.onSelect(folderTarget, mode);
          }}
          draggable={!disabled} onDragStart={dragFrom({ kind: 'folder', path })} {...dropOn(path, onDrop)} onContextMenu={menu({ kind: 'folder', path })}>
          <ChevronRight className="tree-chevron" size={14} aria-hidden="true" />
          {props.projects?.has(path) ? <Sigma className="tree-project" size={14} aria-hidden="true" /> : <Folder size={14} aria-hidden="true" />}<span className="tree-folder-name">{name}</span>
          {props.rowActions ? <RowActions target={folderTarget} actions={props.rowActions} disabled={disabled} onMenu={onMenu} /> : null}
        </summary>
        <NoteTree {...props} notes={children} prefix={`${path}/`} />
      </details></li>;
    })}
    {grouped.files.map(note => {
      const target: TreeTarget = { kind: 'note', noteId: note.noteId, path: note.relativePath };
      return <li key={note.noteId} className="tree-row"><button type="button" className={`note-item${note.noteId === activeId ? ' active' : ''}`} aria-current={note.noteId === activeId ? 'page' : undefined} disabled={disabled}
        draggable={!disabled} onDragStart={dragFrom(target)}
        data-tree-key={treeKey(target)} data-selected={props.selected?.has(treeKey(target)) ? 'true' : undefined}
        onClick={(event) => {
          const mode = selectMode(event);
          if (mode !== null && props.onSelect !== undefined) props.onSelect(target, mode);
          else onOpen(note.noteId);
        }} onContextMenu={menu(target)} title={note.relativePath}>
        <FileText size={16} aria-hidden="true" /><span>{note.relativePath.slice(prefix.length).replace(/\.md$/i, '')}</span>
      </button>{props.rowActions ? <RowActions target={target} actions={props.rowActions} disabled={disabled} onMenu={onMenu} /> : null}</li>;
    })}
  </ul>;
}

/** Names a folder in place: Enter applies, Escape or leaving it unchanged cancels. */
function NameRow({ initial, namingLabel, namingError, onName, onCancelName }: Props & { initial: string }) {
  const [name, setName] = useState(initial);
  const submit = () => (name.trim() === '' || name.trim() === initial ? onCancelName() : onName(name.trim()));
  return <div className="tree-name-row">
    <Folder size={14} aria-hidden="true" />
    <input
      autoFocus
      type="text"
      value={name}
      aria-label={namingLabel}
      aria-invalid={namingError !== null || undefined}
      onFocus={(event) => event.currentTarget.select()}
      onChange={(event) => setName(event.target.value)}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === 'Enter') { event.preventDefault(); submit(); }
        else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onCancelName(); }
      }}
      onBlur={() => { if (name.trim() === '' || name.trim() === initial) onCancelName(); }}
    />
    {namingError !== null ? <p role="alert">{namingError}</p> : null}
  </div>;
}

export interface TreeMenuItem { key: string; label: string; disabled?: boolean; run(): void }

/** Keeps the menu inside the window when opened near its right or bottom edge. */
export function menuPosition(x: number, y: number, count: number, width = globalThis.innerWidth ?? 1024, height = globalThis.innerHeight ?? 768) {
  const menuWidth = 200;
  const menuHeight = count * 32 + 16;
  return { left: Math.max(4, Math.min(x, width - menuWidth - 4)), top: Math.max(4, Math.min(y, height - menuHeight - 4)) };
}

/** The file tree's own right-click menu, in place of the browser's. */
export function TreeMenu({ x, y, label, items, onClose }: { x: number; y: number; label: string; items: readonly TreeMenuItem[]; onClose(): void }) {
  return <>
    <div className="tree-menu-backdrop" onClick={onClose} onContextMenu={(event) => { event.preventDefault(); onClose(); }} />
    <div className="tree-menu" role="menu" aria-label={label} style={menuPosition(x, y, items.length)}
      onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); onClose(); } }}>
      {items.map((item, index) => <button key={item.key} type="button" role="menuitem" autoFocus={index === 0} disabled={item.disabled}
        onClick={() => { onClose(); item.run(); }}>{item.label}</button>)}
    </div>
  </>;
}
