import type { CheckpointRequest, NoteService, CheckpointResult, ServiceOutcome } from './ports';

export class CheckpointController {
  private readonly busy = new Set<string>();
  private readonly failed = new Map<string, CheckpointRequest>();
  private readonly completed = new Map<string, string>();

  constructor(private readonly io: Pick<NoteService, 'writeCheckpoint'>, private readonly uuid: () => string) {}

  async write(input: Omit<CheckpointRequest, 'operationId'>): Promise<ServiceOutcome<CheckpointResult> | null> {
    if (this.busy.has(input.noteId)) return null;
    const fingerprint = JSON.stringify(input);
    if (!this.failed.has(input.noteId) && this.completed.get(input.noteId) === fingerprint) return null;
    const request = this.failed.get(input.noteId) ?? { ...input, operationId: this.uuid() };
    this.busy.add(input.noteId);
    try {
      const outcome = await this.io.writeCheckpoint(request);
      if (outcome.ok) {
        this.failed.delete(input.noteId);
        const { operationId: _, ...saved } = request;
        this.completed.set(input.noteId, JSON.stringify(saved));
      } else {
        this.failed.set(input.noteId, request);
      }
      return outcome;
    } catch {
      this.failed.set(input.noteId, request);
      return { ok: false, error: { code: 'IO_DENIED', messageKey: 'opennote.service.error', params: {}, retryable: true, operationId: request.operationId, retryAfterSeconds: null } };
    } finally {
      this.busy.delete(input.noteId);
    }
  }
}
