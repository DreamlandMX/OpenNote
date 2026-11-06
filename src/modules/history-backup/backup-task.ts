import type { BackupJob, BackupService } from './ports';
import type { ServiceOutcome } from '../editor/ports';
import type { ServiceError } from '../domain';

export async function monitorBackupTask(
  service: BackupService, initial: BackupJob, onJob: (job: BackupJob) => void,
  onError: (error: ServiceError) => void,
  pause: () => Promise<void> = () => new Promise(resolve => setTimeout(resolve, 600)),
): Promise<ServiceOutcome<BackupJob>> {
  let job = initial;
  onJob(job);
  while (job.status === 'running') {
    await pause();
    const next = await service.status(job.jobId);
    if (!next.ok) {
      onError(next.error);
      const current = await service.context();
      if (current.ok && current.value.job?.jobId !== job.jobId) return next;
      if (current.ok && current.value.job?.status !== 'running' && current.value.job) { job = current.value.job; onJob(job); }
      if (!current.ok && current.error.code === 'INVALID_INPUT') return current;
      continue;
    }
    job = next.value; onJob(job);
  }
  return { ok: true, value: job };
}
