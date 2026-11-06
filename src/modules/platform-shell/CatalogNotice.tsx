import type { CatalogRefresh } from '../editor';
import type { ServiceError } from '../domain';
import { serviceErrorMessage, type Translator, type SupportedLocaleId, type MessageKey } from '../presentation-i18n';
const findingKeys: Readonly<Record<string, MessageKey>> = {
  CatalogMissingNote: 'catalog.finding.missing', CatalogIdentityChanged: 'catalog.finding.identity',
  CatalogLegacyMoveUnresolved: 'catalog.finding.legacy', DuplicateNoteId: 'catalog.finding.duplicate',
  DeletedNoteReappeared: 'catalog.finding.deleted',
};
export function CatalogNotice({ result, error, watchError, readFailures = [], translator: t, locale, onRetry }: {
  result: CatalogRefresh | null; error: ServiceError | null; watchError: ServiceError | null;
  readFailures?: readonly { path: string; error: ServiceError }[];
  translator: Translator; locale: SupportedLocaleId; onRetry(): void;
}) {
  const findings = result?.findings ?? [];
  if (!error && !watchError && !result?.deferred && findings.length === 0 && readFailures.length === 0) return null;
  return <section className="notice-panel catalog-notice" aria-live="polite">
    {watchError ? <p>{t('catalog.watchUnavailable')}</p> : null}
    {error ? <p>{t('catalog.scanFailed')} {serviceErrorMessage(locale, error.code)}</p> : null}
    {readFailures.length > 0 ? <ul>{readFailures.map((failure, index) => <li key={`${failure.path}:${index}`}><bdi dir="ltr">{failure.path}</bdi> — {serviceErrorMessage(locale, failure.error.code)}</li>)}</ul> : null}
    {result?.deferred ? <p>{t('catalog.deferred')}</p> : null}
    {findings.length > 0 ? <details><summary>{t('catalog.issues', { count: findings.length })}</summary>
      <ul>{findings.map((finding, index) => <li key={`${finding.code}:${finding.target}:${index}`}><bdi dir="ltr">{finding.target}</bdi> — {finding.code === 'CatalogPathCollision' ? serviceErrorMessage(locale, 'PATH_COLLISION') : t(findingKeys[finding.code] ?? 'catalog.finding.other')}</li>)}</ul>
    </details> : null}
    <button type="button" className="secondary" onClick={onRetry}>{t('catalog.retry')}</button>
  </section>;
}
