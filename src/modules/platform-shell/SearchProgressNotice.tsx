import type { SearchJob } from '../editor/ports';
import type { Translator } from '../presentation-i18n';

export function SearchProgressNotice({ job, translator: t, cancelPending, cancelFailed, monitorFailed, onCancel }: {
  job: SearchJob | null; translator: Translator; cancelPending: boolean;
  cancelFailed: boolean; monitorFailed: boolean; onCancel(): void;
}) {
  if (job === null || job.status !== 'running') return null;
  const cancelling = cancelPending || job.cancelRequested;
  const phase = t(`search.phase.${job.phase}`);
  const count = t('search.progress', { completed: job.processed, total: job.total });
  const label = `${phase} · ${count}`;
  return <div className="search-progress">
    <p role="status">{cancelling ? t('search.cancelling') : <>{phase} · <bdi dir="ltr">{count}</bdi></>}</p>
    {job.total > 0 ? <progress aria-label={label} max={job.total} value={Math.min(job.processed, job.total)} /> : <progress aria-label={label} />}
    {job.unreadable > 0 ? <p>{t('search.indexIncomplete', { count: job.unreadable })}</p> : null}
    {cancelFailed ? <p role="alert">{t('search.cancelFailed')}</p> : null}
    {monitorFailed ? <p role="alert">{t('search.monitorFailed')}</p> : null}
    <button type="button" className="secondary" disabled={cancelling} onClick={onCancel}>{t('search.cancel')}</button>
  </div>;
}
