import { describe, expect, it, vi } from 'vitest';
import type { HistoryCleanupJob, ServiceError } from '../domain';
import { monitorHistoryCleanup } from './history-cleanup-task';

const initial: HistoryCleanupJob = {
  jobId: 'job', operationId: 'operation', status: 'running',
  progress: { removed: 0, total: 2 }, cancelRequested: false, result: null, error: null,
};
const error = (code: ServiceError['code']): ServiceError => ({
  code, messageKey: 'error', params: {}, retryable: false, operationId: null, retryAfterSeconds: null,
});

describe('history cleanup monitoring', () => {
  it('keeps an unknown task running after a transient query failure and waits for the cancellation result', async () => {
    const cancelled: HistoryCleanupJob = { ...initial, status: 'cancelled', cancelRequested: true,
      progress: { removed: 1, total: 2 }, result: { operationId: 'operation', status: 'cancelled', removed: [] } };
    const historyCleanupStatus = vi.fn()
      .mockResolvedValueOnce({ ok: false, error: error('IO_DENIED') })
      .mockResolvedValueOnce({ ok: true, value: { ...initial, cancelRequested: true } })
      .mockResolvedValueOnce({ ok: true, value: cancelled });
    const seen: string[] = [];
    const onError = vi.fn();
    expect(await monitorHistoryCleanup({ historyCleanupStatus }, initial, job => seen.push(job.status), onError, async () => {}))
      .toEqual({ ok: true, value: cancelled });
    expect(seen).toEqual(['running', 'running', 'cancelled']);
    expect(onError).toHaveBeenCalledTimes(1);
    expect(historyCleanupStatus).toHaveBeenCalledTimes(3);
  });

  it('does not invent a successful result when the captured job is no longer available', async () => {
    const failure = { ok: false as const, error: error('INVALID_INPUT') };
    const service = { historyCleanupStatus: vi.fn().mockResolvedValue(failure) };
    const seen = vi.fn();
    expect(await monitorHistoryCleanup(service, initial, seen, vi.fn(), async () => {})).toEqual(failure);
    expect(seen).toHaveBeenCalledTimes(1);
    expect(service.historyCleanupStatus).toHaveBeenCalledWith('job');
  });
});
