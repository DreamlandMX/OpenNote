import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import type { NoteService } from '../editor';
import { LocaleContext } from '../presentation-i18n';
import { AttachmentPanel } from './AttachmentPanel';

it('offers the native picker and no insertion before anything is saved', () => {
  const translator = new LocaleContext('en').translator;
  const html = renderToStaticMarkup(<AttachmentPanel
    service={{} as NoteService} translator={translator} locale="en"
    canInsert notePath="a.md" onOpenPdf={() => undefined} onInsert={() => true} onClose={() => undefined}
  />);
  expect(html).toContain(translator('attachment.choose'));
  expect(html).toContain(translator('attachment.body'));
  expect(html).not.toContain(`>${translator('attachment.insert')}</button>`);
});

it('has the three interface languages for every new string', () => {
  for (const locale of ['en', 'zh-CN', 'es'] as const) {
    const translator = new LocaleContext(locale).translator;
    for (const key of ['attachment.choose', 'attachment.result.stored', 'attachment.reason.occupied'] as const) {
      expect(translator(key).length).toBeGreaterThan(0);
    }
  }
  expect(new LocaleContext('zh-CN').translator('attachment.choose')).toBe('选择文件…');
});
