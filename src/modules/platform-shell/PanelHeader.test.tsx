import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Network } from 'lucide-react';
import type { NoteService } from '../editor';
import { LocaleContext } from '../presentation-i18n';
import { PanelHeader, PanelSteps } from './PanelHeader';
import { TemplatePanel, templateExample } from './TemplatePanel';
import { GraphPanel } from './GraphPanel';

const translator = new LocaleContext('zh-CN').translator;
const pending = () => new Promise<never>(() => undefined);

describe('panel header', () => {
  it('names the panel, says what it is for, and offers a labelled close', () => {
    const html = renderToStaticMarkup(<PanelHeader id="h" icon={Network} title="Graph" purpose="See links" closeLabel="Close graph"
      status={<span>Ready</span>} onClose={() => undefined} />);
    expect(html).toContain('<h2 id="h">Graph</h2>');
    expect(html).toContain('<p class="panel-purpose">See links</p>');
    expect(html).toContain('aria-label="Close graph"');
    expect(html).toContain('<span>Ready</span>');
  });

  it('numbers the how-to steps', () => {
    const html = renderToStaticMarkup(<PanelSteps steps={['one', 'two']} />);
    expect(html.match(/panel-step-number/g)).toHaveLength(2);
    expect(html).toContain('<span>two</span>');
  });
});

describe('panel purposes', () => {
  it('every tool panel opens with one line on what it is for', () => {
    const service = { listTemplates: pending, graphReport: pending } as unknown as NoteService;
    const template = renderToStaticMarkup(<TemplatePanel service={service} translator={translator} locale="zh-CN" region="zh-CN"
      suggestedTitle="" writable canInsert onInsert={() => true} onCreateNote={() => undefined} onClose={() => undefined} />);
    expect(template).toContain(translator('templates.purpose'));
    const graph = renderToStaticMarkup(<GraphPanel service={service} translator={translator} locale="zh-CN" focusNoteId="n"
      onOpenNote={() => undefined} onClose={() => undefined} />);
    expect(graph).toContain(translator('graph.purpose'));
    expect(graph).toContain(translator('graph.layers', { count: 1 }));
  });
});

describe('template example', () => {
  it('is itself a template with every variable, kept unexpanded', () => {
    const text = templateExample(translator);
    expect(text.startsWith('---\ntemplate: true\n---\n')).toBe(true);
    for (const name of ['{{title}}', '{{date}}', '{{cursor}}']) expect(text).toContain(name);
  });
});
