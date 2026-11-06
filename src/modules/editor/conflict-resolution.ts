import type { NoteDocument } from '../domain';
import type { CheckpointRequest, CreateCopyRequest, NoteService, ServiceOutcome } from './ports';
import { sameDiskVersion } from './draft-recovery';

type Resolution = { kind: 'changed' | 'resolved'; disk: NoteDocument; copy: NoteDocument | null };
type ConflictIo = Pick<NoteService, 'prepareConflict' | 'writeCheckpoint' | 'createCopy'>;

export class ConflictSession {
  private readonly checkpoint: CheckpointRequest;
  private copyRequest: CreateCopyRequest | null = null;
  private copied: NoteDocument | null = null;
  private running = false;

  constructor(private readonly io: ConflictIo, private readonly local: NoteDocument, private readonly uuid: () => string) {
    this.checkpoint = { noteId: local.noteId, relativePath: local.relativePath, source: local.source, baseRevision: local.localRevision, baseContentHash: local.contentHash, operationId: uuid() };
  }
  get pendingCopyPath(): string | null { return this.copyRequest?.relativePath ?? null; }

  async resolve(preview: NoteDocument, copyPath?: string): Promise<ServiceOutcome<Resolution>> {
    if (this.running || preview.noteId !== this.local.noteId || preview.vaultId !== this.local.vaultId) return this.failure('INVALID_INPUT');
    this.running = true;
    try {
      const before = await this.io.prepareConflict(this.local.noteId);
      if (!before.ok) return before;
      if (!sameDiskVersion({ ok: true, value: preview }, before)) return { ok: true, value: { kind: 'changed', disk: before.value, copy: this.copied } };
      const checkpoint = await this.io.writeCheckpoint(this.checkpoint);
      if (!checkpoint.ok) return checkpoint;
      if (copyPath !== undefined) {
        if (this.copyRequest && this.copyRequest.relativePath !== copyPath) return this.failure('INVALID_INPUT');
        this.copyRequest ??= { sourceNoteId: this.local.noteId, source: this.local.source, relativePath: copyPath, operationId: this.uuid() };
        if (!this.copied) {
          const copied = await this.io.createCopy(this.copyRequest);
          if (!copied.ok) {
            if (copied.error.code === 'INVALID_INPUT' || copied.error.code === 'PATH_COLLISION') this.copyRequest = null;
            return copied;
          }
          this.copied = copied.value;
        }
      }
      const after = await this.io.prepareConflict(this.local.noteId);
      if (!after.ok) return after;
      return { ok: true, value: { kind: sameDiskVersion(before, after) ? 'resolved' : 'changed', disk: after.value, copy: this.copied } };
    } catch {
      return this.failure('IO_DENIED');
    } finally {
      this.running = false;
    }
  }

  private failure(code: 'IO_DENIED' | 'INVALID_INPUT'): ServiceOutcome<never> {
    return { ok: false, error: { code, messageKey: 'error.conflictResolution', params: {}, retryable: code === 'IO_DENIED', operationId: null, retryAfterSeconds: null } };
  }
}
