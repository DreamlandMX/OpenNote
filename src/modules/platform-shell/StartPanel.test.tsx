import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { START_RECENT_LIMIT, StartPanel, type StartPanelProps } from './StartPanel';

const noop = () => undefined;

function render(overrides: Partial<StartPanelProps> = {}) {
  const translator = new LocaleContext('zh-CN').translator;
  const props: StartPanelProps = {
    translator, vaultName: 'Testvault', writable: true, newNoteHint: 'Ctrl+N', newNoteDisabled: false, todayDisabled: false,
    recent: [], onNewNote: noop, onNewLatexProject: noop, onImportLatex: noop, onToday: noop, onOpenRecent: noop,
    ...overrides,
  };
  return { html: renderToStaticMarkup(<StartPanel {...props} />), translator };
}

describe('start panel', () => {
  it('offers a note and a LaTeX project as equal ways to start', () => {
    const { html, translator } = render();
    expect(html).toContain(translator('start.title'));
    expect(html).toContain(`<strong>${translator('workspace.create.heading')}</strong>`);
    expect(html).toContain(`<strong>${translator('sidebar.newLatexProject')}</strong>`);
    expect(html).toContain(translator('start.livePdf'));
    expect(html).toContain(translator('sidebar.importLatex'));
    expect(html).toContain('<kbd>Ctrl+N</kbd>');
  });

  it('lists only a few recent notes and marks missing ones', () => {
    const recent = Array.from({ length: 8 }, (_, index) => ({ noteId: `n${index}`, label: `Note ${index}`, title: '', available: index !== 1 }));
    const { html, translator } = render({ recent });
    expect(html).toContain(translator('start.recent'));
    expect(html.match(/<span>Note \d<\/span>/g)).toHaveLength(START_RECENT_LIMIT);
    expect(html).toContain('disabled="" title=""><svg');
  });

  it('shows no create actions in a read-only vault', () => {
    const { html, translator } = render({ writable: false, newNoteHint: '' });
    expect(html).toContain('Testvault');
    expect(html).not.toContain(translator('sidebar.newLatexProject'));
    expect(html).not.toContain(translator('daily.today'));
  });
});
