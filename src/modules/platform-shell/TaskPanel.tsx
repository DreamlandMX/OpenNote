import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { CalendarDays, FileText, Trash2, X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import type { NoteService, ServiceOutcome, TaskRow } from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import { formatCalendarDate, localCalendarDate, type RegionalPreferences } from './regional';
import {
  filterCounts,
  groupTasks,
  inFolder,
  sourceFolders,
  type TaskRewrite,
  type TaskFilter,
  type TaskGroupKey,
} from './task-view';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  regional: RegionalPreferences;
  writable: boolean;
  onWrite(task: TaskRow, rewrite: TaskRewrite): Promise<ServiceOutcome<{ changed: boolean }>>;
  /** Opens the source note; `offset` is a byte offset from the task parse. */
  onOpenNote(noteId: string, offset: number): void;
  onClose(): void;
}

const GROUP_LABEL_KEYS: Record<TaskGroupKey, 'tasks.today' | 'tasks.upcoming' | 'tasks.overdue' | 'tasks.undated' | 'tasks.done'> = {
  today: 'tasks.today',
  upcoming: 'tasks.upcoming',
  overdue: 'tasks.overdue',
  undated: 'tasks.undated',
  done: 'tasks.done',
};

/**
 * S24 task list.
 *
 * A toggle is a targeted rewrite of one Markdown line, applied to the editor
 * buffer and then saved through the normal path; there is no second task table
 * (F16-R01). A task whose text changed at its recorded offset is refused with
 * "the task moved", so a stale list can never tick the wrong line (F16-R03).
 */
export function TaskPanel({
  service,
  translator,
  locale,
  regional,
  writable,
  onWrite,
  onOpenNote,
  onClose,
}: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [tasks, setTasks] = useState<readonly TaskRow[]>([]);
  const [error, setError] = useState<ServiceError | null>(null);
  const [loading, setLoading] = useState(false);
  const [filter, setFilter] = useState<TaskFilter>('all');
  const [folder, setFolder] = useState<string | null>(null);
  const [toggling, setToggling] = useState<string | null>(null);
  const [notice, setNotice] = useState<'tasks.deleted' | 'tasks.dueSet' | 'tasks.dueCleared' | null>(null);
  const busy = useRef(false);
  const generation = useRef(0);
  const mounted = useRef(true);

  const [today, setToday] = useState(() => localCalendarDate(new Date()));

  useEffect(() => {
    const refresh = () => setToday(localCalendarDate(new Date()));
    // Recheck wall-clock/time-zone changes within a minute, and on resume.
    const timer = window.setInterval(refresh, 60_000);
    window.addEventListener('focus', refresh);
    document.addEventListener('visibilitychange', refresh);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener('focus', refresh);
      document.removeEventListener('visibilitychange', refresh);
    };
  }, []);

  const load = useCallback(async (): Promise<void> => {
    const request = ++generation.current;
    setLoading(true);
    const outcome = await service.listTasks();
    if (!mounted.current || request !== generation.current) return;
    setLoading(false);
    if (outcome.ok) {
      setTasks(outcome.value);
      setError(null);
    } else {
      setError(outcome.error);
    }
  }, [service]);

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => { mounted.current = false; generation.current += 1; };
  }, [load]);

  const folders = useMemo(() => sourceFolders(tasks), [tasks]);
  const scoped = useMemo(
    () => tasks.filter((task) => inFolder(task, folder)),
    [tasks, folder],
  );
  const counts = useMemo(() => filterCounts(scoped), [scoped]);
  const groups = useMemo(
    () => groupTasks(scoped, today, filter),
    [scoped, today, filter],
  );

  async function toggle(task: TaskRow): Promise<void> {
    await applyRewrite(task, (source, offset, text) =>
      service.toggleTask(source, offset, text, task.status === 'done' ? 'open' : 'done'));
  }

  async function applyRewrite(
    task: TaskRow,
    rewrite: TaskRewrite,
    noticeKey?: 'tasks.deleted' | 'tasks.dueSet' | 'tasks.dueCleared',
  ): Promise<void> {
    if (!writable || task.readOnly || busy.current) return;
    busy.current = true;
    generation.current += 1;
    setLoading(false);
    setToggling(`${task.noteId}:${task.offset}`);
    setNotice(null);
    try {
      const outcome = await onWrite(task, rewrite);
      if (!mounted.current) return;
      if (!outcome.ok) { setError(outcome.error); return; }
      setNotice(noticeKey ?? null);
      await load();
    } finally {
      busy.current = false;
      if (mounted.current) setToggling(null);
    }
  }

  return (
    <aside
      className="task-panel"
      aria-labelledby="task-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="task-head">
        <div>
          <h2 id="task-heading">{translator('tasks.title')}</h2>
          <p className="task-date">
            <time dateTime={today}>{formatCalendarDate(today, regional.region, 'medium')}</time>
          </p>
        </div>
        <button
          type="button"
          className="task-close"
          aria-label={translator('tasks.close')}
          title={translator('tasks.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <div className="task-filters" role="group" aria-label={translator('tasks.filterLabel')}>
        {(['all', 'open', 'done'] as const).map((option) => (
          <button
            key={option}
            type="button"
            className="task-filter"
            aria-pressed={filter === option ? 'true' : 'false'}
            onClick={() => setFilter(option)}
          >
            {translator(`tasks.filter.${option}`)}
            <span className="task-count">{counts[option]}</span>
          </button>
        ))}
        {folders.length > 0 ? (
          <label className="task-folder">
            <span className="visually-hidden">{translator('tasks.sourceFolder')}</span>
            <select
              value={folder ?? ''}
              onChange={(event) => setFolder(event.target.value === '' ? null : event.target.value)}
            >
              <option value="">{translator('tasks.allFolders')}</option>
              {folders.map((option) => (
                <option key={option} value={option}>
                  {option}
                </option>
              ))}
            </select>
          </label>
        ) : null}
      </div>

      {loading ? (
        <p role="status">{translator('tasks.loading')}</p>
      ) : null}
      {error !== null ? (
        <div className="task-error" role="alert">
          <p>{error.messageKey === 'tasks.sourceChanged' ? translator('tasks.sourceChanged') : serviceErrorMessage(locale, error.code)}</p>
          <button type="button" className="secondary" disabled={toggling !== null} onClick={() => void load()}>
            {translator('tasks.retry')}
          </button>
        </div>
      ) : null}
      {notice !== null ? (
        <p className="task-notice" role="status">
          {translator(notice)}
        </p>
      ) : null}

      {!loading && error === null && groups.length === 0 ? (
        <p className="task-empty" role="status">
          {translator('tasks.empty')}
        </p>
      ) : null}

      {groups.map((group) => (
        <section key={group.key} className="task-group" aria-labelledby={`task-group-${group.key}`}>
          <h3 id={`task-group-${group.key}`}>
            {translator(GROUP_LABEL_KEYS[group.key])}
            <span className="task-count">{group.tasks.length}</span>
          </h3>
          <ul>
            {group.tasks.map(({ task }) => {
              const key = `${task.noteId}:${task.offset}`;
              const checked = task.status === 'done';
              return (
                <li key={key} className="task-row">
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={!writable || task.readOnly || toggling !== null}
                    aria-label={translator(checked ? 'tasks.markOpen' : 'tasks.markDone', {
                      name: task.text,
                    })}
                    onChange={() => void toggle(task)}
                  />
                  <span className="task-text" data-done={checked ? 'true' : undefined}>
                    {task.text}
                  </span>
                  {task.due !== null ? (
                    <span className="task-due">
                      <CalendarDays size={14} aria-hidden="true" />
                      <time dateTime={task.due}>
                        {formatCalendarDate(task.due, regional.region, 'short')}
                      </time>
                    </span>
                  ) : (
                    <span className="task-due task-undated">{translator('tasks.noDate')}</span>
                  )}
                  <button
                    type="button"
                    className="task-source"
                    title={task.relativePath}
                    onClick={() => onOpenNote(task.noteId, task.offset)}
                  >
                    <FileText size={14} aria-hidden="true" />
                    <span>{task.relativePath.split('/').pop()?.replace(/\.md$/i, '')}</span>
                  </button>
                  <label className="task-due-edit">
                    <span className="visually-hidden">
                      {translator('tasks.setDue', { name: task.text })}
                    </span>
                    <input
                      type="date"
                      value={task.due ?? ''}
                      disabled={!writable || task.readOnly || toggling !== null}
                      aria-label={translator('tasks.setDue', { name: task.text })}
                      onChange={(event) => {
                        const value = event.target.value;
                        void applyRewrite(
                          task,
                          (source, offset, text) =>
                            service.setTaskDue(source, offset, text, value === '' ? null : value),
                          value === '' ? 'tasks.dueCleared' : 'tasks.dueSet',
                        );
                      }}
                    />
                  </label>
                  <button
                    type="button"
                    className="task-delete"
                    disabled={!writable || task.readOnly || toggling !== null}
                    aria-label={translator('tasks.delete', { name: task.text })}
                    title={translator('tasks.delete', { name: task.text })}
                    onClick={() => {
                      // Deleting removes the line; completing keeps it. The
                      // confirmation makes that difference explicit (F16-R04).
                      if (!window.confirm(translator('tasks.deleteConfirm', { name: task.text }))) {
                        return;
                      }
                      void applyRewrite(
                        task,
                        (source, offset, text) => service.deleteTask(source, offset, text),
                        'tasks.deleted',
                      );
                    }}
                  >
                    <Trash2 size={14} aria-hidden="true" />
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
      <p className="task-hint">{translator('tasks.hint')}</p>
    </aside>
  );
}
