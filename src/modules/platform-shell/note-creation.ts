import type { NoteDocument, ServiceError } from '../domain';
import type { CreateNoteRequest, NoteService, ServiceOutcome } from '../editor';
import { serviceErrorFromThrown } from './port';

export interface NoteCreationSnapshot {
  request: Readonly<CreateNoteRequest> | null;
  working: boolean;
  error: ServiceError | null;
  uncertain: boolean;
}

/** The folder part of a vault-relative path, `''` at the root. */
export function folderOf(relativePath: string): string {
  const at = relativePath.lastIndexOf('/');
  return at < 0 ? '' : relativePath.slice(0, at);
}

/**
 * A free `<base>.md` in `folder`, numbered `<base> 1.md`, `<base> 2.md`… past
 * names already taken. Paths compare case-insensitively, as on Windows.
 */
export function untitledPath(taken: Iterable<string>, folder: string, base: string): string {
  const used = new Set([...taken].map((path) => path.toLowerCase()));
  const prefix = folder === '' ? '' : `${folder}/`;
  for (let index = 0; ; index += 1) {
    const path = `${prefix}${index === 0 ? base : `${base} ${index}`}.md`;
    if (!used.has(path.toLowerCase())) return path;
  }
}

/**
 * The path a typed title renames a note to: same folder, characters a file name
 * cannot hold become `-`, and `a/b` moves it into a subfolder. Null when
 * nothing usable is left.
 */
export function titleToPath(title: string, folder: string): string | null {
  const segments = title
    .split(/[/\\]/)
    .map((segment) => segment.replace(/[:*?"<>|\0]/g, '-').trim().replace(/[. ]+$/, ''))
    .filter((segment) => segment !== '' && segment !== '..');
  if (segments.length === 0) return null;
  const name = segments.join('/');
  return `${folder === '' ? '' : `${folder}/`}${name}.md`;
}

/** The title shown for a note: its file name without `.md`. */
export function titleOf(relativePath: string): string {
  return relativePath.slice(relativePath.lastIndexOf('/') + 1).replace(/\.md$/i, '');
}

/** Session request retention; durable commit/replay remains owned by M02. */
export class NoteCreationController {
  private request: Readonly<CreateNoteRequest> | null = null;
  private flight: Promise<ServiceOutcome<NoteDocument>> | null = null;
  private error: ServiceError | null = null;
  private uncertain = false;
  private listeners = new Set<(snapshot: NoteCreationSnapshot) => void>();

  constructor(private io: Pick<NoteService, 'createNote'> & { canCreate(): boolean; newOperationId(): string }) {}

  snapshot(): NoteCreationSnapshot {
    return { request: this.request, working: this.flight !== null, error: this.error, uncertain: this.uncertain };
  }

  subscribe(listener: (snapshot: NoteCreationSnapshot) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  private publish(): void {
    for (const listener of this.listeners) listener(this.snapshot());
  }

  private blocked(): ServiceOutcome<never> {
    return { ok: false, error: { code: 'VERSION_CONFLICT', messageKey: 'workspace.create.pending', params: {},
      retryable: false, operationId: this.request?.operationId ?? null, retryAfterSeconds: null } };
  }

  create(relativePath: string, source: string): Promise<ServiceOutcome<NoteDocument>> {
    const same = this.request?.relativePath === relativePath && this.request.source === source;
    if (this.flight !== null) return same ? this.flight : Promise.resolve(this.blocked());
    if (!this.io.canCreate() || (this.uncertain && !same)) return Promise.resolve(this.blocked());
    if (!same) this.request = Object.freeze({ relativePath, source, operationId: this.io.newOperationId() });
    const request = this.request!;
    this.error = null;
    this.flight = Promise.resolve().then(async () => {
      let outcome: ServiceOutcome<NoteDocument>;
      try { outcome = await this.io.createNote(request); }
      catch (error) { outcome = { ok: false, error: serviceErrorFromThrown(error) }; }
      if (outcome.ok) {
        this.request = null;
        this.uncertain = false;
      } else {
        this.error = outcome.error;
        // These typed rejections precede a fresh create commit in M02.
        this.uncertain ||= !['INVALID_INPUT', 'PATH_COLLISION', 'UNSUPPORTED_SCHEMA'].includes(outcome.error.code);
      }
      return outcome;
    }).finally(() => { this.flight = null; this.publish(); });
    this.publish();
    return this.flight;
  }

  retry(): Promise<ServiceOutcome<NoteDocument>> {
    return this.request === null ? Promise.resolve(this.blocked()) : this.create(this.request.relativePath, this.request.source);
  }

  dismissRejected(): boolean {
    if (this.flight !== null || this.uncertain) return false;
    this.request = null;
    this.error = null;
    this.publish();
    return true;
  }
}
