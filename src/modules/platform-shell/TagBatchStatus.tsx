import type { TagBatchResult } from '../editor';
import type { SupportedLocaleId, Translator } from '../presentation-i18n';
import { serviceErrorMessage } from '../presentation-i18n';

export function TagBatchStatus({ result, translator: t, locale }: { result: TagBatchResult; translator: Translator; locale: SupportedLocaleId }) {
  const saved = result.items.filter(item => item.result !== null).length;
  const paused = result.items.filter(item => item.error?.code === 'VERSION_CONFLICT').length;
  const failed = result.items.filter(item => item.error !== null && item.error.code !== 'VERSION_CONFLICT').length;
  const pending = result.items.length - saved - paused - failed;
  return <section className="tag-rename-section" aria-live="polite">
    <p role="status">{t('tags.writeProgress', { saved, paused, failed, pending })}</p>
    {result.cancelled ? <p>{t('tags.cancelled')}</p> : null}
    <ul>{result.items.map(item => <li key={item.noteId} className="tag-rename-row">
      <span className="tag-rename-path">{item.relativePath}</span>
      <span>{item.result !== null ? t('tags.saved') : item.error !== null ? serviceErrorMessage(locale, item.error.code) : t('tags.pending')}</span>
    </li>)}</ul>
  </section>;
}
