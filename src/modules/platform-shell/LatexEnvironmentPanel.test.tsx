import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { LatexEnvironmentPanel } from './LatexEnvironmentPanel';
import { disconnectedLatexEnvironment } from './tauri-latex-service';

it('offers no download before the plan has been loaded and shown, in each language', () => {
  for (const locale of ['en', 'zh-CN', 'es'] as const) {
    const translator = new LocaleContext(locale).translator;
    const html = renderToStaticMarkup(<LatexEnvironmentPanel
      service={disconnectedLatexEnvironment} translator={translator} locale={locale} onClose={() => undefined}
    />);
    expect(html).toContain(translator('latexEnv.title'));
    expect(html).toContain(translator('latexEnv.loading'));
    // The agree button only appears next to the plan it agrees to.
    expect(html).not.toContain(translator('latexEnv.agree'));
  }
});
