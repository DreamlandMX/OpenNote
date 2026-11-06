import { expect, it, vi } from 'vitest';
import type { SearchJob, SearchPage, ServiceOutcome } from '../editor/ports';
import type { ServiceError } from '../domain';
import { runSearchTask } from './search-task';

const error: ServiceError = { code: 'IO_DENIED', messageKey: 'test', params: {}, retryable: true, operationId: null, retryAfterSeconds: null };
const page: SearchPage = { items: [], nextCursor: null, indexStatus: { state: 'ready', version: 1, indexedNotes: 2, unreadableNotes: 0 } };
const job: SearchJob = { jobId: 'job', status: 'running', phase: 'reading', processed: 1, total: 2, unreadable: 0, cancelRequested: false, page: null, error: null };
const request = { query: 'alpha', filters: { folders: [], tags: [], types: [] }, locale: 'en', limit: 50, cursor: null };
const flush = async () => { for (let i = 0; i < 12; i += 1) await Promise.resolve(); };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
function harness() {
  const start = deferred<ServiceOutcome<SearchJob>>();
  const polls: ReturnType<typeof deferred<ServiceOutcome<SearchJob>>>[] = [];
  const pauses: ReturnType<typeof deferred<void>>[] = [];
  const service = {
    startSearch: vi.fn(() => start.promise),
    searchJobStatus: vi.fn(() => { const poll = deferred<ServiceOutcome<SearchJob>>(); polls.push(poll); return poll.promise; }),
    cancelSearch: vi.fn(async (): Promise<ServiceOutcome<void>> => ({ ok: true, value: undefined })),
  };
  const onJob = vi.fn(); const onError = vi.fn();
  const task = runSearchTask(service, request, onJob, onError, () => { const pause = deferred<void>(); pauses.push(pause); return pause.promise; });
  return { start, polls, pauses, service, onJob, onError, task };
}

it('shows real progress and waits for terminal confirmation after a cancellation request', async () => {
  const h = harness(); h.start.resolve({ ok: true, value: job }); await flush();
  expect(h.onJob).toHaveBeenLastCalledWith(job);
  const requested = await h.task.cancel(); expect(requested.ok).toBe(true);
  expect(h.onJob).toHaveBeenCalledTimes(1);
  h.pauses[0]!.resolve(); await flush();
  const cancelling: SearchJob = { ...job, cancelRequested: true }; h.polls[0]!.resolve({ ok: true, value: cancelling }); await flush();
  expect(h.onJob).toHaveBeenLastCalledWith(cancelling);
  h.pauses[1]!.resolve(); await flush();
  h.polls[1]!.resolve({ ok: true, value: { ...cancelling, status: 'cancelled', error: { ...error, code: 'OPERATION_CANCELLED' } } });
  expect(await h.task.result).toMatchObject({ ok: false, error: { code: 'OPERATION_CANCELLED' } });
});

it('cancels a late start after stop without delivering progress or polling', async () => {
  const h = harness(); h.task.stop(); h.start.resolve({ ok: true, value: job });
  expect(await h.task.result).toBeNull(); await flush();
  expect(h.service.cancelSearch).toHaveBeenCalledWith('job'); expect(h.service.searchJobStatus).not.toHaveBeenCalled(); expect(h.onJob).not.toHaveBeenCalled();
});

it('suppresses a late status response after closing or changing a query', async () => {
  const h = harness(); h.start.resolve({ ok: true, value: job }); await flush(); h.pauses[0]!.resolve(); await flush(); h.task.stop();
  h.polls[0]!.resolve({ ok: true, value: { ...job, status: 'completed', page } });
  expect(await h.task.result).toBeNull(); expect(h.onJob).toHaveBeenCalledTimes(1);
});

it('keeps cancellation failures retryable and never invents a cancelled result', async () => {
  const h = harness(); h.start.resolve({ ok: true, value: job }); await flush();
  h.service.cancelSearch.mockResolvedValueOnce({ ok: false, error });
  expect(await h.task.cancel()).toEqual({ ok: false, error }); expect((await h.task.cancel()).ok).toBe(true);
  h.pauses[0]!.resolve(); await flush(); h.polls[0]!.resolve({ ok: true, value: { ...job, status: 'completed', page } });
  expect(await h.task.result).toEqual({ ok: true, value: page });
});

it('reports a monitoring failure and resumes serial observation without restarting the search', async () => {
  const h = harness(); h.start.resolve({ ok: true, value: job }); await flush(); h.pauses[0]!.resolve(); await flush();
  h.polls[0]!.resolve({ ok: false, error }); await flush(); expect(h.onError).toHaveBeenCalledWith(error);
  h.pauses[1]!.resolve(); await flush(); h.polls[1]!.resolve({ ok: true, value: { ...job, status: 'completed', page } });
  expect(await h.task.result).toEqual({ ok: true, value: page }); expect(h.service.startSearch).toHaveBeenCalledOnce();
});

it('does not turn malformed terminal success or a lost job into zero results', async () => {
  const h = harness(); h.start.resolve({ ok: true, value: { ...job, status: 'completed' } });
  expect(await h.task.result).toMatchObject({ ok: false, error: { code: 'RESCAN_REQUIRED' } });
  const lost = harness(); lost.start.resolve({ ok: true, value: job }); await flush(); lost.pauses[0]!.resolve(); await flush();
  lost.polls[0]!.resolve({ ok: false, error: { ...error, code: 'INVALID_INPUT' } });
  expect(await lost.task.result).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } }); expect(lost.pauses).toHaveLength(1);
});
