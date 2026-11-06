import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { OutlinePanel } from './OutlinePanel';

const t = new LocaleContext('en').translator;
const render = (pending: boolean, failed: boolean) => renderToStaticMarkup(createElement(OutlinePanel, {
  rows: [], activeId: null, collapsedIds: [], translator: t, pending, failed,
  onToggle: () => {}, onJump: () => {}, onClose: () => {},
}));

it('announces loading rather than claiming that an unparsed note has no headings', () => {
  const html = render(true, false);
  expect(html).toContain('aria-busy="true"');
  expect(html).toContain(t('workspace.loading'));
  expect(html).not.toContain(t('outline.empty'));
});

it('does not label failed parsing as a successful empty outline', () => {
  expect(render(false, true)).not.toContain(t('outline.empty'));
  expect(render(false, false)).toContain(t('outline.empty'));
});
