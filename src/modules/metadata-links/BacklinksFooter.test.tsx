import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { BacklinksList } from './BacklinksFooter';
import { backlinkRows } from './view-model';

const translator = ((key: string, params?: Record<string, unknown>) => `${key}:${String(params?.count ?? '')}`) as never;
const noop = () => undefined;

it('lists each linking note once, collapsed, without the .md suffix', () => {
  const rows = backlinkRows({ noteId: 'n', unreadable: [], backlinks: [
    { fromNoteId: 'a', fromPath: 'Research/Intro.md', kind: 'link', label: null, anchor: null, offset: 3 },
    { fromNoteId: 'a', fromPath: 'Research/Intro.md', kind: 'link', label: null, anchor: 'x', offset: 9 },
    { fromNoteId: 'b', fromPath: 'Daily.md', kind: 'embed', label: null, anchor: null, offset: 0 },
  ] });
  const html = renderToStaticMarkup(<BacklinksList rows={rows} translator={translator} onOpenNote={noop} />);
  expect(html).toContain('<details class="backlinks-footer">');
  expect(html).not.toContain('open=""');
  expect(html).toContain('backlinks.footer:2');
  expect(html).toContain('>Research/Intro</button>');
  expect(html).toContain('>Daily</button>');
});

it('renders nothing when no note links here', () => {
  expect(renderToStaticMarkup(<BacklinksList rows={[]} translator={translator} onOpenNote={noop} />)).toBe('');
});
