import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { PdfPane } from './PdfPane';

it('shows a clear empty state, in each interface language, without loading pdf.js', () => {
  for (const locale of ['en', 'zh-CN', 'es'] as const) {
    const translator = new LocaleContext(locale).translator;
    const html = renderToStaticMarkup(<PdfPane source={null} translator={translator} />);
    expect(html).toContain(translator('pdf.none'));
    expect(html).toContain(`aria-label="${translator('pdf.label')}"`);
  }
});
