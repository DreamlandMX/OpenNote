import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import type { SearchJob } from '../editor';
import { LocaleContext } from '../presentation-i18n';
import { SearchProgressNotice } from './SearchProgressNotice';
const job: SearchJob = { jobId: 'job', status: 'running', phase: 'indexing', processed: 2, total: 8, unreadable: 1, cancelRequested: false, page: null, error: null };
const t = new LocaleContext('en').translator;
const render = (value: SearchJob, cancelFailed = false) => renderToStaticMarkup(createElement(SearchProgressNotice, { job: value, translator: t, cancelPending: false, cancelFailed, monitorFailed: false, onCancel: () => undefined }));
it('names cache loading and saving while retaining actual running counts and cancellation', () => {
  for (const [phase, label] of [['loadingIndex', 'Reading saved index'], ['savingIndex', 'Saving search index']] as const) {
    const html = render({ ...job, phase });
    expect(html).toContain(label);
    expect(html).toContain('<bdi dir="ltr">2 / 8</bdi>');
    expect(html).not.toContain('disabled=""');
    expect(html).not.toContain(t('search.cancelled'));
  }
});
it('shows phase counts, incomplete sources and an accessible progress control', () => {
  const html = render(job); expect(html).toContain(t('search.phase.indexing')); expect(html).toContain('<bdi dir="ltr">2 / 8</bdi>'); expect(html).toContain('max="8" value="2"'); expect(html).toContain(t('search.indexIncomplete', { count: 1 })); expect(html).toContain('aria-label=');
});
it('distinguishes a cancellation request from terminal confirmation and exposes failed cancellation retry', () => {
  const html = render({ ...job, cancelRequested: true }); expect(html).toContain(t('search.cancelling')); expect(html).toContain('disabled=""'); expect(html).not.toContain(t('search.cancelled'));
  const failed = render(job, true); expect(failed).toContain(t('search.cancelFailed')); expect(failed).not.toContain('disabled=""');
  expect(render({ ...job, status: 'cancelled' })).toBe('');
});
