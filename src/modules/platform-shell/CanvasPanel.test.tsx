import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import type { NoteService } from '../editor';
import { LocaleContext } from '../presentation-i18n';
import { CanvasPanel } from './CanvasPanel';

it('starts as an unsaved canvas with save-as offered, in each interface language', () => {
  for (const locale of ['en', 'zh-CN', 'es'] as const) {
    const translator = new LocaleContext(locale).translator;
    const html = renderToStaticMarkup(<CanvasPanel
      service={{} as NoteService} translator={translator} locale={locale}
      knownNotes={[]} onClose={() => undefined}
    />);
    expect(html).toContain(translator('canvas.file.untitled'));
    expect(html).toContain(translator('canvas.file.state.unsaved'));
    expect(html).toContain(translator('canvas.file.saveAs'));
    // One toolbar replaces the old form rows; an empty canvas explains the three steps.
    expect(html).toContain(translator('canvas.purpose'));
    expect(html).toContain(translator('canvas.tool.text'));
    expect(html).toContain(translator('canvas.tool.connect'));
    expect(html).toContain(translator('canvas.step.connect'));
    expect(html).not.toContain(translator('canvas.connectFrom'));
  }
});
