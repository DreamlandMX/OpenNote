import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Network } from 'lucide-react';
import { LocaleContext } from '../presentation-i18n';
import { SidebarRail, WorkspaceSidebar, type WorkspaceSidebarProps } from './WorkspaceSidebar';
import { NoteBar, crumbsOf, type NoteBarProps } from './NoteBar';

const noop = () => undefined;

function sidebar(overrides: Partial<WorkspaceSidebarProps> = {}) {
  const translator = new LocaleContext('zh-CN').translator;
  const props: WorkspaceSidebarProps = {
    translator, vaultName: 'Testvault', writable: true, searchHint: 'Ctrl+P', newNoteHint: 'Ctrl+N',
    busy: false, newNoteDisabled: false, todayDisabled: false, trashDisabled: false,
    favorites: [], dailyOptions: <button type="button">day options</button>,
    tools: [{ key: 'graph', label: 'Graph', icon: Network, onClick: noop }],
    onSwitchVault: noop, onSettings: noop, onCollapse: noop, onSearch: noop, onToday: noop,
    onNewNote: noop, onNewFolder: noop, onOpenFavorite: noop, onRemoveFavorite: noop, onLatex: noop, onTrash: noop,
    latexProjects: [], onNewLatexProject: noop, onOpenProject: noop,
    children: <ul className="file-tree"><li>tree</li></ul>,
    ...overrides,
  };
  return { html: renderToStaticMarkup(<WorkspaceSidebar {...props} />), translator };
}

describe('workspace sidebar', () => {
  it('lists the places, the notes, and the trash and settings rows', () => {
    const { html, translator } = sidebar();
    expect(html).toContain('<strong>Testvault</strong>');
    expect(html).toContain(`>${translator('sidebar.search')}</span><kbd>Ctrl+P</kbd>`);
    expect(html).toContain(`>${translator('daily.today')}</span>`);
    expect(html).toContain('day options');
    expect(html).toContain('<li>tree</li>');
    expect(html).toContain(`>${translator('trash.title')}</span>`);
    expect(html).toContain(`>${translator('shell.nav.settings')}</span>`);
    // Favorites only appear once there are some.
    expect(html).not.toContain('favorites-heading');
  });

  it('keeps the other tools folded until asked for', () => {
    const { html, translator } = sidebar();
    expect(html).toContain(`aria-expanded="false"`);
    expect(html).toContain(`>${translator('sidebar.moreTools')}</span>`);
    expect(html).not.toContain('>Graph<');
  });

  it('shows favorites and marks the ones that are gone', () => {
    const { html, translator } = sidebar({ favorites: [
      { noteId: 'a', label: 'Alpha', title: 'a.md', available: true },
      { noteId: 'b', label: 'Beta', title: '', available: false },
    ] });
    expect(html).toContain('favorites-heading');
    expect(html).toContain('<span>Alpha</span>');
    expect(html).toContain(`<small>${translator('favorites.unavailable')}</small>`);
  });

  it('lists LaTeX projects in their own section beside the notes', () => {
    const { html, translator } = sidebar({ latexProjects: [
      { projectId: 'p1', name: 'Thesis', folder: 'Papers/Thesis' },
    ] });
    expect(html).toContain(`>${translator('sidebar.kinds')}</p>`);
    expect(html).toContain('projects-heading');
    expect(html).toContain('<span>Thesis</span>');
    expect(html).toContain('title="Papers/Thesis"');
    expect(html).toContain(`<span>${translator('sidebar.importLatex')}</span>`);
    // With a project listed, the empty-section shortcut row is gone.
    expect(html).not.toContain(`<span>${translator('sidebar.newLatexProject')}</span>`);
  });

  it('offers a new LaTeX project from the New menu and an empty section', () => {
    const { html, translator } = sidebar();
    expect(html).toContain(`<span>${translator('sidebar.new')}</span>`);
    expect(html).toContain(`<span>${translator('sidebar.newLatexProject')}</span>`);
  });

  it('hides the writing actions in a read-only vault', () => {
    const { html, translator } = sidebar({ writable: false });
    expect(html).not.toContain(translator('daily.today'));
    expect(html).not.toContain(`aria-label="${translator('workspace.tree.newFolder')}"`);
    expect(html).not.toContain(`aria-label="${translator('sidebar.new')}"`);
    expect(html).not.toContain(translator('sidebar.newLatexProject'));
  });

  it('folds to a rail with one button back', () => {
    const translator = new LocaleContext('en').translator;
    const html = renderToStaticMarkup(<SidebarRail translator={translator} onExpand={noop} />);
    expect(html).toContain(`aria-label="${translator('sidebar.expand')}"`);
  });
});

describe('note bar', () => {
  it('splits a path into folder crumbs', () => {
    expect(crumbsOf('a/b/note.md')).toEqual([{ name: 'a', path: 'a' }, { name: 'b', path: 'a/b' }]);
    expect(crumbsOf('note.md')).toEqual([]);
  });

  function bar(overrides: Partial<NoteBarProps> = {}) {
    const translator = new LocaleContext('en').translator;
    const props: NoteBarProps = {
      translator, vaultName: 'Testvault', relativePath: 'papers/Draft.md', status: <span>saved</span>,
      favorite: false, mode: 'live', busy: false,
      latexActions: [{ key: 'insert', label: 'Insert math', onClick: noop }],
      noteActions: [[{ key: 'rename', label: 'Rename', onClick: noop, disabled: true }], [{ key: 'outline', label: 'Outline', onClick: noop, pressed: true }]],
      onCrumb: noop, onToggleFavorite: noop, onMode: noop,
      ...overrides,
    };
    return { html: renderToStaticMarkup(<NoteBar {...props} />), translator };
  }

  it('shows where the note lives, its state, the LaTeX menu and grouped note actions', () => {
    const { html, translator } = bar();
    expect(html).toContain('<span>Testvault</span>');
    expect(html).toContain('>papers</button>');
    expect(html).toContain('<span class="note-crumb-current">Draft</span>');
    expect(html).toContain('<span>saved</span>');
    expect(html).toContain(`aria-label="${translator('noteBar.latex')}"`);
    expect(html).toContain('>Insert math</button>');
    expect(html).toContain('disabled="">Rename</button>');
    expect(html).toContain('aria-pressed="true">Outline</button>');
    expect(html.match(/workspace-action-group/g)).toHaveLength(3);
  });

  it('offers reading while editing and editing while reading', () => {
    expect(bar().html).toContain(`aria-label="${bar().translator('noteBar.read')}"`);
    const reading = bar({ mode: 'read' });
    expect(reading.html).toContain(`aria-label="${reading.translator('noteBar.edit')}"`);
  });

  it('leaves out the LaTeX menu when there is nothing in it', () => {
    const { html, translator } = bar({ latexActions: [] });
    expect(html).not.toContain(`aria-label="${translator('noteBar.latex')}"`);
  });
});

describe('redesign details', () => {
  it('marks the vault with its first letter and offers a resize edge', async () => {
    const { initialOf } = await import('./WorkspaceSidebar');
    expect(initialOf('testing')).toBe('T');
    expect(initialOf('  学习笔记')).toBe('学');
    expect(initialOf('')).toBe('?');
    const { html, translator } = sidebar({ onResize: noop });
    expect(html).toContain('<span class="vault-avatar" aria-hidden="true">T</span>');
    expect(html).toContain(`role="separator" aria-orientation="vertical" aria-label="${translator('sidebar.resize')}"`);
    expect(sidebar().html).not.toContain('role="separator"');
  });

  it('keeps a dragged sidebar width in range', async () => {
    const { clampSidebarWidth, SIDEBAR_WIDTH_MAX, SIDEBAR_WIDTH_MIN } = await import('./device-prefs');
    expect(clampSidebarWidth(50)).toBe(SIDEBAR_WIDTH_MIN);
    expect(clampSidebarWidth(9999)).toBe(SIDEBAR_WIDTH_MAX);
    expect(clampSidebarWidth(300.4)).toBe(300);
    expect(clampSidebarWidth(Number.NaN)).toBe(SIDEBAR_WIDTH_MIN);
  });

  it('shows success messages in a polite corner stack with a close button each', async () => {
    const { ToastStack } = await import('./Toast');
    const html = renderToStaticMarkup(<ToastStack closeLabel="Dismiss" items={[{ key: 'a', text: 'Daily note created.', onDismiss: noop }]} />);
    expect(html).toContain('class="toast-stack" role="status" aria-live="polite"');
    expect(html).toContain('<span>Daily note created.</span>');
    expect(html).toContain('aria-label="Dismiss"');
    expect(renderToStaticMarkup(<ToastStack closeLabel="Dismiss" items={[]} />)).not.toContain('class="toast"');
  });
});

describe('LaTeX projects in the tree', () => {
  it('marks a project folder and starts it with its main file, which opens the project', async () => {
    const { NoteTree } = await import('./NoteTree');
    const opened: string[] = [];
    const notes = [{ noteId: 'n1', relativePath: 'thesis/notes.md' }, { noteId: 'n2', relativePath: 'plain/a.md' }] as never[];
    const html = renderToStaticMarkup(<NoteTree notes={notes} folders={['thesis', 'thesis/sections', 'plain']} activeId={null} disabled={false}
      naming={null} namingLabel="" namingError={null} onOpen={noop} onName={noop} onCancelName={noop} onMenu={noop} onDrop={noop}
      projects={new Map([['thesis', { projectId: 'p1', entry: 'main.tex' }]])} onOpenProject={(id) => opened.push(id)} />);
    expect(html.match(/tree-project/g)).toHaveLength(1);
    expect(html).toContain('class="note-item project-entry"');
    expect(html).toContain('<span>main.tex</span>');
    // The entry comes before the project's subfolders and notes.
    expect(html.indexOf('main.tex')).toBeLessThan(html.indexOf('sections'));
  });

  it('keeps importing LaTeX folders under More', () => {
    const { html } = sidebar();
    expect(html).not.toContain('sidebar-latex');
  });
});

describe('tree selection', () => {
  it('marks picked rows and gives every row a selection key', async () => {
    const { NoteTree, treeKey } = await import('./NoteTree');
    const notes = [{ noteId: 'n1', relativePath: 'docs/a.md' }, { noteId: 'n2', relativePath: 'b.md' }] as never[];
    const html = renderToStaticMarkup(<NoteTree notes={notes} folders={['docs']} activeId={null} disabled={false}
      naming={null} namingLabel="" namingError={null} onOpen={noop} onName={noop} onCancelName={noop} onMenu={noop} onDrop={noop}
      selected={new Set([treeKey({ kind: 'folder', path: 'docs' }), treeKey({ kind: 'note', noteId: 'n2', path: 'b.md' })])} onSelect={noop} />);
    expect(html).toContain('data-tree-key="folder:docs" data-selected="true"');
    expect(html).toContain('data-tree-key="note:n2" data-selected="true"');
    expect(html).toContain('data-tree-key="note:n1"');
    expect(html).not.toContain('data-tree-key="note:n1" data-selected');
  });
});
