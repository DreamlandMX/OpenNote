import type { NoteDocument, SaveResult, ServiceError, ErrorCode } from '../domain';
import type { SaveNoteRequest, RestoreHistoryRequest, ServiceOutcome } from './ports';

export type SaveStatus = 'clean' | 'dirty' | 'saving' | 'failed' | 'conflict';

export interface SaveOperationRequest {
  operationId: string;
  source: string;
  expectedRevision: number;
  historyRevisionId?: string;
}

export interface NoteSaveSnapshot {
  noteId: string;
  status: SaveStatus;
  dirty: boolean;
  inFlight: boolean;
  revision: number;
  contentHash: string;
  error: ServiceError | null;
  failedOperation: SaveOperationRequest | null;
}

export interface ControllerEvent {
  noteId: string;
  snapshot: NoteSaveSnapshot;
}

export interface SaveControllerIo {
  restoreHistory?(request: RestoreHistoryRequest): Promise<ServiceOutcome<SaveResult>>;
  saveNote(request: SaveNoteRequest): Promise<
    { ok: true; value: SaveResult } | { ok: false; error: ServiceError }
  >;
}

export type SaveControllerConfig = {
  uuid: () => string;
};

const KNOWN_ERROR_CODES: readonly ErrorCode[] = [
  'INVALID_INPUT',
  'UNSUPPORTED_SCHEMA',
  'OBJECT_NOT_FOUND',
  'VERSION_CONFLICT',
  'IDEMPOTENCY_MISMATCH',
  'IO_DENIED',
  'DISK_FULL',
  'FILE_LOCKED',
  'RESCAN_REQUIRED',
  'NOT_IMPLEMENTED',
  'HISTORY_MISSING',
  'PATH_COLLISION',
];

// Unknown codes/thrown failures normalize to IO_DENIED: the note buffer is
// still retained in memory by this controller (vault persistence is not yet
// implemented), and the failure surfaces as an I/O problem, never as an
// accidental "not implemented" that would mislabel local-content failures.
function normalizeServiceError(error: ServiceError): ServiceError {
  const code = KNOWN_ERROR_CODES.includes(error.code) ? error.code : 'IO_DENIED';
  const params: Record<string, string> = {};
  if (error.params && typeof error.params === 'object') {
    for (const [key, value] of Object.entries(error.params)) {
      if (typeof value === 'string') {
        params[key] = value;
      }
    }
  }
  return {
    code,
    messageKey: typeof error.messageKey === 'string' ? error.messageKey : 'opennote.service.error',
    params,
    retryable: error.retryable === true,
    operationId: typeof error.operationId === 'string' ? error.operationId : null,
    retryAfterSeconds:
      typeof error.retryAfterSeconds === 'number' && Number.isFinite(error.retryAfterSeconds)
        ? error.retryAfterSeconds
        : null,
  };
}

function safeServiceErrorFromThrown(thrown: unknown): ServiceError {
  if (
    thrown !== null &&
    typeof thrown === 'object' &&
    'code' in thrown &&
    typeof (thrown as { code: unknown }).code === 'string'
  ) {
    const candidate = (thrown as { code: string }).code;
    if (KNOWN_ERROR_CODES.includes(candidate as ErrorCode)) {
      return normalizeServiceError(thrown as ServiceError);
    }
  }
  return {
    code: 'IO_DENIED',
    messageKey: 'opennote.service.thrownFailure',
    params: {},
    retryable: false,
    operationId: null,
    retryAfterSeconds: null,
  };
}

interface NoteSaveState {
  bufferText: string;
  revision: number;
  contentHash: string;
  committedSource: string;
  status: SaveStatus;
  error: ServiceError | null;
  failedRequest: SaveOperationRequest | null;
  inFlight: SaveOperationRequest | null;
  flightPromise: Promise<void> | null;
  composing: boolean;
  chainedSavePendingComposition: boolean;
}

export type ControllerListener = (event: ControllerEvent) => void;

export class SaveController {
  private readonly io: SaveControllerIo;
  private readonly uuid: () => string;
  private readonly notes = new Map<string, NoteSaveState>();
  private readonly listeners = new Set<ControllerListener>();

  constructor(io: SaveControllerIo, config: SaveControllerConfig) {
    this.io = io;
    this.uuid = config.uuid;
  }

  subscribe(listener: ControllerListener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  registerDocument(doc: NoteDocument): void {
    const existing = this.notes.get(doc.noteId);
    if (existing) {
      if (
        existing.inFlight !== null ||
        existing.failedRequest !== null ||
        existing.bufferText !== existing.committedSource
      ) {
        return;
      }
      existing.revision = doc.localRevision;
      existing.contentHash = doc.contentHash;
      existing.committedSource = doc.source;
      existing.bufferText = doc.source;
      existing.status = 'clean';
      existing.error = null;
      existing.failedRequest = null;
      this.emit(doc.noteId);
      return;
    }
    this.notes.set(doc.noteId, {
      bufferText: doc.source,
      revision: doc.localRevision,
      contentHash: doc.contentHash,
      committedSource: doc.source,
      status: 'clean',
      error: null,
      failedRequest: null,
      inFlight: null,
      flightPromise: null,
      composing: false,
      chainedSavePendingComposition: false,
    });
    this.emit(doc.noteId);
  }

  setBufferText(noteId: string, text: string): void {
    const state = this.notes.get(noteId);
    if (!state || state.bufferText === text) {
      return;
    }
    state.bufferText = text;
    if (state.inFlight === null) {
      state.status = state.failedRequest !== null
        ? 'failed'
        : state.bufferText === state.committedSource
          ? 'clean'
          : 'dirty';
    }
    this.emit(noteId);
  }

  snapshot(noteId: string): NoteSaveSnapshot | null {
    const state = this.notes.get(noteId);
    if (!state) {
      return null;
    }
    return this.snapshotOf(state, noteId);
  }

  // Authoritative current buffer text for a registered note, including edits
  // that have not been flushed to the editor rendering yet (e.g. the final
  // committed composition input). Null for an unknown note.
  getBufferText(noteId: string): string | null {
    const state = this.notes.get(noteId);
    if (!state) {
      return null;
    }
    return state.bufferText;
  }

  isComposing(noteId: string): boolean {
    return this.notes.get(noteId)?.composing ?? false;
  }

  private snapshotOf(state: NoteSaveState, noteId: string): NoteSaveSnapshot {
    const dirty = state.bufferText !== state.committedSource;
    const status: SaveStatus =
      state.status === 'failed' || state.status === 'conflict'
        ? state.status
        : state.inFlight !== null
          ? 'saving'
          : dirty
            ? 'dirty'
            : 'clean';
    return {
      noteId,
      status,
      dirty,
      inFlight: state.inFlight !== null,
      revision: state.revision,
      contentHash: state.contentHash,
      error: state.error,
      failedOperation: state.failedRequest,
    };
  }

  private emit(noteId: string): void {
    const snapshot = this.snapshot(noteId);
    if (!snapshot) {
      return;
    }
    const event: ControllerEvent = { noteId, snapshot };
    for (const listener of this.listeners) {
      listener(event);
    }
  }

  notesNeedingAttentionOnExit(): readonly { noteId: string; status: SaveStatus }[] {
    const blocked: { noteId: string; status: SaveStatus }[] = [];
    for (const [noteId, state] of this.notes) {
      const snapshot = this.snapshotOf(state, noteId);
      if (
        state.composing ||
        snapshot.dirty ||
        snapshot.inFlight ||
        snapshot.status === 'failed' ||
        snapshot.status === 'conflict'
      ) {
        // An active composition blocks attention even while the buffer still
        // equals the committed source: the final committed input may still
        // arrive and is not yet durable anywhere. A clean-composing entry is
        // therefore reported as dirty for guard wording; any other status is
        // reported as-is.
        const status: SaveStatus = snapshot.status === 'clean' ? 'dirty' : snapshot.status;
        blocked.push({ noteId, status });
      }
    }
    return blocked;
  }

  async requestSave(noteId: string): Promise<void> {
    const state = this.notes.get(noteId);
    if (!state) {
      return;
    }
    if (state.status === 'conflict') {
      return;
    }
    // Never start a flight while an composition is active: the buffer may be
    // missing the final committed input until the CM DOM flush observes it
    // (which lands after compositionend). Explicit saves deferring upstream
    // call us again once the composition state has been released.
    if (state.composing) {
      return;
    }
    if (state.inFlight !== null) {
      return state.flightPromise ?? undefined;
    }
    if (state.failedRequest !== null) {
      return this.runFlight(noteId, state.failedRequest);
    }
    if (state.bufferText === state.committedSource) {
      return;
    }
    const request: SaveOperationRequest = {
      operationId: this.uuid(),
      source: state.bufferText,
      expectedRevision: state.revision,
    };
    return this.runFlight(noteId, request);
  }

  async requestHistoryRestore(noteId: string, historical: NoteDocument, revisionId: string): Promise<boolean> {
    const state = this.notes.get(noteId);
    if (!state || historical.noteId !== noteId || state.composing || this.snapshot(noteId)?.status !== 'clean') return false;
    state.bufferText = historical.source;
    await this.runFlight(noteId, { operationId: this.uuid(), source: historical.source, expectedRevision: state.revision, historyRevisionId: revisionId });
    return this.snapshot(noteId)?.status === 'clean';
  }

  async overwriteExternalConfirmed(noteId: string, externalRevision: number): Promise<void> {
    const state = this.notes.get(noteId);
    if (!state) {
      return;
    }
    if (state.inFlight !== null) {
      return state.flightPromise ?? undefined;
    }
    if (state.bufferText === state.committedSource) {
      state.revision = Math.max(state.revision, externalRevision);
      state.status = 'clean';
      state.error = null;
      state.failedRequest = null;
      this.emit(noteId);
      return;
    }
    const request: SaveOperationRequest = {
      operationId: this.uuid(),
      source: state.bufferText,
      expectedRevision: externalRevision,
    };
    return this.runFlight(noteId, request);
  }

  async retryFailed(noteId: string): Promise<void> {
    const state = this.notes.get(noteId);
    if (!state) {
      return;
    }
    if (state.status === 'conflict') {
      return;
    }
    if (state.inFlight !== null) {
      return state.flightPromise ?? undefined;
    }
    const previous = state.failedRequest;
    if (previous === null) {
      return;
    }
    return this.runFlight(noteId, previous);
  }

  keepLocalVersion(noteId: string): void {
    const state = this.notes.get(noteId);
    if (!state) {
      return;
    }
    state.error = null;
    state.failedRequest = null;
    state.status = state.bufferText === state.committedSource ? 'clean' : 'dirty';
    this.emit(noteId);
  }

  reportExternalConflict(noteId: string): boolean {
    const state = this.notes.get(noteId);
    if (!state || state.inFlight || state.composing) return false;
    state.status = 'conflict';
    state.failedRequest = null;
    state.error = { code: 'VERSION_CONFLICT', messageKey: 'error.externalChange', params: {}, retryable: false, operationId: null, retryAfterSeconds: null };
    this.emit(noteId);
    return true;
  }

  adoptExternalContent(noteId: string, external: NoteDocument): void {
    const state = this.notes.get(noteId);
    if (state && state.inFlight !== null) {
      const current = state;
      void Promise.resolve(current.flightPromise).then(() => {
        const now = this.notes.get(noteId);
        if (now !== current) {
          return;
        }
        if (now && now.inFlight === null) {
          this.resetStateToDocument(now, external);
          this.emit(noteId);
        }
      });
      return;
    }
    if (state) {
      this.resetStateToDocument(state, external);
    } else {
      this.registerDocument(external);
    }
    this.emit(noteId);
  }

  private resetStateToDocument(state: NoteSaveState, doc: NoteDocument): void {
    state.bufferText = doc.source;
    state.revision = doc.localRevision;
    state.contentHash = doc.contentHash;
    state.committedSource = doc.source;
    state.status = 'clean';
    state.error = null;
    state.failedRequest = null;
    state.chainedSavePendingComposition = false;
  }

  restoreDraft(noteId: string, source: string): boolean {
    const state = this.notes.get(noteId);
    if (!state || state.composing || state.inFlight !== null || state.failedRequest !== null || state.bufferText !== state.committedSource) {
      return false;
    }
    state.bufferText = source;
    state.status = state.bufferText === state.committedSource ? 'clean' : 'dirty';
    state.error = null;
    state.failedRequest = null;
    this.emit(noteId);
    return true;
  }

  compositionStarted(noteId: string): void {
    const state = this.notes.get(noteId);
    if (!state) {
      return;
    }
    state.composing = true;
    this.emit(noteId);
  }

  compositionEnded(noteId: string): void {
    const state = this.notes.get(noteId);
    if (!state) {
      return;
    }
    state.composing = false;
    // A save owed at composition end is never launched here: at this point the
    // browser may not yet have flushed the composition's final committed input
    // to the controller, so the follow-up save is routed through the caller's
    // 1000ms idle workflow when the final buffer text arrives.
    state.chainedSavePendingComposition = false;
    this.emit(noteId);
  }

  private runFlight(noteId: string, request: SaveOperationRequest): Promise<void> {
    const state = this.notes.get(noteId);
    if (!state || state.inFlight !== null) {
      return Promise.resolve();
    }
    state.failedRequest = null;
    state.error = null;
    state.inFlight = request;
    state.status = 'saving';
    this.emit(noteId);
    const flight: Promise<void> = Promise.resolve()
      .then(() => {
        if (request.historyRevisionId !== undefined) {
          return this.io.restoreHistory?.({ noteId, revisionId: request.historyRevisionId, expectedRevision: request.expectedRevision, operationId: request.operationId }) ??
            Promise.resolve({ ok: false as const, error: safeServiceErrorFromThrown({ code: 'NOT_IMPLEMENTED' }) });
        }
        return this.io.saveNote({
          noteId,
          source: request.source,
          expectedRevision: request.expectedRevision,
          operationId: request.operationId,
        });
      })
      .then(
        (outcome) => {
          if (this.notes.get(noteId) !== state || state.inFlight !== request) {
            return;
          }
          state.inFlight = null;
          state.flightPromise = null;
          if (outcome.ok) {
            state.revision = outcome.value.localRevision;
            state.contentHash = outcome.value.contentHash;
            state.committedSource = request.source;
            if (state.bufferText !== request.source) {
              state.status = 'dirty';
              this.emit(noteId);
              this.considerChainedSave(noteId, state);
              return;
            }
            state.status = 'clean';
          } else {
            state.error = normalizeServiceError(outcome.error);
            state.failedRequest = request;
            state.status = outcome.error.code === 'VERSION_CONFLICT' ? 'conflict' : 'failed';
          }
          this.emit(noteId);
        },
        (thrown: unknown) => {
          if (this.notes.get(noteId) !== state || state.inFlight !== request) {
            return;
          }
          state.inFlight = null;
          state.flightPromise = null;
          state.error = safeServiceErrorFromThrown(thrown);
          state.failedRequest = request;
          state.status = 'failed';
          this.emit(noteId);
        },
      );
    state.flightPromise = flight;
    return flight;
  }

  private considerChainedSave(noteId: string, state: NoteSaveState): void {
    if (state.bufferText === state.committedSource) {
      return;
    }
    if (state.composing) {
      // Deferred: the follow-up save is the caller's idle-workflow job once
      // the composition ends and its final committed text has been applied.
      state.chainedSavePendingComposition = true;
      return;
    }
    void this.requestSave(noteId);
  }
}
