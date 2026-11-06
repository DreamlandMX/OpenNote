import type {
  NoteService,
  PropertyValue,
  SaveController,
  ServiceOutcome,
} from '../editor';
import type { ServiceError } from '../domain';

export interface DatabaseCellWriteIo {
  service: Pick<NoteService, 'applyPropertyEdit' | 'readNote'>;
  controller: SaveController;
  canWrite(noteId: string): boolean;
  /** Feeds the controller's edited source back to the editor/checkpoints. */
  onSourceApplied(noteId: string, source: string): void;
}

export interface DatabaseCellWriteRequest {
  noteId: string;
  sourceRevision: number;
  sourceHash: string;
  readOnly: boolean;
  name: string;
  value: PropertyValue;
  retryFailed?: boolean;
}

function failure(code: ServiceError['code'], messageKey: string): ServiceError {
  return {
    code,
    messageKey,
    params: {},
    retryable: false,
    operationId: null,
    retryAfterSeconds: null,
  };
}

function saveFailed(): ServiceError {
  return failure('IO_DENIED', 'opennote.service.error');
}

/**
 * F23-R02/F23-A01: writes one property cell through the normal save path.
 *
 * A registered note is edited in its current buffer, so unsaved edits are kept
 * and the controller owns revision checks, failed requests and conflict state.
 * An unopened note joins that same path after a revision-bound disk read.
 */
export async function writeDatabaseCell(
  io: DatabaseCellWriteIo,
  request: DatabaseCellWriteRequest,
): Promise<ServiceOutcome<{ changed: boolean }>> {
  let current = io.controller.snapshot(request.noteId);
  const conflict = (): ServiceOutcome<never> => ({
    ok: false, error: failure('VERSION_CONFLICT', 'database.sourceChanged'),
  });
  if (!io.canWrite(request.noteId) || current?.inFlight || io.controller.isComposing(request.noteId)) return conflict();
  if (request.readOnly || (current !== null &&
      (current.revision !== request.sourceRevision || current.contentHash !== request.sourceHash))) return conflict();
  let recovered = false;
  if (request.retryFailed && current?.status === 'failed') {
    await io.controller.retryFailed(request.noteId);
    current = io.controller.snapshot(request.noteId);
    recovered = current?.status === 'clean';
  }
  if (current?.status === 'failed' || current?.status === 'conflict') {
    return { ok: false, error: current.error ?? saveFailed() };
  }
  if (!io.canWrite(request.noteId) || current?.inFlight || io.controller.isComposing(request.noteId)) return conflict();
  const disk = current === null ? await io.service.readNote(request.noteId) : null;
  if (disk !== null && !disk.ok) return disk;
  const document = disk?.ok ? disk.value : null;
  if (document && (document.noteId !== request.noteId || document.readOnly ||
      document.localRevision !== request.sourceRevision || document.contentHash !== request.sourceHash)) return conflict();
  const source = document?.source ?? io.controller.getBufferText(request.noteId);
  if (source === null) return { ok: false, error: failure('OBJECT_NOT_FOUND', 'opennote.service.error') };
  const unchanged = (): boolean => {
    const after = io.controller.snapshot(request.noteId);
    return io.canWrite(request.noteId) && !io.controller.isComposing(request.noteId) &&
      (current === null ? after === null : after !== null && !after.inFlight &&
        (after.status === 'clean' || after.status === 'dirty') &&
        after.revision === current.revision && after.contentHash === current.contentHash &&
        io.controller.getBufferText(request.noteId) === source);
  };
  if (!unchanged()) return conflict();
  const edited = await io.service.applyPropertyEdit(source, request.name, request.value);
  if (!edited.ok) return edited;
  // The editor can change while the pure IPC command is running.
  if (!unchanged()) return conflict();
  if (!edited.value.changed) return { ok: true, value: { changed: recovered } };
  if (document) io.controller.registerDocument(document);
  io.controller.setBufferText(request.noteId, edited.value.source);
  io.onSourceApplied(request.noteId, edited.value.source);
  await io.controller.requestSave(request.noteId);
  const after = io.controller.snapshot(request.noteId);
  if (after?.status !== 'clean') {
    return { ok: false, error: after?.error ?? saveFailed() };
  }
  return { ok: true, value: { changed: true } };
}
