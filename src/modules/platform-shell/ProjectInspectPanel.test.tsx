import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import type { NoteService } from '../editor';
import { LocaleContext } from '../presentation-i18n';
import { ProjectInspectPanel } from './ProjectInspectPanel';
import { ProjectWizardPanel } from './ProjectWizardPanel';
import { disconnectedLatexEnvironment, disconnectedLatexService } from './tauri-latex-service';

it('offers a folder picker instead of a typed root and pasted listing, in each language', () => {
  for (const locale of ['en', 'zh-CN', 'es'] as const) {
    const translator = new LocaleContext(locale).translator;
    const html = renderToStaticMarkup(<ProjectInspectPanel
      latexService={disconnectedLatexService} environment={disconnectedLatexEnvironment} vaultId="v1" writable
      translator={translator} locale={locale} onOpenProject={() => undefined}
      onOpenEnvironment={() => undefined} onCreateProject={() => undefined} onClose={() => undefined}
    />);
    // The environment is a badge in the head; then a new project, an existing folder, and the list.
    const order = [translator('project.badge.checking'), translator('project.card.new'), translator('project.card.open'), translator('project.mine')];
    const at = order.map((text) => html.indexOf(text));
    expect(at.every((index) => index >= 0)).toBe(true);
    expect([...at].sort((a, b) => a - b)).toEqual(at);
    expect(html).toContain(translator('latexEnv.title'));
    expect(html).toContain(translator('project.safety'));
    expect(html).not.toContain('<textarea');
    expect(html).not.toContain('placeholder="projects/thesis"');
  }
});

it('the wizard is one step: pick a kind, type a title, create; the rest waits under more options', () => {
  const translator = new LocaleContext('zh-CN').translator;
  const html = renderToStaticMarkup(<ProjectWizardPanel
    service={{} as NoteService} latexService={disconnectedLatexService} vaultId="v1" writable
    translator={translator} locale="zh-CN" onOpenProject={() => undefined} onClose={() => undefined}
  />);
  for (const id of ['report', 'paper', 'assignment'] as const) {
    expect(html).toContain(translator(`wizard.template.${id}.hint`));
  }
  expect(html).toContain(`placeholder="${translator('wizard.titlePlaceholder')}"`);
  // No title yet: the button waits and says why.
  expect(html).toMatch(new RegExp(`disabled=""[^>]*>.*?${translator('wizard.create')}`));
  expect(html).toContain(translator('wizard.needTitle'));
  // Author, date, language, paper and folder are folded away.
  expect(html).toContain(`<details class="wizard-more">`);
  expect(html).toContain(translator('wizard.moreOptions'));
});
