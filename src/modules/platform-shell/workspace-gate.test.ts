import { describe, expect, it } from 'vitest';
import { SaveController } from '../editor';
import type { NoteService, ServiceOutcome } from '../editor';
import type { NoteDocument, SaveResult, ServiceError } from '../domain';
import { createWorkspaceGate } from './WorkspaceScreen';
import { NoteCreationController } from './note-creation';

it('waits for an active trash mutation and never treats export as permission to exit mid-operation', async () => {
  const { gate, setPending, exports } = buildHarness();
  let finish!: () => void;
  const done = new Promise<void>(resolve => { finish = resolve; });
  setPending({ noteId: 'trash-note', path: 'note.md', done });
  expect(gate.blockedEntries()).toEqual([{ noteId: 'trash-note', path: 'note.md', status: 'working' }]);
  expect(await gate.exportBlocked()).toBe('retained');
  expect(exports).toHaveLength(0);
  let settled = false;
  const waiting = gate.saveBlockedAndWait().then(result => { settled = true; return result; });
  await awaitMicrotasks(); expect(settled).toBe(false);
  setPending(null); finish();
  expect(await waiting).toBe(true);
  expect(gate.blockedEntries()).toEqual([]);
});

interface PendingSave {
  request: { noteId: string; source: string; expectedRevision: number; operationId: string };
  settle: (outcome: ServiceOutcome<SaveResult>) => void;
}

interface PendingExport {
  relativePath: string;
  source: string;
  resolve: (outcome: ServiceOutcome<{ outcome: 'saved' | 'cancelled' }>) => void;
}

function awaitMicrotasks(times = 8): Promise<void> {
  let running: Promise<void> = Promise.resolve();
  for (let index = 0; index < times; index += 1) {
    running = running.then(() => Promise.resolve());
  }
  return running;
}

function serviceError(code: ServiceError['code']): ServiceError {
  return {
    code,
    messageKey: 'opennote.service.test',
    params: {},
    retryable: true,
    operationId: null,
    retryAfterSeconds: null,
  };
}

function documentOf(noteId: string, source: string, localRevision: number): NoteDocument {
  return {
    noteId,
    vaultId: 'v1',
    relativePath: 'notes/n1.md',
    source,
    localRevision,
    contentHash: `hash-${localRevision}`,
    readOnly: false,
  };
}

function failureOutcome(): ServiceOutcome<SaveResult> {
  return { ok: false as const, error: serviceError('IO_DENIED') };
}

function successOutcome(localRevision: number): ServiceOutcome<SaveResult> {
  return {
    ok: true as const,
    value: { noteId: 'n1', localRevision, contentHash: `hash-${localRevision}` },
  };
}

function buildHarness() {
  const saves: PendingSave[] = [];
  const exports: PendingExport[] = [];
  const buffers: Record<string, string> = { n1: 'draft-body' };
  const paths = new Map([['n1', 'notes/n1.md']]);
  let uuidSequence = 0;

  const controller = new SaveController(
    {
      saveNote(request) {
        return new Promise<ServiceOutcome<SaveResult>>((resolve) => {
          saves.push({ request, settle: resolve });
        });
      },
    },
    {
      uuid: () => {
        uuidSequence += 1;
        return `op-${uuidSequence}`;
      },
    },
  );

  const noteService: NoteService = {
    refreshCatalog: async () => ({ ok: true, value: { notes: [], changes: [], unavailableNoteIds: [], findings: [], fingerprint: 'empty', deferred: false, persisted: true, writable: true } }),
    watchCatalogChanges: async () => ({ ok: true, value: async () => undefined }),
    watchSourceChanges: async () => ({ ok: true, value: async () => undefined }),
    previewTagUpdate: async () => { throw Error('unused'); },
    applyTagUpdate: async () => { throw Error('unused'); },
    resumeTagUpdate: async () => { throw Error('unused'); },
    cancelTagUpdate: async () => { throw Error('unused'); },
    listTagUpdates: async () => ({ ok: true, value: [] }),
    rename: async () => { throw Error('unused'); },
    listTrash: async () => ({ ok: true, value: [] }),
    readTrash: async () => { throw Error('unused'); },
    deleteNote: async () => { throw Error('unused'); },
    restoreTrash: async () => { throw Error('unused'); },
    purgeTrash: async () => { throw Error('unused'); },
    listTrashOperations: async () => ({ ok: true, value: [] }),
    resumeTrash: async () => { throw Error('unused'); },
    cancelTrash: async () => { throw Error('unused'); },
    createCopy: async () => { throw new Error('unused'); },
    prepareConflict: async () => { throw new Error('unused'); },
    listNotes() {
      return Promise.resolve({ ok: true as const, value: [] });
    },
    listFolders() {
      return Promise.resolve({ ok: true as const, value: [] });
    },
    createFolder: async () => { throw new Error('unused'); },
    inspectFolder: async () => { throw new Error('unused'); },
    removeEmptyFolder: async () => { throw new Error('unused'); },
    recycleFolder: async () => { throw new Error('unused'); },
    renameEmptyFolder: async () => { throw new Error('unused'); },
    readNote() {
      return Promise.resolve({ ok: false as const, error: serviceError('OBJECT_NOT_FOUND') });
    },
    createNote() {
      return Promise.resolve({ ok: false as const, error: serviceError('OBJECT_NOT_FOUND') });
    },
    saveNote(request) {
      return new Promise<ServiceOutcome<SaveResult>>((resolve) => {
        saves.push({ request, settle: resolve });
      });
    },
    recoverVault() {
      return Promise.resolve({ ok: true as const, value: { applied: [], pending: [] } });
    },
    writeCheckpoint() {
      return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') });
    },
    listDrafts() {
      return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') });
    },
    readDraft() {
      return Promise.resolve({ ok: false as const, error: serviceError('OBJECT_NOT_FOUND') });
    },
    listHistory() { return Promise.resolve({ ok: true as const, value: { items: [], unavailableRevisionIds: [] } }); },
    readHistory() { return Promise.resolve({ ok: false as const, error: serviceError('HISTORY_MISSING') }); },
    restoreHistory() { return Promise.resolve({ ok: false as const, error: serviceError('HISTORY_MISSING') }); },
    historyRetentionSettings() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    setHistoryRetention() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    previewHistoryCleanup() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    applyHistoryCleanup() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
  historyCleanupStatus() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
  cancelHistoryCleanup() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
  stopHistoryCleanup() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    exportDraft(request) {
      return new Promise<ServiceOutcome<{ outcome: 'saved' | 'cancelled' }>>((resolve) => {
        exports.push({
          relativePath: request.relativePath,
          source: request.source,
          resolve,
        });
      });
    },
    listTags() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    listBacklinks() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    listLinkIssues() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    readNoteMetadata() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    startSearch: async () => { throw Error('unused'); },
    searchJobStatus: async () => { throw Error('unused'); },
    cancelSearch: async () => { throw Error('unused'); },
    searchNotes() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    parseHeadings() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    applyPropertyEdit() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    parseProperties() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    exportVault() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    importPrepare() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    importStart() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    importStatus() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    importCancel() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    importUnfinished() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    importDiscard() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    listTasks() { return Promise.resolve({ ok: true as const, value: [] }); },
    toggleTask() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    deleteTask() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    setTaskDue() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    expandTemplate() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    openDailyNote() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    graphReport() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    databaseTable() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    parseCanvas() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    parseTexLog() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    previewState() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    planLatexExport() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    planLatexScaffold() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    detectLatexLanguage() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    planLatexInsertion() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    parseBibliography() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    inspectLatexProject() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    planTagRename() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    previewRenameCommit() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    planAttachmentImport() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    importAttachments() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    attachmentInfo() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    readAttachment() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    storeAttachmentBytes() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    listVaultPdfs() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    saveNoteHtml() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    openAttachmentWithSystem() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    listCanvases() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    readCanvas() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    saveCanvas() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    listViews() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    readView() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    saveView() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    classifyAttachment() { return Promise.resolve({ ok: true as const, value: ['unknown', 'download'] as [string, string] }); },
    planTagRemoval() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    planCitation() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    planCitationRename() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    bibliographyDangling() { return Promise.resolve({ ok: true as const, value: [] }); },
    bibliographyUnicode() { return Promise.resolve({ ok: true as const, value: [] }); },
    latexLabels() { return Promise.resolve({ ok: true as const, value: [] }); },
    latexToggleComment() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    latexWrapSelection() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    latexMatchingBracket() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    latexCompletions() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    latexApplyCompletion() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    escapeLatexText() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    diagnosticsHostFacts() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    diagnosticsProbe() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    diagnosticsPreview() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    diagnosticsExportText() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
  diagnosticsExport() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    diagnosticsOverall() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    listCompileJobs() { return Promise.resolve({ ok: true as const, value: [] }); },
    cancelCompileJob() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    redactTexLog() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    serializeCanvas() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    listTemplates() { return Promise.resolve({ ok: true as const, value: [] }); },
    expandNoteEmbed() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
    previewNoteRename() { return Promise.resolve({ ok: false as const, error: serviceError('NOT_IMPLEMENTED') }); },
  };

  const gate = createWorkspaceGate({
    saveController: controller,
    noteService,
    pathOf: (noteId) => paths.get(noteId),
    bufferOf: (noteId) => buffers[noteId],
    pendingOperation: () => pending,
  });

  let pending: { noteId: string; path: string; done: Promise<void> } | null = null;
  return { controller, noteService, gate, saves, exports, setPending: (value: typeof pending) => { pending = value; } };
}

async function seedFailedSnapshot(
  controller: SaveController,
  saves: PendingSave[],
): Promise<void> {
  controller.registerDocument(documentOf('n1', 'base', 1));
  controller.setBufferText('n1', 'draft-body');
  const done = controller.requestSave('n1');
  await awaitMicrotasks();
  saves[0]?.settle(failureOutcome());
  await done;
  await awaitMicrotasks();
}

describe('workspace gate blocks exit on a failed save acknowledgement', () => {
  it('refuses close/switch after requestSave resolved because the snapshot is still failed', async () => {
    const { controller, gate, saves } = buildHarness();
    await seedFailedSnapshot(controller, saves);

    expect(gate.blockedEntries()).toEqual([
      { noteId: 'n1', path: 'notes/n1.md', status: 'failed' },
    ]);

    const pending = gate.saveBlockedAndWait();
    await awaitMicrotasks();
    saves[1]?.settle(failureOutcome());
    const permitted = await pending;
    await awaitMicrotasks();

    expect(permitted).toBe(false);
    expect(saves).toHaveLength(2);
    expect(saves[1]?.request.source).toBe('draft-body');
    expect(saves[1]?.request.expectedRevision).toBe(1);
    expect(saves[1]?.request.operationId).toBe('op-1');
    expect(controller.snapshot('n1')?.status).toBe('failed');
    expect(controller.snapshot('n1')?.dirty).toBe(true);
  });

  it('permits close/switch only when the recheck after save-and-wait is fully clean', async () => {
    const { controller, gate, saves } = buildHarness();
    await seedFailedSnapshot(controller, saves);

    const pending = gate.saveBlockedAndWait();
    await awaitMicrotasks();
    saves[1]?.settle(failureOutcome());
    expect(await pending).toBe(false);
    await awaitMicrotasks();
    expect(controller.snapshot('n1')?.status).toBe('failed');

    const retry = gate.saveBlockedAndWait();
    await awaitMicrotasks();
    saves[2]?.settle(successOutcome(2));
    const permitted = await retry;
    await awaitMicrotasks();

    expect(permitted).toBe(true);
    expect(gate.blockedEntries()).toEqual([]);
    expect(controller.snapshot('n1')?.status).toBe('clean');
    expect(controller.snapshot('n1')?.revision).toBe(2);
  });
});

describe('workspace gate export keeps the buffer on cancellation or failure', () => {
  it('a cancelled export returns retained and every snapshot stays blocked', async () => {
    const { controller, gate, exports } = buildHarness();
    controller.registerDocument(documentOf('n1', 'base', 1));
    controller.setBufferText('n1', 'draft-body');
    const pending = gate.exportBlocked();

    expect(exports).toHaveLength(1);
    expect(exports[0]?.source).toBe('draft-body');
    expect(exports[0]?.relativePath).toBe('notes/n1.md');

    exports[0]?.resolve({ ok: true as const, value: { outcome: 'cancelled' as const } });
    const outcome = await pending;
    await awaitMicrotasks();

    expect(outcome).toBe('retained');
    expect(controller.snapshot('n1')?.dirty).toBe(true);
    expect(gate.blockedEntries()).toHaveLength(1);
  });

  it('a failed export returns retained and keeps the unsaved text', async () => {
    const { controller, gate, exports } = buildHarness();
    controller.registerDocument(documentOf('n1', 'base', 1));
    controller.setBufferText('n1', 'draft-body');

    const pending = gate.exportBlocked();
    expect(exports).toHaveLength(1);
    exports[0]?.resolve({ ok: false as const, error: serviceError('INVALID_INPUT') });
    const outcome = await pending;
    await awaitMicrotasks();

    expect(outcome).toBe('retained');
    expect(controller.snapshot('n1')?.dirty).toBe(true);
    expect(gate.blockedEntries()).toHaveLength(1);
  });

  it('even fully exported copies leave the snapshots blocked; only saved acks permit', async () => {
    const { controller, gate, exports } = buildHarness();
    controller.registerDocument(documentOf('n1', 'base', 1));
    controller.setBufferText('n1', 'draft-body');

    const pending = gate.exportBlocked();
    exports[0]?.resolve({ ok: true as const, value: { outcome: 'saved' as const } });
    const outcome = await pending;
    await awaitMicrotasks();

    expect(outcome).toBe('exported');
    expect(controller.snapshot('n1')?.dirty).toBe(true);
    expect(gate.blockedEntries()).toHaveLength(1);
  });
});


describe('pending note creation lifecycle', () => {
  function setup() {
    const harness = buildHarness();
    const calls: { relativePath: string; source: string; operationId: string }[] = [];
    let allowed = true;
    const creation = new NoteCreationController({
      newOperationId: () => 'creation-operation', canCreate: () => allowed,
      createNote: request => { calls.push({ ...request }); return harness.noteService.createNote(request); },
    });
    const gate = createWorkspaceGate({ saveController: harness.controller, noteService: harness.noteService,
      pathOf: () => 'old.md', bufferOf: id => harness.controller.getBufferText(id) ?? undefined,
      pendingOperation: () => null, creation });
    return { ...harness, gate, creation, calls, setAllowed: (value: boolean) => { allowed = value; } };
  }

  it('blocks exit and export while creation runs, joining rather than starting another request', async () => {
    const h = setup();
    let finish!: (outcome: ServiceOutcome<NoteDocument>) => void;
    h.noteService.createNote = () => new Promise(resolve => { finish = resolve; });
    const creating = h.creation.create('Template.md', 'expanded body');
    expect(h.gate.blockedEntries()).toEqual([{ noteId: 'creation:creation-operation', path: 'Template.md', status: 'working' }]);
    expect(await h.gate.exportBlocked()).toBe('retained');
    expect(h.exports).toHaveLength(0);
    let exited = false;
    const waiting = h.gate.saveBlockedAndWait().then(value => { exited = value; return value; });
    await awaitMicrotasks();
    expect(exited).toBe(false);
    expect(h.calls).toHaveLength(1);
    finish({ ok: true, value: documentOf('new-note', 'expanded body', 0) });
    await creating;
    expect(await waiting).toBe(true);
    expect(h.gate.blockedEntries()).toEqual([]);
  });

  it('retries the exact failed creation before waiting for a dirty existing note to save', async () => {
    const h = setup();
    let attempt = 0;
    h.noteService.createNote = async () => ++attempt === 1
      ? { ok: false, error: serviceError('IO_DENIED') }
      : { ok: true, value: documentOf('new-note', 'expanded body', 0) };
    await h.creation.create('Template.md', 'expanded body');
    h.controller.registerDocument(documentOf('old', 'base', 1));
    h.controller.setBufferText('old', 'old unsaved words');
    expect(h.gate.blockedEntries().find(row => row.noteId.startsWith('creation:'))?.status).toBe('failed');
    const waiting = h.gate.saveBlockedAndWait();
    await awaitMicrotasks(20);
    expect(h.calls[1]).toEqual(h.calls[0]);
    expect(h.saves).toHaveLength(1);
    expect(h.saves[0]?.request.source).toBe('old unsaved words');
    h.saves[0]?.settle(successOutcome(2));
    expect(await waiting).toBe(true);
  });

  it('exports the retained raw template text without marking the original creation resolved', async () => {
    const h = setup();
    h.noteService.createNote = async () => ({ ok: false, error: serviceError('IO_DENIED') });
    await h.creation.create('Template.md', 'expanded body');
    const exporting = h.gate.exportBlocked();
    expect(h.exports[0]?.source).toBe('expanded body');
    expect(h.exports[0]?.relativePath).toBe('Template.md');
    h.exports[0]?.resolve({ ok: true, value: { outcome: 'saved' } });
    expect(await exporting).toBe('exported');
    expect(h.creation.snapshot().request?.operationId).toBe('creation-operation');
    expect(h.gate.blockedEntries()).toHaveLength(1);
  });

  it('retains creation after cancelled export or lost write permission', async () => {
    const h = setup();
    h.noteService.createNote = async () => ({ ok: false, error: serviceError('IO_DENIED') });
    await h.creation.create('Template.md', 'expanded body');
    const exporting = h.gate.exportBlocked();
    h.exports[0]?.resolve({ ok: true, value: { outcome: 'cancelled' } });
    expect(await exporting).toBe('retained');
    h.setAllowed(false);
    expect(await h.gate.saveBlockedAndWait()).toBe(false);
    expect(h.calls).toHaveLength(1);
    expect(h.creation.snapshot().request?.source).toBe('expanded body');
  });
});
