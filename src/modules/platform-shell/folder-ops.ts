import type { NoteSummary, ServiceError } from '../domain';
import type { ServiceOutcome } from '../editor';

/** One note moving with its folder. */
export interface FolderMove {
  noteId: string;
  from: string;
  to: string;
}

export type FolderMoveRefusal = 'same' | 'intoItself' | 'exists' | 'caseOnly';

export type FolderMovePlan =
  | { ok: true; from: string; to: string; moves: readonly FolderMove[]; folders: readonly string[] }
  | { ok: false; reason: FolderMoveRefusal };

const lower = (path: string) => path.toLowerCase();
const inside = (path: string, folder: string) => lower(path).startsWith(`${lower(folder)}/`);

/**
 * Where each note and each (possibly empty) subfolder of `from` goes when the
 * folder becomes `to`. Paths compare case-insensitively, as on Windows. A
 * case-only rename of a folder that holds notes is refused: Windows would
 * keep the old folder name while the notes move.
 */
export function planFolderMove(
  notes: readonly NoteSummary[],
  folders: readonly string[],
  from: string,
  to: string,
): FolderMovePlan {
  if (to === from) return { ok: false, reason: 'same' };
  if (lower(to) === lower(from)) {
    return notes.some((note) => inside(note.relativePath, from)) ? { ok: false, reason: 'caseOnly' } : { ok: true, from, to, moves: [], folders: [] };
  }
  if (inside(to, from)) return { ok: false, reason: 'intoItself' };
  const taken = [...notes.map((note) => note.relativePath), ...folders];
  if (taken.some((path) => lower(path) === lower(to) || inside(path, to))) return { ok: false, reason: 'exists' };
  const moves = notes
    .filter((note) => inside(note.relativePath, from))
    .map((note) => ({ noteId: note.noteId, from: note.relativePath, to: `${to}${note.relativePath.slice(from.length)}` }))
    .sort((a, b) => a.from.localeCompare(b.from));
  const moved = folders
    .filter((folder) => inside(folder, from))
    .map((folder) => `${to}${folder.slice(from.length)}`)
    .sort((a, b) => a.localeCompare(b));
  return { ok: true, from, to, moves, folders: [to, ...moved] };
}

/** The notes a folder delete sends to the trash, deepest path last. */
export function notesUnder(notes: readonly NoteSummary[], folder: string): NoteSummary[] {
  return notes.filter((note) => inside(note.relativePath, folder)).sort((a, b) => a.relativePath.localeCompare(b.relativePath));
}

export interface BatchDeletePlan {
  /** Notes to trash, each once, by path. */
  notes: NoteSummary[];
  /** Folders to remove once emptied; none sits inside another. */
  folders: string[];
  /** Folders holding files other than notes, sent whole to the system recycle bin. */
  recycle: string[];
  /** Selected folders holding a link, left untouched. */
  skipped: string[];
}

/**
 * What deleting a selection does. A note inside a selected folder is trashed
 * once, always to the vault's trash; a folder inside another selected folder
 * goes with it. A folder holding other files (a LaTeX project, say) goes to
 * the system recycle bin once its notes are out; one holding a link is
 * skipped, though notes picked one by one inside it are still trashed. The
 * vault root is never removed.
 */
export function planBatchDelete(
  targets: readonly ({ kind: 'folder'; path: string } | { kind: 'note'; noteId: string })[],
  notes: readonly NoteSummary[],
  folderKinds: ReadonlyMap<string, 'link' | 'files'>,
): BatchDeletePlan {
  const picked = [...new Set(targets.flatMap((target) => target.kind === 'folder' && target.path !== '' ? [target.path] : []))];
  const skipped = picked.filter((path) => folderKinds.get(path) === 'link').sort();
  const removable = picked.filter((path) => folderKinds.get(path) !== 'link');
  const deepestFirst = (a: string, b: string) => b.split('/').length - a.split('/').length || a.localeCompare(b);
  const outermost = removable.filter((path) => !removable.some((other) => other !== path && inside(path, other)));
  const recycle = outermost.filter((path) => folderKinds.get(path) === 'files').sort(deepestFirst);
  const folders = outermost.filter((path) => folderKinds.get(path) !== 'files').sort(deepestFirst);
  const chosen = new Set(targets.flatMap((target) => (target.kind === 'note' ? [target.noteId] : [])));
  const doomed = notes.filter((note) => chosen.has(note.noteId) || outermost.some((folder) => inside(note.relativePath, folder)));
  return { notes: [...doomed].sort((a, b) => a.relativePath.localeCompare(b.relativePath)), folders, recycle, skipped };
}

/** The path a dropped note or folder takes in `target` (`''` is the root). */
export function droppedPath(path: string, target: string): string {
  const name = path.slice(path.lastIndexOf('/') + 1);
  return target === '' ? name : `${target}/${name}`;
}

export interface Step {
  run(): Promise<ServiceOutcome<unknown>>;
}

export interface StepsResult {
  /** Steps finished, counting those done before `start`. */
  done: number;
  error: ServiceError | null;
}

/**
 * Runs steps in order from `start`, stopping at the first failure so a retry
 * resumes there. Each step must leave the vault consistent by itself.
 */
export async function runSteps(
  steps: readonly Step[],
  start: number,
  onProgress: (done: number, total: number) => void,
): Promise<StepsResult> {
  for (let index = start; index < steps.length; index += 1) {
    onProgress(index, steps.length);
    let outcome: ServiceOutcome<unknown>;
    try {
      outcome = await steps[index]!.run();
    } catch {
      outcome = { ok: false, error: { code: 'IO_DENIED', messageKey: 'error.unexpected', params: {}, retryable: true, operationId: null, retryAfterSeconds: null } };
    }
    if (!outcome.ok) return { done: index, error: outcome.error };
  }
  onProgress(steps.length, steps.length);
  return { done: steps.length, error: null };
}
