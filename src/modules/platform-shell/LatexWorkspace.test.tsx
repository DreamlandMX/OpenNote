import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { LatexWorkspace } from './LatexWorkspace';
import { disconnectedLatexBuilds, disconnectedLatexService } from './tauri-latex-service';

it('shows the sources beside the PDF reader with build and auto-compile controls, in each language', () => {
  for (const locale of ['en', 'zh-CN', 'es'] as const) {
    const translator = new LocaleContext(locale).translator;
    const html = renderToStaticMarkup(<LatexWorkspace
      projects={disconnectedLatexService} builds={disconnectedLatexBuilds} vaultId="v1" projectId="p1"
      translator={translator} locale={locale} onOpenEnvironment={() => undefined} onClose={() => undefined}
    />);
    expect(html).toContain(translator('latexWs.build'));
    expect(html).toContain(translator('latexWs.files'));
    // The source pane is a code editor (mounted after render), with auto compile on by default.
    expect(html).toContain('class="latex-ws-editor"');
    expect(html).toMatch(/type="checkbox" checked=""/);
    expect(html).toContain(translator('latexWs.auto'));
    // The reader is mounted beside the source; empty until a PDF is published.
    expect(html).toContain(translator('pdf.none'));
  }
});
