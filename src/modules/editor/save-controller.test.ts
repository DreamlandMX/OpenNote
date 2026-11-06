import { describe, expect, it } from 'vitest';
import { SaveController, type ControllerEvent, type SaveControllerIo } from './save-controller';
import type { NoteDocument, SaveResult, ServiceError } from '../domain';
import type { ServiceOutcome, SaveNoteRequest } from './ports';

it('keeps a clean editing baseline available when deletion preflight discovers an external change', () => {
  const { controller } = controllerFor(ioWithQueue().io);
  controller.registerDocument(documentOf('n1', 'old text', 1));
  expect(controller.reportExternalConflict('n1')).toBe(true);
  expect(controller.getBufferText('n1')).toBe('old text');
  expect(controller.snapshot('n1')).toMatchObject({ status: 'conflict', revision: 1 });
  expect(controller.notesNeedingAttentionOnExit()).toEqual([{ noteId: 'n1', status: 'conflict' }]);
});

it('keeps the disk hash paired with the acknowledged revision, never pending edits', async () => {
  const { io, calls } = ioWithQueue();
  const { controller } = controllerFor(io);
  controller.registerDocument(documentOf('n1', 'old', 1));
  controller.setBufferText('n1', 'edited');
  const saving = controller.requestSave('n1');
  await awaitMicrotasks();
  expect(controller.snapshot('n1')?.contentHash).toBe('hash-1');
  calls[0]!.settle.resolve({ ok: true, value: { noteId: 'n1', localRevision: 2, contentHash: 'disk-hash-2' } });
  await saving;
  expect(controller.snapshot('n1')).toMatchObject({ revision: 2, contentHash: 'disk-hash-2' });
});

it('refuses draft restoration over dirty or composing buffers', () => {
  const { controller } = controllerFor(ioWithQueue().io);
  controller.registerDocument(documentOf('n1', 'old', 1));
  controller.setBufferText('n1', 'my edits');
  expect(controller.restoreDraft('n1', 'draft')).toBe(false);
  expect(controller.getBufferText('n1')).toBe('my edits');
  controller.registerDocument(documentOf('n2', 'old', 1));
  controller.compositionStarted('n2');
  expect(controller.restoreDraft('n2', 'draft')).toBe(false);
  controller.compositionEnded('n2');
  expect(controller.restoreDraft('n2', 'draft')).toBe(true);
  expect(controller.snapshot('n2')?.status).toBe('dirty');
});

it('tracks history restoration as an in-flight save and retries its original revision ID', async () => {
  const calls: unknown[] = [];
  const result = deferred<ServiceOutcome<SaveResult>>();
  let attempt = 0;
  const controller = new SaveController({
    saveNote: async () => { throw Error('history restoration must use its protected native command'); },
    restoreHistory(request) {
      calls.push(request);
      if (++attempt === 1) return result.promise;
      return Promise.resolve({ ok: true as const, value: { noteId: 'n', localRevision: 3, contentHash: 'restored' } });
    },
  }, { uuid: () => 'restore-op' });
  controller.registerDocument(documentOf('n', 'current', 2));
  const restoring = controller.requestHistoryRestore('n', documentOf('n', 'historic', 0), 'r0-hash');
  await awaitMicrotasks();
  expect(controller.notesNeedingAttentionOnExit()).toEqual([{ noteId: 'n', status: 'saving' }]);
  result.resolve({ ok: false, error: serviceError('FILE_LOCKED', 'restore-op') });
  await restoring;
  expect(controller.getBufferText('n')).toBe('historic');
  await controller.retryFailed('n');
  expect(calls).toEqual([
    { noteId: 'n', revisionId: 'r0-hash', expectedRevision: 2, operationId: 'restore-op' },
    { noteId: 'n', revisionId: 'r0-hash', expectedRevision: 2, operationId: 'restore-op' },
  ]);
  expect(controller.snapshot('n')).toMatchObject({ status: 'clean', revision: 3, contentHash: 'restored' });
});

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

interface RecordedCall {
  request: SaveNoteRequest;
  settle: Deferred<ServiceOutcome<SaveResult>>;
}

function ioWithQueue(): { io: SaveControllerIo; calls: RecordedCall[] } {
  const calls: RecordedCall[] = [];
  const io: SaveControllerIo = {
    saveNote(request: SaveNoteRequest): Promise<ServiceOutcome<SaveResult>> {
      const settle = deferred<ServiceOutcome<SaveResult>>();
      calls.push({ request, settle });
      return settle.promise;
    },
  };
  return { io, calls };
}

function awaitMicrotasks(times = 4): Promise<void> {
  let running: Promise<void> = Promise.resolve();
  for (let index = 0; index < times; index += 1) {
    running = running.then(() => Promise.resolve());
  }
  return running.then(() => undefined);
}

function documentOf(noteId: string, source: string, localRevision: number): NoteDocument {
  return {
    noteId,
    vaultId: 'v1',
    relativePath: 'notes/example.md',
    source,
    localRevision,
    contentHash: `hash-${localRevision}`,
    readOnly: false,
  };
}

function controllerFor(
  io: SaveControllerIo,
): { controller: SaveController; uuidCalls: string[] } {
  const uuidCalls: string[] = [];
  let index = 0;
  const controller = new SaveController(io, {
    uuid: () => {
      index += 1;
      const id = `op-${index}`;
      uuidCalls.push(id);
      return id;
    },
  });
  return { controller, uuidCalls };
}

function serviceError(code: ServiceError['code'], operationId: string): ServiceError {
  return {
    code,
    messageKey: 'opennote.service.test',
    params: {},
    retryable: true,
    operationId,
    retryAfterSeconds: null,
  };
}

function saveOk(noteId: string, localRevision: number): { ok: true; value: SaveResult } {
  return { ok: true, value: { noteId, localRevision, contentHash: `hash-${localRevision}` } };
}

describe('SaveController requestSave acknowledgement', () => {
  it('keeps the request promise pending until the deferred I/O resolves', async () => {
    const { io, calls } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    controller.setBufferText('n1', 'b');

    let resolved = false;
    void controller.requestSave('n1').then(() => {
      resolved = true;
    });
    await awaitMicrotasks(8);

    expect(calls).toHaveLength(1);
    expect(resolved).toBe(false);

    calls[0]?.settle.resolve(saveOk('n1', 2));
    await awaitMicrotasks(8);
    expect(resolved).toBe(true);

    const snapshot = controller.snapshot('n1');
    expect(snapshot?.status).toBe('clean');
    expect(snapshot?.revision).toBe(2);
  });

  it('resolves failure as a value (never an unhandled rejection) with a safe typed error', async () => {
    const { io, calls } = ioWithQueue();
    const { controller, uuidCalls } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'base-from-disk', 1));
    controller.setBufferText('n1', 'edited before the failed attempt');

    const done = controller.requestSave('n1');
    await awaitMicrotasks();
    calls[0]?.settle.reject(new Error('C:\\Windows\\TEMP\\disk i/o fault 42'));
    await done;
    await awaitMicrotasks(8);

    const snapshot = controller.snapshot('n1');
    expect(snapshot?.status).toBe('failed');
    expect(snapshot?.error?.code).toBe('IO_DENIED');
    expect(snapshot?.error?.messageKey).toBe('opennote.service.thrownFailure');
    expect(JSON.stringify(snapshot?.error)).not.toContain('disk i/o fault');
    expect(snapshot?.dirty).toBe(true);
    // The original failed request is preserved so a retry replays exactly it.
    expect(snapshot?.failedOperation?.operationId).toBe('op-1');
    expect(snapshot?.failedOperation?.source).toBe('edited before the failed attempt');
    expect(snapshot?.failedOperation?.expectedRevision).toBe(1);
    expect(snapshot?.revision).toBe(1);
    expect(uuidCalls).toEqual(['op-1']);
  });

  it('normalizes an unknown error code to IO_DENIED, both for returned outcomes and unknown thrown values', async () => {
    const { io, calls } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'A', 1));
    controller.setBufferText('n1', 'A2');
    const unexpected = controller.requestSave('n1');
    await awaitMicrotasks();
    const shapelessError = {
      ...serviceError('IO_DENIED', 'op-1'),
      code: 'MYSTERY_CODE',
    } as unknown as ServiceError;
    calls[0]?.settle.resolve({ ok: false, error: shapelessError });
    await unexpected;
    await awaitMicrotasks(8);
    expect(controller.snapshot('n1')?.error?.code).toBe('IO_DENIED');
    expect(controller.snapshot('n1')?.failedOperation?.operationId).toBe('op-1');

    controller.setBufferText('n1', 'A3');
    const thrownCode = controller.requestSave('n1');
    await awaitMicrotasks();
    expect(calls).toHaveLength(2);
    calls[1]?.settle.reject({ code: 'TOTALLY_UNKNOWN_KIND', secret: 'payload' });
    await thrownCode;
    await awaitMicrotasks(8);
    const thrownSnapshot = controller.snapshot('n1');
    expect(thrownSnapshot?.error?.code).toBe('IO_DENIED');
    expect(thrownSnapshot?.error?.messageKey).toBe('opennote.service.thrownFailure');
    expect(JSON.stringify(thrownSnapshot?.error)).not.toContain('payload');
    expect(thrownSnapshot?.dirty).toBe(true);
  });
});

describe('SaveController sequential saves', () => {
  it('chains newer text', async () => {
    const { io, calls } = ioWithQueue();
    const { controller, uuidCalls } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    controller.setBufferText('n1', 'b');

    const first = controller.requestSave('n1');
    await awaitMicrotasks();

    controller.setBufferText('n1', 'c-during-save');
    let snapshot = controller.snapshot('n1');
    expect(snapshot?.status).toBe('saving');
    expect(snapshot?.dirty).toBe(true);

    calls[0]?.settle.resolve(saveOk('n1', 2));
    await first;
    await awaitMicrotasks(8);

    expect(calls).toHaveLength(2);
    expect(calls[1]?.request.source).toBe('c-during-save');
    expect(calls[1]?.request.expectedRevision).toBe(2);
    snapshot = controller.snapshot('n1');
    expect(snapshot?.status).toBe('saving');
    expect(snapshot?.dirty).toBe(true);

    calls[1]?.settle.resolve(saveOk('n1', 3));
    await awaitMicrotasks(8);
    snapshot = controller.snapshot('n1');
    expect(snapshot?.status).toBe('clean');
    expect(snapshot?.revision).toBe(3);
    expect(uuidCalls).toEqual(['op-1', 'op-2']);
  });

  it('never treats an acknowledged earlier snapshot as cleaner of later edits', async () => {
    const { io, calls } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'base', 1));
    controller.setBufferText('n1', 'snapshot-text');
    const done = controller.requestSave('n1');
    await awaitMicrotasks();
    controller.setBufferText('n1', 'newer-text');
    calls[0]?.settle.resolve(saveOk('n1', 2));
    await done;
    await awaitMicrotasks(8);

    const snapshot = controller.snapshot('n1');
    expect(snapshot?.status).toBe('saving');
    expect(snapshot?.dirty).toBe(true);
    expect(snapshot?.failedOperation).toBeNull();
    expect(snapshot?.revision).toBe(2);

    expect(calls).toHaveLength(2);
    expect(calls[1]?.request.source).toBe('newer-text');
    calls[1]?.settle.resolve(saveOk('n1', 3));
    await awaitMicrotasks(8);
    expect(controller.snapshot('n1')?.status).toBe('clean');
    expect(controller.snapshot('n1')?.revision).toBe(3);
  });
});

describe('SaveController failed retries', () => {
  it('retries the exact failed operationId/source/expectedRevision even with newer edits, then saves newer text', async () => {
    const { io, calls } = ioWithQueue();
    const { controller, uuidCalls } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'base', 1));
    controller.setBufferText('n1', 'failed-source');

    const first = controller.requestSave('n1');
    await awaitMicrotasks();
    calls[0]?.settle.resolve({ ok: false, error: serviceError('IO_DENIED', 'op-1') });
    await first;

    let snapshot = controller.snapshot('n1');
    expect(snapshot?.status).toBe('failed');
    expect(snapshot?.failedOperation?.operationId).toBe('op-1');

    controller.setBufferText('n1', 'newer-edits-after-failure');
    expect(uuidCalls).toEqual(['op-1']);

    const retried = controller.retryFailed('n1');
    await awaitMicrotasks(8);
    expect(calls).toHaveLength(2);
    expect(calls[1]?.request.operationId).toBe('op-1');
    expect(calls[1]?.request.source).toBe('failed-source');
    expect(calls[1]?.request.expectedRevision).toBe(1);

    let retrySettled = false;
    void retried.then(() => {
      retrySettled = true;
    });
    calls[1]?.settle.resolve(saveOk('n1', 5));
    await awaitMicrotasks(8);
    expect(retrySettled).toBe(true);

    expect(calls).toHaveLength(3);
    expect(calls[2]?.request.source).toBe('newer-edits-after-failure');
    expect(calls[2]?.request.expectedRevision).toBe(5);

    calls[2]?.settle.resolve(saveOk('n1', 6));
    await awaitMicrotasks(8);
    snapshot = controller.snapshot('n1');
    expect(snapshot?.status).toBe('clean');
    expect(snapshot?.revision).toBe(6);
    expect(uuidCalls).toEqual(['op-1', 'op-2']);
  });

  it('suppresses automatic retry storms: failure schedules no further I/O', async () => {
    const { io, calls } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    controller.setBufferText('n1', 'b');

    const done = controller.requestSave('n1');
    await awaitMicrotasks();
    calls[0]?.settle.resolve({ ok: false, error: serviceError('DISK_FULL', 'op-1') });
    await done;
    await awaitMicrotasks(10);

    expect(calls).toHaveLength(1);
    expect(controller.snapshot('n1')?.status).toBe('failed');
    expect(controller.snapshot('n1')?.dirty).toBe(true);
  });

  it('the next requestSave while failed repeats exactly the failed request instead of a fresh uuid', async () => {
    const { io, calls } = ioWithQueue();
    const { controller, uuidCalls } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    controller.setBufferText('n1', 'b');

    const done = controller.requestSave('n1');
    await awaitMicrotasks();
    calls[0]?.settle.resolve({ ok: false, error: serviceError('IO_DENIED', 'op-1') });
    await done;

    const again = controller.requestSave('n1');
    await awaitMicrotasks(8);
    expect(calls).toHaveLength(2);
    expect(calls[1]?.request.operationId).toBe('op-1');
    expect(calls[1]?.request.source).toBe('b');
    expect(calls[1]?.request.expectedRevision).toBe(1);
    calls[1]?.settle.resolve(saveOk('n1', 4));
    await again;
    expect(uuidCalls).toEqual(['op-1']);
  });
});

describe('SaveController composition guards', () => {
  it('refuses requestSave while composing and never consumes an operation id for it', async () => {
    const { io, calls } = ioWithQueue();
    const { controller, uuidCalls } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    controller.compositionStarted('n1');
    controller.setBufferText('n1', 'half-composed');

    const refused = controller.requestSave('n1');
    await awaitMicrotasks(8);
    expect(calls).toHaveLength(0);
    expect(uuidCalls).toEqual([]);
    expect(controller.snapshot('n1')?.status).toBe('dirty');

    controller.compositionEnded('n1');
    const released = controller.requestSave('n1');
    await awaitMicrotasks(8);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.request.source).toBe('half-composed');
    expect(uuidCalls).toEqual(['op-1']);
    await refused;
    calls[0]?.settle.resolve(saveOk('n1', 2));
    await released;
    expect(controller.snapshot('n1')?.status).toBe('clean');
  });

  it('does not chain a save into an active composition, and never launches a chained save at composition end', async () => {
    const { io, calls } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    controller.setBufferText('n1', 'pre-ime');

    const done = controller.requestSave('n1');
    await awaitMicrotasks();
    controller.compositionStarted('n1');
    controller.setBufferText('n1', 'ime-text');
    calls[0]?.settle.resolve(saveOk('n1', 2));
    await done;
    await awaitMicrotasks(10);

    expect(calls).toHaveLength(1);
    expect(controller.snapshot('n1')?.status).toBe('dirty');
    expect(controller.snapshot('n1')?.revision).toBe(2);

    // The controller never launches the owed save at composition end: the
    // final committed input may not have arrived yet. This belongs to the
    // caller's 1000ms idle workflow once the final text reaches the buffer.
    controller.compositionEnded('n1');
    await awaitMicrotasks(10);
    expect(calls).toHaveLength(1);

    // The idle workflow then asks again after the final input; that save
    // carries the newest buffer text and the latest known revision.
    controller.setBufferText('n1', 'ime-final-text');
    const followUp = controller.requestSave('n1');
    await awaitMicrotasks(8);
    expect(calls).toHaveLength(2);
    expect(calls[1]?.request.source).toBe('ime-final-text');
    expect(calls[1]?.request.expectedRevision).toBe(2);

    calls[1]?.settle.resolve(saveOk('n1', 3));
    await followUp;
    await awaitMicrotasks(8);
    expect(controller.snapshot('n1')?.status).toBe('clean');
  });

  it('registerDocument keeps dirty and in-flight state instead of replacing it', async () => {
    const { io, calls } = ioWithQueue();
    const { controller, uuidCalls } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'base', 1));
    controller.setBufferText('n1', 'typed');
    const done = controller.requestSave('n1');
    await awaitMicrotasks();

    controller.registerDocument(documentOf('n1', 'fresh-from-disk', 9));
    expect(calls).toHaveLength(1);

    calls[0]?.settle.resolve(saveOk('n1', 2));
    await done;
    await awaitMicrotasks(8);
    expect(controller.snapshot('n1')?.revision).toBe(2);
    expect(controller.snapshot('n1')?.status).toBe('clean');
    expect(uuidCalls).toEqual(['op-1']);
  });

  it('registerDocument keeps failed state with newer edits instead of replacing it', async () => {
    const { io, calls } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'base', 1));
    controller.setBufferText('n1', 'typed');
    const first = controller.requestSave('n1');
    await awaitMicrotasks();
    calls[0]?.settle.resolve({ ok: false, error: serviceError('IO_DENIED', 'op-1') });
    await first;
    controller.setBufferText('n1', 'typed-newer');

    controller.registerDocument(documentOf('n1', 'fresh-from-disk', 9));
    expect(calls).toHaveLength(1);

    const snapshot = controller.snapshot('n1');
    expect(snapshot?.status).toBe('failed');
    expect(snapshot?.failedOperation?.operationId).toBe('op-1');
    expect(snapshot?.dirty).toBe(true);
  });

  it('a requestSave during a running flight awaits the same acknowledgement without a second I/O call', async () => {
    const { io, calls } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    controller.setBufferText('n1', 'b');

    const first = controller.requestSave('n1');
    await awaitMicrotasks();
    let secondResolved = false;
    void controller.requestSave('n1').then(() => {
      secondResolved = true;
    });
    await awaitMicrotasks(8);
    expect(secondResolved).toBe(false);
    expect(calls).toHaveLength(1);

    calls[0]?.settle.resolve(saveOk('n1', 2));
    await first;
    await awaitMicrotasks(8);
    expect(secondResolved).toBe(true);
    expect(controller.snapshot('n1')?.status).toBe('clean');
  });
});

describe('SaveController exit attention', () => {
  it('flags failed, dirty and in-flight notes needing attention on exit', async () => {
    const { io, calls } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    controller.registerDocument(documentOf('n2', 'clean', 1));
    controller.setBufferText('n1', 'b');

    void controller.requestSave('n1');
    await awaitMicrotasks();

    expect(controller.notesNeedingAttentionOnExit()).toEqual([{ noteId: 'n1', status: 'saving' }]);

    calls[0]?.settle.resolve({ ok: false, error: serviceError('IO_DENIED', 'op-1') });
    await awaitMicrotasks(8);
    expect(controller.notesNeedingAttentionOnExit().map((entry) => entry.noteId)).toEqual(['n1']);
    expect(controller.snapshot('n2')?.status).toBe('clean');
  });

  it('blocks exit while a clean note is composing; a save attempt neither issues I/O nor authorizes the exit', async () => {
    const { io, calls } = ioWithQueue();
    const { controller, uuidCalls } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    expect(controller.notesNeedingAttentionOnExit()).toEqual([]);

    // No docChanged yet: the buffer still equals the committed source, but
    // the composition alone must block the exit and be worded as dirty.
    controller.compositionStarted('n1');
    expect(controller.notesNeedingAttentionOnExit()).toEqual([{ noteId: 'n1', status: 'dirty' }]);
    expect(controller.snapshot('n1')?.status).toBe('clean');

    const refused = controller.requestSave('n1');
    await awaitMicrotasks(8);
    expect(calls).toHaveLength(0);
    expect(uuidCalls).toEqual([]);
    expect(controller.notesNeedingAttentionOnExit()).toEqual([{ noteId: 'n1', status: 'dirty' }]);
    await refused;

    // Composition end without committed input releases the attention.
    controller.compositionEnded('n1');
    expect(controller.notesNeedingAttentionOnExit()).toEqual([]);

    // A composing note that receives final dirty text stays blocked as dirty.
    controller.compositionStarted('n1');
    controller.setBufferText('n1', 'ime-text');
    expect(controller.notesNeedingAttentionOnExit()).toEqual([{ noteId: 'n1', status: 'dirty' }]);
    controller.compositionEnded('n1');
    expect(controller.notesNeedingAttentionOnExit().map((entry) => entry.noteId)).toEqual(['n1']);
  });

  it('emits an event on composition start and end so exit guards can re-evaluate', () => {
    const { io } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'a', 1));
    const events: ControllerEvent[] = [];
    controller.subscribe((event) => events.push(event));

    controller.compositionStarted('n1');
    controller.compositionEnded('n1');

    expect(events).toHaveLength(2);
    expect(events[0]?.noteId).toBe('n1');
    expect(events[1]?.noteId).toBe('n1');
  });

  it('getBufferText returns the current buffer, including text newer than a retained failed request', async () => {
    const { io, calls } = ioWithQueue();
    const { controller } = controllerFor(io);
    controller.registerDocument(documentOf('n1', 'base', 1));
    expect(controller.getBufferText('n1')).toBe('base');

    controller.setBufferText('n1', 'failed-source');
    const first = controller.requestSave('n1');
    await awaitMicrotasks();
    calls[0]?.settle.resolve({ ok: false, error: serviceError('IO_DENIED', 'op-1') });
    await first;
    controller.setBufferText('n1', 'newer-current-text');

    expect(controller.snapshot('n1')?.failedOperation?.source).toBe('failed-source');
    expect(controller.getBufferText('n1')).toBe('newer-current-text');
    expect(controller.getBufferText('missing')).toBeNull();
  });
});
