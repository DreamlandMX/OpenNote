import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import type { NoteSummary } from '../domain';
import { menuPosition, NoteTree, TreeMenu, type TreeNaming } from './NoteTree';

const notes = ['journal/day.md', 'top.md'].map((relativePath, i) => ({ noteId: String(i), relativePath } as NoteSummary));
const noop = () => undefined;
const tree = (naming: TreeNaming | null, namingError: string | null = null) => renderToStaticMarkup(<NoteTree
  notes={notes} folders={['empty', 'journal', 'journal/2026']} activeId={null} disabled={false}
  naming={naming} namingLabel="Folder name" namingError={namingError}
  onOpen={noop} onName={noop} onCancelName={noop} onMenu={noop} onDrop={noop}
/>);

it('shows empty folders, nested ones included, beside folders that hold notes', () => {
  const html = tree(null);
  for (const name of ['empty', 'journal', '2026', 'day', 'top']) expect(html).toContain(name);
  expect(html.indexOf('empty')).toBeLessThan(html.indexOf('top'));
  expect(html).not.toContain('aria-label="Folder name"');
});

it('names a new folder in place, in the folder it is made in', () => {
  const root = tree({ mode: 'create', parent: '' });
  expect(root.indexOf('aria-label="Folder name"')).toBeLessThan(root.indexOf('empty'));
  const nested = tree({ mode: 'create', parent: 'journal' }, 'Already here.');
  expect(nested.indexOf('aria-label="Folder name"')).toBeGreaterThan(nested.indexOf('journal'));
  expect(nested).toContain('role="alert">Already here.');
});

it('renames a folder in place, starting from its name, and lets notes and folders be dragged', () => {
  const html = tree({ mode: 'rename', path: 'journal/2026' });
  expect(html).toContain('value="2026"');
  expect(html).toContain('aria-label="Folder name"');
  expect(html).toContain('draggable="true"');
});

it('renders its own menu and keeps it inside the window', () => {
  const html = renderToStaticMarkup(<TreeMenu x={10} y={10} label="File actions" onClose={noop}
    items={[{ key: 'a', label: 'New note here', run: noop }, { key: 'b', label: 'Delete…', disabled: true, run: noop }]} />);
  expect(html).toContain('role="menu"');
  expect(html).toContain('>New note here</button>');
  expect(html).toMatch(/disabled="".*>Delete…/);
  expect(menuPosition(990, 760, 2, 1000, 800)).toEqual({ left: 796, top: 716 });
  expect(menuPosition(10, 10, 2, 1000, 800)).toEqual({ left: 10, top: 10 });
});

it('offers "+" on folders and "…" on every row only when row actions are given', () => {
  const html = renderToStaticMarkup(<NoteTree
    notes={notes} folders={['journal']} activeId={null} disabled={false}
    naming={null} namingLabel="Folder name" namingError={null}
    onOpen={noop} onName={noop} onCancelName={noop} onMenu={noop} onDrop={noop}
    rowActions={{ newNote: 'New note here', more: 'More actions', newNoteDisabled: false, onNewNote: noop }}
  />);
  expect(html.match(/aria-label="New note here"/g)).toHaveLength(1);
  // journal (folder), day and top (notes).
  expect(html.match(/aria-label="More actions"/g)).toHaveLength(3);
  expect(html).toContain('aria-haspopup="menu"');
  expect(tree(null)).not.toContain('tree-row-actions');
});
