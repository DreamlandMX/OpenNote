import type { NoteService, SearchJob, SearchPage, SearchRequest, ServiceOutcome } from '../editor/ports';
import type { ServiceError } from '../domain';
import { serviceErrorFromThrown } from './port';

export function runSearchTask(
  service: Pick<NoteService, 'startSearch' | 'searchJobStatus' | 'cancelSearch'>,
  request: SearchRequest,
  onJob: (job: SearchJob) => void,
  onMonitorError: (error: ServiceError) => void,
  pause: () => Promise<void> = () => new Promise(resolve => setTimeout(resolve, 100)),
): { result: Promise<ServiceOutcome<SearchPage> | null>; cancel(): Promise<ServiceOutcome<void>>; stop(): void } {
  let active = true;
  let finished = false;
  const call = async <T,>(run: () => Promise<ServiceOutcome<T>>): Promise<ServiceOutcome<T>> => {
    try { return await run(); } catch (error) { return { ok: false, error: serviceErrorFromThrown(error) }; }
  };
  const started = call(() => service.startSearch(request));
  const cancel = async (): Promise<ServiceOutcome<void>> => {
    const initial = await started;
    if (!initial.ok) return initial;
    return call(() => service.cancelSearch(initial.value.jobId));
  };
  const result = (async (): Promise<ServiceOutcome<SearchPage> | null> => {
    const initial = await started;
    if (!active) return null;
    if (!initial.ok) return initial;
    let job = initial.value;
    onJob(job);
    while (job.status === 'running') {
      await pause();
      if (!active) return null;
      const next = await call(() => service.searchJobStatus(job.jobId));
      if (!active) return null;
      if (!next.ok) {
        onMonitorError(next.error);
        if (next.error.code === 'INVALID_INPUT' || next.error.code === 'OBJECT_NOT_FOUND') return next;
        continue;
      }
      job = next.value;
      onJob(job);
    }
    if (job.status === 'completed' && job.page !== null) return { ok: true, value: job.page };
    return { ok: false, error: job.error ?? { code: 'RESCAN_REQUIRED', messageKey: 'error.searchIndexUnavailable', params: {}, retryable: true, operationId: null, retryAfterSeconds: null } };
  })().finally(() => { finished = true; });
  return { result, cancel, stop: () => { active = false; if (!finished) void cancel(); } };
}
