import { expect, it, vi } from 'vitest';
import type { NoteService, SourceWatchEvent, SourceWatchStop, ServiceOutcome } from '../editor/ports';
import type { ServiceError } from '../domain';
import { observeSources } from './observe-sources';

const failure: ServiceError = { code: 'IO_DENIED', messageKey: 'error.sourceWatchUnavailable', params: {}, retryable: true, operationId: null, retryAfterSeconds: null };
const flush = async () => { await Promise.resolve(); await Promise.resolve(); };
function harness() {
  let deliver!: (event: SourceWatchEvent) => void;
  let finish!: (outcome: ServiceOutcome<SourceWatchStop>) => void;
  const detached = vi.fn(async () => undefined);
  const registration = new Promise<ServiceOutcome<SourceWatchStop>>(resolve => { finish = resolve; });
  const service: Pick<NoteService, 'watchSourceChanges'> = { watchSourceChanges: vi.fn((_ids, listener) => { deliver = listener; return registration; }) };
  const callbacks = { ready: vi.fn(), changed: vi.fn(), failed: vi.fn() };
  return { service, callbacks, detached, deliver: (event: SourceWatchEvent) => deliver(event), finish: () => finish({ ok: true, value: detached }), fail: () => finish({ ok: false, error: failure }) };
}

it('refreshes once after registration and reacts only to selected source identities', async () => {
  const h = harness(); const stop = observeSources(h.service, ['source'], h.callbacks);
  h.finish(); await flush(); expect(h.callbacks.ready).toHaveBeenCalledOnce();
  h.deliver({ noteIds: ['unrelated'], error: null }); expect(h.callbacks.changed).not.toHaveBeenCalled();
  h.deliver({ noteIds: ['source'], error: null }); expect(h.callbacks.changed).toHaveBeenCalledOnce();
  stop(); h.deliver({ noteIds: ['source'], error: null }); expect(h.callbacks.changed).toHaveBeenCalledOnce();
  expect(h.detached).toHaveBeenCalledOnce();
});
it('detaches a late registration after the reading note or vault has already closed', async () => {
  const h = harness(); const stop = observeSources(h.service, ['source'], h.callbacks); stop();
  h.finish(); await flush();
  expect(h.detached).toHaveBeenCalledOnce(); expect(h.callbacks.ready).not.toHaveBeenCalled();
  h.deliver({ noteIds: ['source'], error: failure }); expect(h.callbacks.failed).not.toHaveBeenCalled();
});
it('shows startup and runtime failures instead of claiming an active observer', async () => {
  const h = harness(); observeSources(h.service, ['source'], h.callbacks); h.fail(); await flush();
  expect(h.callbacks.failed).toHaveBeenCalledWith(failure); expect(h.callbacks.ready).not.toHaveBeenCalled();
  const live = harness(); observeSources(live.service, ['source'], live.callbacks); live.finish(); await flush();
  live.deliver({ noteIds: [], error: failure }); expect(live.callbacks.failed).toHaveBeenCalledWith(failure);
});
