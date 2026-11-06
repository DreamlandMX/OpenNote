import type { NoteService, SaveController, ServiceOutcome, TaskRow } from '../editor';

export type TaskRewrite = (source: string, offset: number, text: string) => Promise<ServiceOutcome<{ source: string }>>;

/** Task ranges belong to a parsed revision, never to an arbitrary matching line. */
export async function writeTaskSource(
  io: {
    service: Pick<NoteService, 'readNote'>;
    controller: SaveController;
    canWrite(): boolean;
    onSourceApplied(noteId: string, source: string): void;
  },
  task: TaskRow,
  rewrite: TaskRewrite,
): Promise<ServiceOutcome<{ changed: boolean }>> {
  const conflict = (): ServiceOutcome<never> => ({ ok: false, error: {
    code: 'VERSION_CONFLICT', messageKey: 'tasks.sourceChanged', params: {},
    retryable: false, operationId: null, retryAfterSeconds: null,
  } });
  const ready = (): boolean => {
    const snapshot = io.controller.snapshot(task.noteId);
    return io.canWrite() && !task.readOnly &&
      !io.controller.notesNeedingAttentionOnExit().some(note => note.noteId === task.noteId) &&
      (snapshot === null || (snapshot.revision === task.sourceRevision && snapshot.contentHash === task.sourceHash));
  };
  if (!ready()) return conflict();
  const disk = await io.service.readNote(task.noteId);
  if (!disk.ok) return disk;
  if (disk.value.noteId !== task.noteId || disk.value.readOnly ||
      disk.value.localRevision !== task.sourceRevision || disk.value.contentHash !== task.sourceHash ||
      !ready()) return conflict();
  const source = disk.value.source;
  if (io.controller.snapshot(task.noteId) !== null && io.controller.getBufferText(task.noteId) !== source) return conflict();
  const edited = await rewrite(source, task.offset, task.text);
  if (!edited.ok) return edited;
  // A late pure-command response must not replace input made while it ran.
  if (!ready() || (io.controller.snapshot(task.noteId) !== null && io.controller.getBufferText(task.noteId) !== source)) return conflict();
  if (edited.value.source === source) return { ok: true, value: { changed: false } };
  io.controller.registerDocument(disk.value);
  io.controller.setBufferText(task.noteId, edited.value.source);
  io.onSourceApplied(task.noteId, edited.value.source);
  await io.controller.requestSave(task.noteId);
  const after = io.controller.snapshot(task.noteId);
  if (after?.status !== 'clean') return after?.error ? { ok: false, error: after.error } : conflict();
  return { ok: true, value: { changed: true } };
}

/**
 * S24 task list view logic.
 *
 * Grouping is by a **local** calendar date supplied by the caller: the host
 * never reports a due date as an instant, so a device in another zone cannot
 * move a task to yesterday (F16-R02). Everything here is pure so the grouping
 * and filtering rules are testable without a vault.
 */

export type TaskFilter = 'all' | 'open' | 'done';

export type TaskGroupKey = 'overdue' | 'today' | 'upcoming' | 'undated' | 'done';

export interface VisibleTask {
  task: TaskRow;
  /** Index in the full list, so a caller can keep a stable key order. */
  index: number;
}

export interface TaskGroup {
  key: TaskGroupKey;
  tasks: VisibleTask[];
}

/** Order the sections the reference image shows, with overdue first. */
export const GROUP_ORDER: readonly TaskGroupKey[] = [
  'overdue',
  'today',
  'upcoming',
  'undated',
  'done',
];

export function groupOf(task: TaskRow, today: string): TaskGroupKey {
  if (task.status === 'done') {
    return 'done';
  }
  if (task.due === null) {
    return 'undated';
  }
  if (task.due === today) {
    return 'today';
  }
  return task.due > today ? 'upcoming' : 'overdue';
}

export function filterTasks(tasks: readonly TaskRow[], filter: TaskFilter): VisibleTask[] {
  const visible: VisibleTask[] = [];
  for (let index = 0; index < tasks.length; index += 1) {
    const task = tasks[index];
    if (task === undefined) {
      continue;
    }
    if (filter === 'open' && task.status !== 'open') {
      continue;
    }
    if (filter === 'done' && task.status !== 'done') {
      continue;
    }
    visible.push({ task, index });
  }
  return visible;
}

/** Groups visible tasks into the S24 sections, omitting empty sections. */
export function groupTasks(
  tasks: readonly TaskRow[],
  today: string,
  filter: TaskFilter,
): TaskGroup[] {
  const buckets = new Map<TaskGroupKey, VisibleTask[]>();
  for (const entry of filterTasks(tasks, filter)) {
    const key = groupOf(entry.task, today);
    const bucket = buckets.get(key);
    if (bucket === undefined) {
      buckets.set(key, [entry]);
    } else {
      bucket.push(entry);
    }
  }
  return GROUP_ORDER.filter((key) => (buckets.get(key)?.length ?? 0) > 0).map((key) => ({
    key,
    tasks: buckets.get(key) ?? [],
  }));
}

/**
 * The new source after toggling, or the reason nothing changed.
 *
 * `refresh` is returned so the caller knows to re-read the note: the offset of
 * every later task can move when a line is edited.
 */
export interface TogglePlan {
  ok: boolean;
  /** Why the toggle could not be applied, as a message key. */
  reason: 'taskMoved' | 'taskMissing' | 'taskAmbiguous' | null;
}

export function toggleRequest(
  task: TaskRow,
  target: 'open' | 'done',
): { offset: number; expectedText: string; status: 'open' | 'done' } {
  return { offset: task.offset, expectedText: task.text, status: target };
}

/**
 * Counts the tasks a filter would show, for the tab labels in S24.
 * `all` counts every task including done ones.
 */
export function filterCounts(tasks: readonly TaskRow[]): Record<TaskFilter, number> {
  let open = 0;
  let done = 0;
  for (const task of tasks) {
    if (task.status === 'done') {
      done += 1;
    } else {
      open += 1;
    }
  }
  return { all: open + done, open, done };
}

/** Distinct source folders, for the "source folder" filter. */
export function sourceFolders(tasks: readonly TaskRow[]): string[] {
  const folders = new Set<string>();
  for (const task of tasks) {
    const segments = task.relativePath.replace(/\\/g, '/').split('/');
    segments.pop();
    // Every level is offered, so a picker value always matches `inFolder`'s
    // prefix rule; listing only the deepest path would make "work" selectable
    // in one note and invisible in another.
    let prefix = '';
    for (const segment of segments) {
      if (segment === '') {
        continue;
      }
      prefix = prefix === '' ? segment : `${prefix}/${segment}`;
      folders.add(prefix);
    }
  }
  return [...folders].sort((left, right) => left.localeCompare(right));
}

/** Applies the source-folder filter; a null folder means "all folders". */
export function inFolder(task: TaskRow, folder: string | null): boolean {
  if (folder === null) {
    return true;
  }
  const normalized = task.relativePath.replace(/\\/g, '/');
  return normalized === folder || normalized.startsWith(`${folder}/`);
}

/** A `<time datetime>`-safe value: the stored date is already a day. */
export function dueAttribute(task: TaskRow): string | null {
  return task.due;
}
