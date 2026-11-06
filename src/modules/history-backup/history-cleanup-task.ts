import type { HistoryCleanupJob, ServiceError } from '../domain';
import type { NoteService, ServiceOutcome } from '../editor/ports';

export async function monitorHistoryCleanup(
  service: Pick<NoteService, 'historyCleanupStatus'>,
  initial: HistoryCleanupJob,
  onJob: (job: HistoryCleanupJob) => void,
  onError: (error: ServiceError) => void,
  pause: () => Promise<void> = () => new Promise(resolve => setTimeout(resolve, 600)),
): Promise<ServiceOutcome<HistoryCleanupJob>> {
  let job = initial;
  onJob(job);
  while (job.status === 'running') {
    await pause();
    const next = await service.historyCleanupStatus(job.jobId);
    if (!next.ok) {
      onError(next.error);
      if (next.error.code === 'INVALID_INPUT') return next;
      continue;
    }
    job = next.value;
    onJob(job);
  }
  return { ok: true, value: job };
}
