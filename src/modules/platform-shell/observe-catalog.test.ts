import { expect, it, vi } from 'vitest';
import type { CatalogRefresh, NoteService, ServiceOutcome, SourceWatchStop } from '../editor/ports';
import type { ServiceError } from '../domain';
import { observeCatalog } from './observe-catalog';
const value: CatalogRefresh = { notes: [], changes: [], unavailableNoteIds: [], findings: [], fingerprint: 'empty', deferred: false, persisted: true, writable: true };
const error: ServiceError = { code: 'IO_DENIED', messageKey: 'error.io', params: {}, retryable: true, operationId: null, retryAfterSeconds: null };
const flush = async () => { for (let i = 0; i < 8; i += 1) await Promise.resolve(); };
function harness() {
  let event!: (error: ServiceError | null) => void;
  let register!: (result: ServiceOutcome<SourceWatchStop>) => void;
  const requests: ((result: ServiceOutcome<CatalogRefresh>) => void)[] = [];
  const stopped = vi.fn(async () => undefined);
  const service: Pick<NoteService, 'watchCatalogChanges' | 'refreshCatalog'> = {
    watchCatalogChanges: vi.fn(callback => { event = callback; return new Promise<ServiceOutcome<SourceWatchStop>>(resolve => { register = resolve; }); }),
    refreshCatalog: vi.fn(() => new Promise<ServiceOutcome<CatalogRefresh>>(resolve => { requests.push(resolve); })),
  };
  const callbacks = { refreshed: vi.fn(), watching: vi.fn(), watchFailed: vi.fn(), scanFailed: vi.fn() };
  const observer = observeCatalog(service, callbacks);
  return { observer, service, callbacks, stopped, requests, event: (value: ServiceError | null) => event(value), register: (ok = true) => register(ok ? { ok: true, value: stopped } : { ok: false, error }) };
}
it('serializes scans and coalesces a notification burst into one follow-up scan', async () => {
  const h = harness(); h.register(); await flush(); expect(h.requests).toHaveLength(1);
  for (let i = 0; i < 12; i += 1) h.event(null);
  expect(h.requests).toHaveLength(1); h.requests[0]!({ ok: true, value }); await flush();
  expect(h.requests).toHaveLength(2); h.requests[1]!({ ok: true, value }); await flush();
  expect(h.requests).toHaveLength(2); expect(h.callbacks.refreshed).toHaveBeenCalledTimes(2); h.observer.stop();
});
it('suppresses a completed scan and detaches a late registration after stop', async () => {
  const h = harness(); void h.observer.refresh(); h.observer.stop(); h.register(); h.requests[0]!({ ok: true, value }); await flush();
  expect(h.callbacks.refreshed).not.toHaveBeenCalled(); expect(h.callbacks.watching).not.toHaveBeenCalled(); expect(h.stopped).toHaveBeenCalledOnce();
});
it('keeps manual refresh available after native monitoring fails', async () => {
  const h = harness(); h.register(false); await flush(); expect(h.callbacks.watchFailed).toHaveBeenCalledWith(error);
  const done = h.observer.refresh(); h.requests[0]!({ ok: true, value }); await done;
  expect(h.callbacks.refreshed).toHaveBeenCalledWith(value); expect(h.callbacks.watching).not.toHaveBeenCalled(); h.observer.stop();
});
it('reports a failed scan without replacing a previous result and accepts a later retry', async () => {
  const h = harness(); h.register(); await flush(); h.event(null); h.requests[0]!({ ok: false, error }); await flush();
  expect(h.callbacks.scanFailed).toHaveBeenCalledWith(error); expect(h.callbacks.refreshed).not.toHaveBeenCalled();
  h.event(null); await flush(); expect(h.requests).toHaveLength(1);
  const retried = h.observer.refresh(); h.requests[1]!({ ok: true, value }); await retried; expect(h.callbacks.refreshed).toHaveBeenCalledWith(value);
  h.event(null); await flush(); expect(h.requests).toHaveLength(3); h.requests[2]!({ ok: true, value }); await flush(); h.observer.stop();
});
