import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  createTauriShellPort,
  disconnectedShellPort,
  resolveShellPort,
  type ShellPort,
} from './port';
import { createTauriNoteService, resolveNoteService, resolveBackupService } from './tauri-note-service';
import canvasWire from '../../../crates/onote-tasks/tests/fixtures/canvas-wire.json?raw';
import { newTextNode, reassociateNode } from './canvas-view';
import type {
  CanvasDocument,
  DiagnosticsPreview,
  DiagnosticsRequest,
  NoteService,
} from '../editor/ports';
import type {
  HistoryRevisionContent,
  NoteDocument,
  NoteSummary,
  SaveResult,
  ServiceError,
  Vault,
  VaultFinding,
} from '../domain';

type InvokeArgs = Record<string, unknown>;
type CloseHandler = (event: { event: string; payload: unknown }) => void;

const invokeMock = vi.hoisted(() =>
  vi.fn<(command: string, args?: InvokeArgs) => Promise<unknown>>(),
);
const listenMock = vi.hoisted(() =>
  vi.fn<(event: string, handler: CloseHandler) => Promise<() => Promise<void>>>(),
);

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: listenMock,
}));

type InternalsHost = typeof globalThis & { __TAURI_INTERNALS__?: unknown };
const internalsHost: InternalsHost = globalThis;

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function awaitMicrotasks(times = 4): Promise<void> {
  let running: Promise<void> = Promise.resolve();
  for (let index = 0; index < times; index += 1) {
    running = running.then(() => Promise.resolve());
  }
  return running.then(
    () => undefined,
    () => undefined,
  );
}

function nativePort(): ShellPort {
  const port = createTauriShellPort();
  if (port === null) {
    throw new Error('native shell port was not detected');
  }
  return port;
}

function nativeNoteService(): NoteService {
  const service = createTauriNoteService('vault-1');
  if (service === null) {
    throw new Error('native note adapter was not detected');
  }
  return service;
}

function latestCloseHandler(): CloseHandler {
  const calls = listenMock.mock.calls;
  const handler = calls[calls.length - 1]?.[1];
  if (typeof handler !== 'function') {
    throw new Error('close-requested listener was not registered');
  }
  return handler;
}

const vault: Vault = {
  vaultId: 'vault-1',
  schemaVersion: 1,
  displayName: 'demo',
  syncEnabled: false,
  writable: true,
  findings: [] as VaultFinding[],
};

const genericUnknownFailure: ServiceError = {
  code: 'IO_DENIED',
  messageKey: 'opennote.service.error',
  params: {},
  retryable: false,
  operationId: null,
  retryAfterSeconds: null,
};

beforeEach(() => {
  internalsHost.__TAURI_INTERNALS__ = {};
});

afterEach(() => {
  delete internalsHost.__TAURI_INTERNALS__;
  vi.resetAllMocks();
});

describe('canvas wire contract over mocked IPC', () => {
  it.each([null, 'vault-1'])('retains the Rust document shape for scope %s', async (vaultId) => {
    const service = resolveNoteService(vaultId);
    const parsed: CanvasDocument = JSON.parse(canvasWire);
    invokeMock.mockResolvedValueOnce(parsed);
    const outcome = await service.parseCanvas(canvasWire);
    expect(outcome).toEqual({ ok: true, value: parsed });
    if (!outcome.ok) throw new Error('parse failed');
    const repaired = reassociateNode(outcome.value, 'n', 'exists');
    const edited: CanvasDocument = { ...repaired, nodes: [...repaired.nodes, newTextNode(repaired, 'new', '你好')] };
    invokeMock.mockResolvedValueOnce(JSON.stringify(edited));
    await expect(service.serializeCanvas(edited)).resolves.toEqual({ ok: true, value: JSON.stringify(edited) });
    expect(invokeMock.mock.calls).toEqual([
      ['parse_canvas_source', { source: canvasWire }],
      ['serialize_canvas_document', { document: edited }],
    ]);
    expect(edited.nodes[0]).toMatchObject({ type: 'note', kind: 'future-extension', noteId: 'exists' });
    expect(edited.nodes[1]).toMatchObject({ type: 'text', text: '你好' });
    expect(edited.nodes[1]).not.toHaveProperty('kind');
    expect(edited.future).toEqual(parsed.future);
    expect(edited.edges).toEqual(parsed.edges);
  });
});

describe('native shell port over mocked IPC', () => {
  it('passes sample opt-in only to create_vault', async () => {
    invokeMock.mockResolvedValueOnce(vault);

    const outcome = await nativePort().createVault({
      dialogTitle: '创建保险库',
      sampleOptIn: true,
    });

    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith('create_vault', {
      dialogTitle: '创建保险库',
      sampleOptIn: true,
    });
    expect(outcome).toEqual({ ok: true, value: vault });
  });

  it('invokes open_vault bare and represents null picker cancellation as a null success', async () => {
    invokeMock.mockResolvedValueOnce(null);

    const outcome = await nativePort().openVault({ sampleOptIn: true });

    expect(invokeMock).toHaveBeenCalledWith('open_vault', {});
    expect(outcome).toEqual({ ok: true, value: null });
  });

  it('preserves structured ServiceError fields from a typed native failure', async () => {
    const typed: ServiceError = {
      code: 'FILE_LOCKED',
      messageKey: 'error.vaultWriterLocked',
      params: { source: 'writer.lock' },
      retryable: true,
      operationId: 'op-42',
      retryAfterSeconds: 3,
    };
    invokeMock.mockRejectedValueOnce(typed);

    const outcome = await nativePort().createVault();

    expect(outcome).toEqual({ ok: false, error: typed });
  });

  it('sanitizes unknown invoke failures into a generic IO_DENIED error without leaking details', async () => {
    invokeMock.mockRejectedValueOnce(
      new Error('native failure at C:\\Users\\alice\\vault\\state.sqlite'),
    );

    const outcome = await nativePort().openVault();

    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.error).toEqual(genericUnknownFailure);
      expect(JSON.stringify(outcome.error)).not.toContain('state.sqlite');
    }
  });

  it('treats a throw with an untyped error code as a generic failure, never as NOT_IMPLEMENTED', async () => {
    invokeMock.mockRejectedValueOnce({
      code: 'GARBAGE_CODE',
      messageKey: 'leaked.key',
      params: { x: '1' },
      retryable: true,
      operationId: 'op',
      retryAfterSeconds: 9,
    });

    const outcome = await nativePort().createVault();

    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.error).toEqual(genericUnknownFailure);
      expect(JSON.stringify(outcome.error)).not.toContain('GARBAGE_CODE');
    }
  });

  it('auto-detects the native port when nothing is injected', () => {
    expect(resolveShellPort(undefined).kind).toBe('tauri');
    expect(resolveShellPort(null).kind).toBe('tauri');
    expect(resolveShellPort(disconnectedShellPort)).toBe(disconnectedShellPort);
  });

  it('involves no IPC in a non-native environment and stays disconnected', () => {
    delete internalsHost.__TAURI_INTERNALS__;

    expect(createTauriShellPort()).toBeNull();
    expect(resolveShellPort(undefined)).toBe(disconnectedShellPort);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(listenMock).not.toHaveBeenCalled();
  });

  it('authorizes the native close through confirm_close', async () => {
    invokeMock.mockResolvedValueOnce(undefined);

    const outcome = await nativePort().confirmClose();

    expect(invokeMock).toHaveBeenCalledWith('confirm_close', {});
    expect(outcome).toEqual({ ok: true, value: undefined });
  });

  describe('close subscription', () => {
    it('dispatches close-requested events to the listener and unlistens idempotently', async () => {
      const unlisten = vi.fn<() => Promise<void>>(async () => undefined);
      listenMock.mockImplementation((_event, _handler) => {
        return Promise.resolve(unlisten);
      });

      const listener = vi.fn<() => void>();
      const unlistenOnce = nativePort().onCloseRequested(listener);

      expect(listenMock).toHaveBeenCalledWith(
        'opennote://close-requested',
        expect.any(Function),
      );
      latestCloseHandler()({ event: 'opennote://close-requested', payload: null });
      await awaitMicrotasks();
      expect(listener).toHaveBeenCalledTimes(1);

      await expect(unlistenOnce()).resolves.toBeUndefined();
      await expect(unlistenOnce()).resolves.toBeUndefined();
      expect(unlisten).toHaveBeenCalledTimes(1);
    });

    it('unsubscribes before the registration resolves and cleans the async listener exactly once', async () => {
      const registration = deferred<() => Promise<void>>();
      const unlisten = vi.fn<() => Promise<void>>(async () => undefined);
      listenMock.mockReturnValue(registration.promise);

      const unlistenOnce = nativePort().onCloseRequested(vi.fn<() => void>());
      const settled = unlistenOnce();
      await awaitMicrotasks();
      expect(unlisten).not.toHaveBeenCalled();
      registration.resolve(unlisten);

      await expect(settled).resolves.toBeUndefined();
      expect(unlisten).toHaveBeenCalledTimes(1);

      await expect(unlistenOnce()).resolves.toBeUndefined();
      expect(unlisten).toHaveBeenCalledTimes(1);
    });

    it('never lets a rejecting async listener escape and still cleans up', async () => {
      const unlisten = vi.fn<() => Promise<void>>(async () => undefined);
      listenMock.mockImplementation((_event, _handler) => Promise.resolve(unlisten));

      const unlistenOnce = nativePort().onCloseRequested(() =>
        Promise.reject(new Error('listener failed')),
      );
      expect(() =>
        latestCloseHandler()({ event: 'opennote://close-requested', payload: null }),
      ).not.toThrow();
      await awaitMicrotasks(6);

      await expect(unlistenOnce()).resolves.toBeUndefined();
      expect(unlisten).toHaveBeenCalledTimes(1);
    });

    it('resolves cleanup even when the registration itself fails', async () => {
      listenMock.mockReturnValue(Promise.reject<() => Promise<void>>(new Error('ipc unavailable')));

      const unlistenOnce = nativePort().onCloseRequested(vi.fn<() => void>());

      await expect(unlistenOnce()).resolves.toBeUndefined();
      await awaitMicrotasks();
    });
  });

  it('offers a browser-safe no-op close subscription that never reaches IPC', async () => {
    delete internalsHost.__TAURI_INTERNALS__;

    const unlistenOnce = disconnectedShellPort.onCloseRequested(() => {
      throw new Error('close events never fire while disconnected');
    });
    await expect(unlistenOnce()).resolves.toBeUndefined();

    const confirm = await disconnectedShellPort.confirmClose();
    expect(confirm.ok).toBe(false);
    if (!confirm.ok) {
      expect(confirm.error.code).toBe('NOT_IMPLEMENTED');
    }
    expect(invokeMock).not.toHaveBeenCalled();
    expect(listenMock).not.toHaveBeenCalled();
  });
});

describe('native note adapter over mocked IPC', () => {
  it('invokes native note commands with camelCase argument names exactly', async () => {
    const service = nativeNoteService();
    const document: NoteDocument = {
      noteId: 'note-1',
      vaultId: 'vault-1',
      relativePath: 'notes/new-note.md',
      source: '# draft',
      localRevision: 1,
      contentHash: 'hash-1',
      readOnly: false,
    };

    invokeMock.mockResolvedValueOnce(document);
    const created = await service.createNote({
      relativePath: 'notes/new-note.md',
      source: '# draft',
      operationId: 'uuid-1',
    });
    expect(invokeMock).toHaveBeenCalledWith('create_note', {
      vaultId: 'vault-1',
      relativePath: 'notes/new-note.md',
      source: '# draft',
      operationId: 'uuid-1',
    });
    expect(created).toEqual({ ok: true, value: document });

    invokeMock.mockResolvedValueOnce({ noteId: 'note-1', localRevision: 3, contentHash: 'hash-3' });
    const saved = await service.saveNote({
      noteId: 'note-1',
      source: '# saved',
      expectedRevision: 2,
      operationId: 'op-1',
    });
    expect(invokeMock).toHaveBeenCalledWith('save_note', {
      vaultId: 'vault-1',
      noteId: 'note-1',
      source: '# saved',
      expectedRevision: 2,
      operationId: 'op-1',
    });
    expect(saved.ok).toBe(true);

    invokeMock.mockResolvedValueOnce(document);
    const read = await service.readNote('note-2');
    expect(invokeMock).toHaveBeenCalledWith('read_note', { vaultId: 'vault-1', noteId: 'note-2' });
    expect(read.ok).toBe(true);

    const summary: NoteSummary = {
      noteId: 'note-1',
      vaultId: 'vault-1',
      relativePath: 'notes/new-note.md',
      contentHash: 'hash-1',
      localRevision: 1,
      readOnly: false,
    };
    invokeMock.mockResolvedValueOnce([summary]);
    const listed = await service.listNotes();
    expect(invokeMock).toHaveBeenCalledWith('list_notes', { vaultId: 'vault-1' });
    expect(listed).toEqual({ ok: true, value: [summary] });

    invokeMock.mockResolvedValueOnce(['notes']);
    expect(await service.listFolders()).toEqual({ ok: true, value: ['notes'] });
    expect(invokeMock).toHaveBeenCalledWith('list_folders', { vaultId: 'vault-1' });
    invokeMock.mockResolvedValueOnce('notes/new');
    expect(await service.createFolder('notes/new')).toEqual({ ok: true, value: 'notes/new' });
    expect(invokeMock).toHaveBeenCalledWith('create_folder', { vaultId: 'vault-1', relativePath: 'notes/new' });
    invokeMock.mockResolvedValueOnce(null);
    expect(await service.recycleFolder('thesis')).toEqual({ ok: true, value: null });
    expect(invokeMock).toHaveBeenCalledWith('recycle_folder', { vaultId: 'vault-1', relativePath: 'thesis' });

    invokeMock.mockResolvedValueOnce({ applied: ['a'], pending: [] });
    const recovered = await service.recoverVault();
    expect(invokeMock).toHaveBeenCalledWith('recover_vault', { vaultId: 'vault-1' });
    expect(recovered).toEqual({ ok: true, value: { applied: ['a'], pending: [] } });
  });

  it('sends property writes in the host enum and maps the previous value back', async () => {
    const service = nativeNoteService();
    invokeMock.mockResolvedValueOnce({
      source: '---\nstatus: Done\n---\n',
      previous: { select: 'Todo' },
      changed: true,
    });

    const written = await service.applyPropertyEdit('---\nstatus: Todo\n---\n', 'status', {
      kind: 'select',
      text: 'Done',
    });

    expect(invokeMock).toHaveBeenCalledWith('apply_property_edit', {
      source: '---\nstatus: Todo\n---\n',
      name: 'status',
      value: { select: 'Done' },
    });
    expect(written).toEqual({
      ok: true,
      value: {
        source: '---\nstatus: Done\n---\n',
        previous: { kind: 'select', text: 'Todo' },
        changed: true,
      },
    });

    invokeMock.mockResolvedValueOnce({ source: '', previous: 'unset', changed: true });
    await service.applyPropertyEdit('', 'due', { kind: 'unset' });
    expect(invokeMock).toHaveBeenLastCalledWith('apply_property_edit', {
      source: '',
      name: 'due',
      value: 'unset',
    });
  });

  it('binds each vault id immutably per service and fails a missing scope typed without IPC', async () => {
    const serviceA = resolveNoteService('vault-a');
    const serviceB = resolveNoteService('vault-b');
    const unscoped = resolveNoteService(undefined);

    invokeMock.mockResolvedValueOnce([]).mockResolvedValueOnce([]);
    await serviceA.listNotes();
    await serviceB.listNotes();
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'list_notes', { vaultId: 'vault-a' });
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'list_notes', { vaultId: 'vault-b' });

    const refused = await unscoped.readNote('note-1');
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(refused.ok).toBe(false);
    if (!refused.ok) {
      expect(refused.error.code).toBe('INVALID_INPUT');
      expect(refused.error.messageKey).toBe('error.noVaultOpen');
    }
  });

  it('preserves structured ServiceError fields for note commands', async () => {
    const typed: ServiceError = {
      code: 'VERSION_CONFLICT',
      messageKey: 'error.conflict',
      params: {},
      retryable: false,
      operationId: null,
      retryAfterSeconds: null,
    };
    invokeMock.mockRejectedValueOnce(typed);

    const outcome = await nativeNoteService().saveNote({
      noteId: 'note-1',
      source: 'x',
      expectedRevision: 1,
      operationId: 'op-2',
    });

    expect(outcome).toEqual({ ok: false, error: typed });
  });

  it('sanitizes unknown note-command failures into IO_DENIED without leaking details', async () => {
    invokeMock.mockRejectedValueOnce('sqlite layer exploded beside /vault/state.sqlite');

    const outcome = await nativeNoteService().readNote('note-1');

    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.error).toEqual(genericUnknownFailure);
      expect(JSON.stringify(outcome.error)).not.toContain('state.sqlite');
    }
  });

  it.each(['BACKUP_INCOMPLETE', 'OPERATION_CANCELLED', 'RESTORE_INTERRUPTED'] as const)('preserves the %s outcome across native IPC', async (code) => {
    const failure: ServiceError = { code, messageKey: 'error.backup', params: {}, retryable: false, operationId: 'snapshot-operation', retryAfterSeconds: null };
    invokeMock.mockRejectedValueOnce(failure);
    expect(await nativeNoteService().readNote('note-1')).toEqual({ ok: false, error: failure });
  });

  it('maps export_draft booleans to saved with suggestedName from the relative path', async () => {
    invokeMock.mockResolvedValueOnce(true);

    const saved = await nativeNoteService().exportDraft({
      noteId: 'note-1',
      relativePath: 'notes/case.md',
      source: '# draft body',
    });

    expect(invokeMock).toHaveBeenCalledWith('export_draft', {
      source: '# draft body',
      suggestedName: 'case.md',
    });
    expect(saved).toEqual({ ok: true, value: { outcome: 'saved' } });
  });

  it('maps export_draft false to an honest cancelled success without inventing args', async () => {
    invokeMock.mockResolvedValueOnce(false);

    const cancelled = await nativeNoteService().exportDraft({
      noteId: 'note-1',
      relativePath: '',
      source: 'x',
    });

    expect(invokeMock).toHaveBeenCalledWith('export_draft', { source: 'x' });
    expect(cancelled).toEqual({ ok: true, value: { outcome: 'cancelled' } });
  });

  it('saves a reviewed diagnostics report and keeps cancellation null', async () => {
    const request: DiagnosticsRequest = {
      facts: {
        appVersion: '0.1.0',
        buildChannel: 'dev',
        platform: 'windows-x64',
        locale: 'en',
        roots: [],
      },
      correlationId: 'corr-1',
      counts: [],
      capabilities: [],
      statuses: [],
      lastErrorCode: null,
      timestampMillis: 1,
      rawLog: null,
    };
    const preview: DiagnosticsPreview = {
      correlationId: 'corr-1',
      fields: [],
      statuses: [],
      redactions: [],
      omitted: [],
      includesLog: false,
    };
    invokeMock.mockResolvedValueOnce('C:/Users/u/report.txt');
    const service = nativeNoteService();
    const saved = await service.diagnosticsExport(preview, request, 'Export diagnostics');
    expect(invokeMock).toHaveBeenLastCalledWith('diagnostics_export', {
      preview,
      request,
      dialogTitle: 'Export diagnostics',
    });
    expect(saved).toEqual({ ok: true, value: 'C:/Users/u/report.txt' });

    invokeMock.mockResolvedValueOnce(null);
    expect(
      await service.diagnosticsExport(preview, request, 'Export diagnostics'),
    ).toEqual({ ok: true, value: null });
  });

  it('binds checkpoint, metadata paging and on-demand body reads to its vault', async () => {
    const service = nativeNoteService();
    const request = {
      noteId: 'note-1',
      relativePath: 'notes/case.md',
      source: 'x',
      baseRevision: 1, baseContentHash: 'hash-1', operationId: 'draft-1',
    };
    invokeMock.mockResolvedValueOnce({ draftId: 'draft-1' });
    await service.writeCheckpoint(request);
    expect(invokeMock).toHaveBeenLastCalledWith('write_checkpoint', { vaultId: 'vault-1', ...request });
    invokeMock.mockResolvedValueOnce({ items: [], nextBeforeSequence: null });
    await service.listDrafts({ noteId: 'note-1', beforeSequence: 5, limit: 20 });
    expect(invokeMock).toHaveBeenLastCalledWith('list_drafts', { vaultId: 'vault-1', noteId: 'note-1', beforeSequence: 5, limit: 20 });
    invokeMock.mockResolvedValueOnce({ source: 'x' });
    await service.readDraft('draft-1');
    expect(invokeMock).toHaveBeenLastCalledWith('read_draft', { vaultId: 'vault-1', draftId: 'draft-1' });
  });

  it('passes the SaveResult value through unchanged after save_note', async () => {
    const service = nativeNoteService();
    const value: SaveResult = { noteId: 'note-1', localRevision: 2, contentHash: 'hash-2' };

    invokeMock.mockResolvedValueOnce(value);
    const outcome = await service.saveNote({
      noteId: 'note-1',
      source: 'x',
      expectedRevision: 1,
      operationId: 'op-9',
    });

    expect(outcome).toEqual({ ok: true, value });
  });

  it('scopes history list, reads and protected restoration to the current vault', async () => {
    const service = nativeNoteService();
    invokeMock.mockResolvedValueOnce({ items: [], unavailableRevisionIds: [] });
    await service.listHistory('note-1');
    expect(invokeMock).toHaveBeenLastCalledWith('list_history', { vaultId: 'vault-1', noteId: 'note-1' });
    const history: HistoryRevisionContent = {
      document: {
        noteId: 'note-1', vaultId: 'vault-1', relativePath: 'notes/version.md', source: 'body',
        localRevision: 1, contentHash: 'hash-1', readOnly: false,
      },
      referenceReport: {
        complete: false,
        issues: [{
          noteId: 'note-1', notePath: 'notes/version.md', reference: 'assets/missing.png', reason: 'missing',
        }],
      },
    };
    invokeMock.mockResolvedValueOnce(history);
    const read = await service.readHistory('note-1', 'r1-hash');
    expect(read).toEqual({ ok: true, value: history });
    expect(invokeMock).toHaveBeenLastCalledWith('read_history', { vaultId: 'vault-1', noteId: 'note-1', revisionId: 'r1-hash' });
    invokeMock.mockResolvedValueOnce({});
    await service.restoreHistory({ noteId: 'note-1', revisionId: 'r1-hash', expectedRevision: 3, operationId: 'restore-op' });
    expect(invokeMock).toHaveBeenLastCalledWith('restore_history', { vaultId: 'vault-1', noteId: 'note-1', revisionId: 'r1-hash', expectedRevision: 3, operationId: 'restore-op' });
  });

  it('scopes history retention and cleanup requests to the current vault', async () => {
    const service = nativeNoteService();
    const settings = { retentionDays: 30, revision: 2 };
    invokeMock.mockResolvedValueOnce(settings);
    expect(await service.historyRetentionSettings()).toEqual({ ok: true, value: settings });
    expect(invokeMock).toHaveBeenLastCalledWith('history_retention_settings', { vaultId: 'vault-1' });

    invokeMock.mockResolvedValueOnce({ retentionDays: 45, revision: 3 });
    await service.setHistoryRetention(45, 2);
    expect(invokeMock).toHaveBeenLastCalledWith('set_history_retention', {
      vaultId: 'vault-1',
      retentionDays: 45,
      expectedRevision: 2,
    });

    const plan = {
      planId: 'plan-1',
      retentionDays: 30,
      settingsRevision: 2,
      generatedAt: '2026-10-20T12:00:00Z',
      cutoff: '2026-09-20T12:00:00Z',
      retainedCount: 4,
      candidates: [],
      preserved: [],
      candidateBytes: 0,
    };
    const preview = { current: plan, pending: [] };
    invokeMock.mockResolvedValueOnce(preview);
    expect(await service.previewHistoryCleanup()).toEqual({ ok: true, value: preview });
    expect(invokeMock).toHaveBeenLastCalledWith('preview_history_cleanup', { vaultId: 'vault-1' });

    const job = { jobId: 'opaque-job', operationId: 'cleanup-op', status: 'running' as const,
      progress: { removed: 0, total: 0 }, cancelRequested: false, result: null, error: null };
    invokeMock.mockResolvedValueOnce(job);
    expect(await service.applyHistoryCleanup(plan, 'cleanup-op')).toEqual({ ok: true, value: job });
    expect(invokeMock).toHaveBeenLastCalledWith('apply_history_cleanup', {
      vaultId: 'vault-1',
      plan,
      operationId: 'cleanup-op',
    });
    invokeMock.mockResolvedValueOnce(job);
    expect(await service.historyCleanupStatus('opaque-job')).toEqual({ ok: true, value: job });
    expect(invokeMock).toHaveBeenLastCalledWith('history_cleanup_status', { vaultId: 'vault-1', jobId: 'opaque-job' });
    invokeMock.mockResolvedValueOnce({ ...job, cancelRequested: true });
    await service.cancelHistoryCleanup('opaque-job');
    expect(invokeMock).toHaveBeenLastCalledWith('cancel_history_cleanup', { vaultId: 'vault-1', jobId: 'opaque-job' });
    invokeMock.mockResolvedValueOnce(job);
    await service.stopHistoryCleanup('cleanup-op');
    expect(invokeMock).toHaveBeenLastCalledWith('stop_history_cleanup', { vaultId: 'vault-1', operationId: 'cleanup-op' });
  });

  it('scopes every metadata query to the current vault and passes the embed chain through', async () => {
    const service = nativeNoteService();
    invokeMock.mockResolvedValueOnce({ tags: [], unreadable: [] });
    expect(await service.listTags()).toEqual({ ok: true, value: { tags: [], unreadable: [] } });
    expect(invokeMock).toHaveBeenLastCalledWith('list_tags', { vaultId: 'vault-1' });

    invokeMock.mockResolvedValueOnce({ noteId: 'note-1', backlinks: [], unreadable: [] });
    await service.listBacklinks('note-1');
    expect(invokeMock).toHaveBeenLastCalledWith('list_backlinks', {
      vaultId: 'vault-1',
      noteId: 'note-1',
    });

    invokeMock.mockResolvedValueOnce({ broken: [], missingAnchors: [], unreadable: [] });
    await service.listLinkIssues();
    expect(invokeMock).toHaveBeenLastCalledWith('list_link_issues', { vaultId: 'vault-1' });

    invokeMock.mockResolvedValueOnce({ noteId: 'note-1' });
    await service.readNoteMetadata('note-1');
    expect(invokeMock).toHaveBeenLastCalledWith('read_note_metadata', {
      vaultId: 'vault-1',
      noteId: 'note-1',
    });

    const request = {
      query: '京都',
      filters: { folders: ['notes'], tags: ['travel'], types: [] },
      locale: 'ja',
      limit: 50,
      cursor: null,
    };
    invokeMock.mockResolvedValueOnce({ items: [], nextCursor: null, indexStatus: { state: 'ready', version: 1, indexedNotes: 0, unreadableNotes: 0 } });
    await service.searchNotes(request);
    expect(invokeMock).toHaveBeenLastCalledWith('search_notes', { vaultId: 'vault-1', request });
    invokeMock.mockResolvedValueOnce({ jobId: 'job', status: 'running' });
    await service.startSearch(request);
    expect(invokeMock).toHaveBeenLastCalledWith('start_search', { vaultId: 'vault-1', request });
    invokeMock.mockResolvedValueOnce({ jobId: 'job', status: 'completed' });
    await service.searchJobStatus('job');
    expect(invokeMock).toHaveBeenLastCalledWith('search_job_status', { vaultId: 'vault-1', jobId: 'job' });
    invokeMock.mockResolvedValueOnce(undefined);
    await service.cancelSearch('job');
    expect(invokeMock).toHaveBeenLastCalledWith('cancel_search', { vaultId: 'vault-1', jobId: 'job' });

    // A fresh expansion sends an empty chain; the host seeds it with the root.
    invokeMock.mockResolvedValueOnce({ status: 'ok', chain: [] });
    await service.expandNoteEmbed({ noteId: 'note-1', target: 'notes/b', anchor: 'Two' });
    expect(invokeMock).toHaveBeenLastCalledWith('expand_note_embed', {
      vaultId: 'vault-1',
      noteId: 'note-1',
      target: 'notes/b',
      anchor: 'Two',
      chain: [],
    });

    // A nested expansion must forward the path so cycles stay detectable.
    invokeMock.mockResolvedValueOnce({ status: 'cycle', chain: ['note-1', 'note-2'] });
    await service.expandNoteEmbed({
      noteId: 'note-2',
      target: 'note-1',
      chain: ['note-1', 'note-2'],
    });
    expect(invokeMock).toHaveBeenLastCalledWith('expand_note_embed', {
      vaultId: 'vault-1',
      noteId: 'note-2',
      target: 'note-1',
      anchor: null,
      chain: ['note-1', 'note-2'],
    });
  });

  it('refuses metadata queries without a vault scope instead of calling the host', async () => {
    const unscoped = resolveNoteService(null);
    const tags = await unscoped.listTags();
    expect(tags.ok).toBe(false);
    const links = await unscoped.listLinkIssues();
    expect(links.ok).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('previews a rename read-only and refuses it without a vault scope', async () => {
    const service = nativeNoteService();
    const plan = {
      noteId: 'note-1',
      fromPath: 'notes/old.md',
      toPath: 'notes/new.md',
      keepsIdentity: true,
      edits: [
        {
          noteId: 'note-2',
          relativePath: 'notes/src.md',
          offset: 4,
          length: 14,
          before: '[[notes/old]]',
          after: '[[notes/new]]',
        },
      ],
      untouched: [],
    };
    invokeMock.mockResolvedValueOnce(plan);
    expect(await service.previewNoteRename('note-1', 'notes/new.md')).toEqual({
      ok: true,
      value: plan,
    });
    expect(invokeMock).toHaveBeenLastCalledWith('preview_note_rename', {
      vaultId: 'vault-1',
      noteId: 'note-1',
      toPath: 'notes/new.md',
    });

    const sealed = { ...plan, planId: 'plan', vaultId: 'vault-1', expectedHashes: [{ noteId: 'note-1', relativePath: 'notes/old.md', expectedRevision: 7, contentHash: 'a'.repeat(64) }] };
    invokeMock.mockResolvedValueOnce({ steps: [], issues: [] });
    await service.previewRenameCommit(sealed);
    expect(invokeMock).toHaveBeenLastCalledWith('preview_rename_commit', { vaultId: 'vault-1', plan: sealed });
    const committed = { commitId: 'operation', completed: true, items: [] };
    invokeMock.mockResolvedValueOnce(committed);
    expect(await service.rename(sealed, 7, 'operation')).toEqual({ ok: true, value: committed });
    expect(invokeMock).toHaveBeenLastCalledWith('rename_note', { vaultId: 'vault-1', plan: sealed, expectedRevision: 7, operationId: 'operation' });

    const unscoped = resolveNoteService(null);
    expect((await unscoped.previewNoteRename('note-1', 'notes/new.md')).ok).toBe(false);
  });

  it('scopes conflict preparation and new-identity copies to the current vault', async () => {
    const service = nativeNoteService();
    invokeMock.mockResolvedValueOnce({});
    await service.prepareConflict('note-1');
    expect(invokeMock).toHaveBeenLastCalledWith('prepare_conflict', { vaultId: 'vault-1', noteId: 'note-1' });
    const request = { sourceNoteId: 'note-1', relativePath: 'copy.md', source: 'my text', operationId: 'copy-op' };
    invokeMock.mockResolvedValueOnce({});
    await service.createCopy(request);
    expect(invokeMock).toHaveBeenLastCalledWith('create_copy', { vaultId: 'vault-1', ...request });
  });

  it('scopes every trash operation and preserves the caller operation identity', async () => {
    const service = nativeNoteService();
    const entryRequest = { objectId: 'trash-1', expectedRevision: 4, operationId: 'op-1' };
    for (const [command, args, call] of [
      ['list_trash', {}, () => service.listTrash()],
      ['read_trash', { objectId: 'trash-1' }, () => service.readTrash('trash-1')],
      ['delete_note', { noteId: 'note-1', expectedRevision: 3, expectedHash: 'hash-3', operationId: 'delete-op' }, () => service.deleteNote({ noteId: 'note-1', expectedRevision: 3, expectedHash: 'hash-3', operationId: 'delete-op' })],
      ['restore_trash', { ...entryRequest, targetPath: 'restored.md' }, () => service.restoreTrash({ ...entryRequest, targetPath: 'restored.md' })],
      ['purge_trash', entryRequest, () => service.purgeTrash(entryRequest)],
      ['list_trash_operations', {}, () => service.listTrashOperations()],
      ['resume_trash', { operationId: 'original-op' }, () => service.resumeTrash('original-op')],
      ['cancel_trash', { operationId: 'original-op' }, () => service.cancelTrash('original-op')],
    ] as const) {
      invokeMock.mockResolvedValueOnce(null);
      await call();
      expect(invokeMock).toHaveBeenLastCalledWith(command, { vaultId: 'vault-1', ...args });
    }
  });

  it('scopes backup handles, opaque preview tokens and cancellation without passing filesystem paths', async () => {
    const service = resolveBackupService('vault-1');
    for (const [command, args, call] of [
      ['backup_context', {}, () => service.context()],
      ['choose_backup_destination', { dialogTitle: '备份位置' }, () => service.chooseDestination('备份位置')],
      ['start_backup_task', { selectionId: 'selected', operationId: 'operation', kind: 'restore', planToken: 'opaque-preview' }, () => service.start('selected', 'operation', 'restore', 'opaque-preview')],
      ['start_backup_task', { selectionId: 'selected', operationId: 'overwrite-op', kind: 'overwrite', planToken: 'overwrite-preview' }, () => service.start('selected', 'overwrite-op', 'overwrite', 'overwrite-preview')],
      ['start_backup_task', { selectionId: 'selected', operationId: 'daily-op', kind: 'daily', planToken: null }, () => service.start('selected', 'daily-op', 'daily')],
      ['start_backup_task', { selectionId: 'selected', operationId: 'retention-op', kind: 'retention', planToken: null }, () => service.start('selected', 'retention-op', 'retention')],
      ['start_backup_task', { selectionId: 'selected', operationId: 'cleanup-op', kind: 'cleanup', planToken: 'cleanup-preview' }, () => service.start('selected', 'cleanup-op', 'cleanup', 'cleanup-preview')],
      ['start_backup_task', { selectionId: 'selected', operationId: 'cancel-cleanup-op', kind: 'cancelCleanup', planToken: 'pending-preview' }, () => service.start('selected', 'cancel-cleanup-op', 'cancelCleanup', 'pending-preview')],
      ['preview_backup_upgrade', { selectionId: 'selected', operationId: 'upgrade-preview-op', snapshotId: 'snapshot' }, () => service.previewUpgrade('selected', 'upgrade-preview-op', 'snapshot')],
      ['preview_backup_upgrade', { selectionId: 'selected', operationId: 'upgrade-pending-op', snapshotId: null }, () => service.previewUpgrade('selected', 'upgrade-pending-op')],
      ['start_backup_task', { selectionId: 'selected', operationId: 'upgrade-op', kind: 'upgrade', planToken: 'upgrade-token' }, () => service.start('selected', 'upgrade-op', 'upgrade', 'upgrade-token')],
      ['start_backup_task', { selectionId: 'selected', operationId: 'upgrade-abandon-op', kind: 'upgradeAbandon', planToken: 'pending-upgrade-token' }, () => service.start('selected', 'upgrade-abandon-op', 'upgradeAbandon', 'pending-upgrade-token')],
      ['preview_backup_overwrite', { selectionId: 'selected', operationId: 'overwrite-preview-op', snapshotId: 'snapshot' }, () => service.previewOverwrite('selected', 'overwrite-preview-op', 'snapshot')],
      ['set_backup_retention', { selectionId: 'selected', expectedRevision: 2, retainedSnapshots: 3 }, () => service.setRetention('selected', 2, 3)],
      ['preview_backup_restore', { selectionId: 'selected', operationId: 'preview-op', snapshotId: 'snapshot', targetName: '新目录', dialogTitle: '恢复位置' }, () => service.preview('selected', 'preview-op', 'snapshot', '新目录', '恢复位置')],
      ['backup_job_status', { jobId: 'job' }, () => service.status('job')],
      ['cancel_backup_task', { jobId: 'job' }, () => service.cancel('job')],
    ] as const) {
      invokeMock.mockResolvedValueOnce(null); await call();
      expect(invokeMock).toHaveBeenLastCalledWith(command, { vaultId: 'vault-1', ...args });
    }
    const count = invokeMock.mock.calls.length;
    expect((await resolveBackupService(undefined).context()).ok).toBe(false);
    expect(invokeMock).toHaveBeenCalledTimes(count);
  });
});


describe('tag batch IPC', () => {
  it('projects host conflict tuples and scopes preview, apply, resume, cancel and list', async () => {
    const service = nativeNoteService();
    const issue = { kind: 'sourceUnsupported' as const, noteId: 'paused' };
    const plan = { planId: 'plan', vaultId: 'vault-1', oldTag: 'old', newTag: null, isMerge: false, items: [] };
    const batch = { operationId: 'operation', oldTag: 'old', newTag: null, isMerge: false, completed: false, cancelled: false, items: [] };
    invokeMock.mockResolvedValueOnce({ plan: { oldTag: 'old', newTag: '', isMerge: false, rewrites: [], conflicted: [['paused', issue]], totalChanges: 0 }, writePlan: null, conflictPaths: { paused: 'Alias.md' } });
    const preview = await service.previewTagUpdate('old', null);
    expect(preview.ok && preview.value.plan.conflicted).toEqual([{ noteId: 'paused', issue }]);
    expect(preview.ok && preview.value.conflictPaths).toEqual({ paused: 'Alias.md' });
    expect(invokeMock).toHaveBeenLastCalledWith('preview_tag_update', { vaultId: 'vault-1', oldTag: 'old', newTag: null });
    invokeMock.mockResolvedValueOnce(batch);
    expect(await service.applyTagUpdate(plan, 'operation')).toEqual({ ok: true, value: batch });
    expect(invokeMock).toHaveBeenLastCalledWith('apply_tag_update', { vaultId: 'vault-1', plan, operationId: 'operation' });
    for (const [method, command] of [['resumeTagUpdate', 'resume_tag_update'], ['cancelTagUpdate', 'cancel_tag_update']] as const) {
      invokeMock.mockResolvedValueOnce(batch);
      expect(await service[method]('operation')).toEqual({ ok: true, value: batch });
      expect(invokeMock).toHaveBeenLastCalledWith(command, { vaultId: 'vault-1', operationId: 'operation' });
    }
    invokeMock.mockResolvedValueOnce([batch]);
    expect(await service.listTagUpdates()).toEqual({ ok: true, value: [batch] });
    expect(invokeMock).toHaveBeenLastCalledWith('list_tag_updates', { vaultId: 'vault-1' });
  });
});


describe('source notification subscription', () => {
  it('attaches before starting, filters by vault/watch and stops idempotently', async () => {
    const detach = vi.fn(async () => undefined);
    listenMock.mockResolvedValueOnce(detach); invokeMock.mockResolvedValue(undefined);
    const changed = vi.fn(); const outcome = await nativeNoteService().watchSourceChanges(['note'], changed);
    expect(listenMock).toHaveBeenCalledWith('opennote://source-changed', expect.any(Function));
    expect(invokeMock.mock.calls[0]?.[0]).toBe('start_source_watch');
    const args = invokeMock.mock.calls[0]![1]!;
    expect(args.vaultId).toBe('vault-1'); expect(args.noteIds).toEqual(['note']);
    const event = { event: 'opennote://source-changed', payload: { watchId: args.watchId, vaultId: 'vault-1', noteIds: ['note'], error: null } };
    latestCloseHandler()({ ...event, payload: { ...event.payload, vaultId: 'other' } });
    latestCloseHandler()({ ...event, payload: { ...event.payload, watchId: 'other' } });
    expect(changed).not.toHaveBeenCalled(); latestCloseHandler()(event); expect(changed).toHaveBeenCalledOnce();
    if (!outcome.ok) throw new Error('watch registration failed');
    await outcome.value(); await outcome.value(); latestCloseHandler()(event);
    expect(changed).toHaveBeenCalledOnce(); expect(detach).toHaveBeenCalledOnce();
    expect(invokeMock).toHaveBeenLastCalledWith('stop_source_watch', { vaultId: 'vault-1', watchId: args.watchId });
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });
  it('detaches and requests stop if native startup fails', async () => {
    const detach = vi.fn(async () => undefined); listenMock.mockResolvedValueOnce(detach);
    invokeMock.mockRejectedValueOnce(genericUnknownFailure).mockResolvedValueOnce(undefined);
    const outcome = await nativeNoteService().watchSourceChanges(['note'], vi.fn());
    expect(outcome).toEqual({ ok: false, error: genericUnknownFailure }); expect(detach).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls[1]?.[0]).toBe('stop_source_watch');
  });
});


it('scopes catalog refresh and root notifications and cleans the shared native watch resource', async () => {
  const result = { notes: [], changes: [], unavailableNoteIds: [], findings: [], fingerprint: 'catalog', deferred: false, persisted: true, writable: true };
  const service = nativeNoteService(); invokeMock.mockResolvedValueOnce(result);
  expect(await service.refreshCatalog()).toEqual({ ok: true, value: result });
  expect(invokeMock).toHaveBeenLastCalledWith('refresh_catalog', { vaultId: 'vault-1' });
  listenMock.mockResolvedValueOnce(async () => undefined); invokeMock.mockResolvedValue(undefined);
  const changed = vi.fn(); const watch = await service.watchCatalogChanges(changed);
  expect(listenMock).toHaveBeenCalledWith('opennote://catalog-changed', expect.any(Function));
  const args = invokeMock.mock.calls[1]![1]!; expect(args.vaultId).toBe('vault-1'); expect(args.noteIds).toBeUndefined();
  latestCloseHandler()({ event: 'opennote://catalog-changed', payload: { ...args, noteIds: [], error: null } });
  expect(changed).toHaveBeenCalledWith(null);
  if (!watch.ok) throw new Error('watch failed'); await watch.value();
  expect(invokeMock).toHaveBeenLastCalledWith('stop_source_watch', args);
});
