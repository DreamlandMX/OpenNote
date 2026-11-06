import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import type { TagBatchResult } from '../editor';
import { LocaleContext } from '../presentation-i18n';
import { TagBatchStatus } from './TagBatchStatus';

it('renders partial and cancelled work with escaped paths and distinct pending/conflict states', () => {
  const result: TagBatchResult = {
    operationId: 'batch', oldTag: 'old', newTag: 'new', isMerge: false, completed: false, cancelled: true,
    items: [
      { noteId: 'saved', relativePath: '<saved>.md', result: { noteId: 'saved', localRevision: 2, contentHash: 'hash' }, error: null },
      { noteId: 'pending', relativePath: 'pending.md', result: null, error: null },
      { noteId: 'conflict', relativePath: 'conflict.md', result: null, error: { code: 'VERSION_CONFLICT', messageKey: 'conflict', params: {}, operationId: 'batch', retryable: false, retryAfterSeconds: null } },
    ],
  };
  const locale = new LocaleContext('en');
  const html = renderToStaticMarkup(createElement(TagBatchStatus, { result, translator: locale.translator, locale: 'en' }));
  expect(html).toContain('aria-live="polite"');
  expect(html).toContain('&lt;saved&gt;.md');
  expect(html).toContain(locale.translator('tags.writeProgress', { saved: 1, paused: 1, failed: 0, pending: 1 }));
  expect(html).toContain(locale.translator('tags.cancelled'));
  expect(html).toContain(locale.translator('tags.pending'));
});
