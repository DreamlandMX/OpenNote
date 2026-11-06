import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { CatalogNotice } from './CatalogNotice';
import type { CatalogRefresh } from '../editor';
it('shows deferred recovery and escaped per-path findings instead of a false empty directory', () => {
  const result: CatalogRefresh = { notes: [], changes: [], unavailableNoteIds: [], fingerprint: 'pending', deferred: true, persisted: false, writable: true,
    findings: [{ code: 'CatalogIdentityChanged', target: '<foreign>.md', messageKey: 'unused' }] };
  const t = new LocaleContext('en').translator;
  const html = renderToStaticMarkup(createElement(CatalogNotice, { result, error: null, watchError: null, translator: t, locale: 'en', onRetry: () => undefined }));
  expect(html).toContain(t('catalog.deferred')); expect(html).toContain('&lt;foreign&gt;.md'); expect(html).toContain(t('catalog.finding.identity'));
  expect(html).toContain('dir="ltr"'); expect(html).toContain(t('catalog.retry'));
});
