import type { NoteSummary } from '../domain';
import type { RenamePlan, RenameResult, ServiceOutcome } from '../editor';
import type { BatchDeletePlan, FolderMove, FolderMovePlan, Step } from './folder-ops';

/** What the folder steps need from the vault; the workspace supplies the real calls. */
export interface FolderActionsIo {
  notePath(noteId: string): Promise<string | null>;
  previewNoteRename(noteId: string, toPath: string): Promise<ServiceOutcome<RenamePlan>>;
  applyRename(plan: RenamePlan, expectedRevision: number, operationId: string): Promise<ServiceOutcome<RenameResult>>;
  createFolder(relativePath: string): Promise<ServiceOutcome<string>>;
  removeEmptyFolder(relativePath: string): Promise<ServiceOutcome<null>>;
  /** Sends a folder of non-note files to the system recycle bin. */
  recycleFolder(relativePath: string): Promise<ServiceOutcome<null>>;
  renameEmptyFolder(from: string, to: string): Promise<ServiceOutcome<string>>;
  /** Sends one note to the trash; the operation id makes a retried delete a replay. */
  trashNote(note: NoteSummary, operationId: string): Promise<ServiceOutcome<unknown>>;
  /** Brings open tabs and the list in line after a note left. */
  afterTrash(noteId: string): Promise<void>;
  newOperationId(): string;
}

const failure = (code: 'VERSION_CONFLICT' | 'IO_DENIED', messageKey: string, operationId: string | null) =>
  ({ ok: false as const, error: { code, messageKey, params: {}, retryable: true, operationId, retryAfterSeconds: null } });

/**
 * One note moved, planned again when it runs so earlier moves' link edits are
 * seen. A note already at its destination counts as done, so a retry after a
 * lost reply does not move it twice; the operation id is kept for the same reason.
 */
export function moveStep(move: FolderMove, io: FolderActionsIo): Step {
  const operationId = io.newOperationId();
  return {
    run: async () => {
      if ((await io.notePath(move.noteId)) === move.to) return { ok: true, value: null };
      const planned = await io.previewNoteRename(move.noteId, move.to);
      if (!planned.ok) return planned;
      const baseline = planned.value.expectedHashes.find((item) => item.noteId === move.noteId);
      if (!baseline) return failure('VERSION_CONFLICT', 'error.renameBaselineChanged', null);
      const outcome = await io.applyRename(planned.value, baseline.expectedRevision, operationId);
      if (outcome.ok && !outcome.value.completed) return failure('IO_DENIED', 'folderOp.incomplete', operationId);
      return outcome;
    },
  };
}

/**
 * The steps of a folder move: each note, then the (possibly empty) subfolders
 * at the destination, then the old folder, by then holding only folders. A
 * folder without notes is renamed in one step.
 */
export function moveSteps(plan: Extract<FolderMovePlan, { ok: true }>, io: FolderActionsIo): Step[] {
  if (plan.moves.length === 0) return [{ run: () => io.renameEmptyFolder(plan.from, plan.to) }];
  return [
    ...plan.moves.map((move) => moveStep(move, io)),
    ...plan.folders.map((path): Step => ({
      run: async () => {
        const outcome = await io.createFolder(path);
        // A note's move already made it, or an earlier attempt did.
        return !outcome.ok && outcome.error.code === 'PATH_COLLISION' ? { ok: true, value: null } : outcome;
      },
    })),
    { run: () => io.removeEmptyFolder(plan.from) },
  ];
}

/** Each note to the trash, then the empty folder tree. */
export function deleteSteps(path: string, notes: readonly NoteSummary[], io: FolderActionsIo): Step[] {
  return [...trashSteps(notes, io), { run: () => io.removeEmptyFolder(path) }];
}

/**
 * A selection deleted at once: its notes to the trash, then folders of other
 * files to the recycle bin, then each emptied folder. A folder already gone
 * from the recycle step counts as done, so a retry after a lost reply moves on.
 */
export function batchDeleteSteps(plan: BatchDeletePlan, io: FolderActionsIo): Step[] {
  return [
    ...trashSteps(plan.notes, io),
    ...plan.recycle.map((path): Step => ({
      run: async () => {
        const outcome = await io.recycleFolder(path);
        return !outcome.ok && outcome.error.code === 'OBJECT_NOT_FOUND' ? { ok: true, value: null } : outcome;
      },
    })),
    ...plan.folders.map((path): Step => ({ run: () => io.removeEmptyFolder(path) })),
  ];
}

/** One step per note; the operation id is fixed so a retry replays rather than repeats. */
function trashSteps(notes: readonly NoteSummary[], io: FolderActionsIo): Step[] {
  return notes.map((note): Step => {
    const operationId = io.newOperationId();
    return {
      run: async () => {
        const outcome = await io.trashNote(note, operationId);
        await io.afterTrash(note.noteId);
        return outcome;
      },
    };
  });
}

export interface LinkSummary {
  links: number;
  /** Other notes whose links change; moving notes are not counted. */
  notes: number;
  untouched: number;
}

/** Counts what the moves rewrite, from plans made now; each step plans again when it runs. */
export async function linkSummary(moves: readonly FolderMove[], io: Pick<FolderActionsIo, 'previewNoteRename'>): Promise<LinkSummary> {
  const moving = new Set(moves.map((move) => move.noteId));
  const rewritten = new Set<string>();
  let links = 0;
  let untouched = 0;
  for (const move of moves) {
    const planned = await io.previewNoteRename(move.noteId, move.to);
    if (!planned.ok) continue;
    links += planned.value.edits.length;
    untouched += planned.value.untouched.length;
    for (const edit of planned.value.edits) if (!moving.has(edit.noteId)) rewritten.add(edit.relativePath);
  }
  return { links, notes: rewritten.size, untouched };
}
