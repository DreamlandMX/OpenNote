import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type DragEvent,
  type FormEvent,
} from 'react';
import {
  CalendarDays,
  ChevronLeft,
  ChevronRight,
  Columns3,
  Filter,
  Images,
  Plus,
  Table2,
  X,
} from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import { usePanelExitGuard, type RegisterPanelGuard } from './panel-guards';
import type {
  DatabaseColumn,
  DatabaseFilter,
  DatabaseOutcome,
  DatabaseRow,
  DatabaseSort,
  NoteService,
  PropertyValue,
  ServiceOutcome,
} from '../editor';
import type { NoteDocument, ServiceError } from '../domain';
import type { MessageKey, MessageParams, Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  ariaSort,
  cellText,
  describeFilters,
  isEmptyDueToFilters,
  isUnset,
  nextSort,
  operatorsFor,
  rowKey,
  tableSummary,
  toFilter,
  usableFilters,
} from './table-view';
import {
  calendarRows,
  currentMonth,
  DATABASE_VIEWS,
  dateCellValue,
  formatMonthLabel,
  groupRowsBySelect,
  monthGrid,
  selectChoices,
  shiftMonth,
  type DatabaseViewType,
} from './database-views';
import { WEEKDAY_KEYS } from './daily-view';
import { firstDayOfWeek, formatCalendarDate } from './regional';
import { newRowPath, type NewRowPathFailure } from './database-row';
import { newOperationId } from './tauri-note-service';
import {
  VIEW_FORMAT_VERSION,
  parseViewDefinition,
  serializeViewDefinition,
  viewNameFromPath,
  viewPathFromName,
  type ViewDefinition,
} from './view-definition';

const VIEW_ICONS = {
  table: Table2,
  board: Columns3,
  gallery: Images,
  calendar: CalendarDays,
} as const;

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** Region used for numbers and dates, from the device preferences. */
  region: string;
  timeZone: string;
  onOpenNote(noteId: string): void;
  /** F23-R02: writes one select/date cell through the note's normal save path. */
  onSetCell(
    row: DatabaseRow,
    name: string,
    value: PropertyValue,
    retryFailed?: boolean,
  ): Promise<ServiceOutcome<{ changed: boolean }>>;
  /** F22-R03: creates a real Markdown note for the new row and returns it. */
  onCreateRow(relativePath: string): Promise<ServiceOutcome<NoteDocument>>;
  canWrite: boolean;
  createdNote?: Pick<NoteDocument, 'noteId' | 'relativePath'> | null;
  /** Lists an unsaved view file in the window's exit and vault-switch warning. */
  registerExitGuard?: RegisterPanelGuard;
  onClose(): void;
}

/**
 * S21 database table.
 *
 * The projection runs in M06; this panel only edits the request and renders the
 * answer. Unset cells say "not set" rather than showing a zero, and hidden rows
 * are counted so a filter can never look like a deletion (F22-R01, F23-R04).
 */
export function DatabasePanel({
  service,
  translator,
  locale,
  region,
  timeZone,
  onOpenNote,
  onSetCell,
  onCreateRow,
  canWrite,
  createdNote = null,
  registerExitGuard,
  onClose,
}: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [outcome, setOutcome] = useState<DatabaseOutcome | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [writeError, setWriteError] = useState<ServiceError | null>(null);
  const [failedWrite, setFailedWrite] = useState<{
    row: DatabaseRow;
    name: string;
    value: PropertyValue;
  } | null>(null);
  const [writeNotice, setWriteNotice] = useState(false);
  const writeBusy = useRef(false);
  const draggedRowRef = useRef<DatabaseRow | null>(null);
  const loadGeneration = useRef(0);
  const mounted = useRef(true);
  const [writing, setWriting] = useState<string | null>(null);
  const [rowTitle, setRowTitle] = useState('');
  const [rowError, setRowError] = useState<NewRowPathFailure | ServiceError | null>(null);
  const [createdRowTitle, setCreatedRowTitle] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const rowDraftVersion = useRef(0);
  const rowCreationRef = useRef<{ path: string; after: string | null; draftVersion: number } | null>(null);
  const [loading, setLoading] = useState(false);
  const [sort, setSort] = useState<DatabaseSort | null>(null);
  const [filters, setFilters] = useState<DatabaseFilter[]>([]);
  const [draftKey, setDraftKey] = useState('');
  const [draftOp, setDraftOp] = useState<DatabaseFilter['op']>('contains');
  const [draftValue, setDraftValue] = useState('');
  const [folder, setFolder] = useState('');
  const [recursive, setRecursive] = useState(true);
  const [needsFilterValue, setNeedsFilterValue] = useState(false);
  const [view, setView] = useState<DatabaseViewType>('table');
  const [month, setMonth] = useState(() => currentMonth());
  const [groupKey, setGroupKey] = useState('');
  const [dateKey, setDateKey] = useState('');

  // F22-A02: the saved view file this panel is showing, if any. `baseDefinition`
  // carries unknown fields from the file so a save writes them back unchanged.
  const [viewFiles, setViewFiles] = useState<readonly string[]>([]);
  const [viewFile, setViewFile] = useState<{ path: string; sha256: string; readOnly: boolean } | null>(null);
  const [baseDefinition, setBaseDefinition] = useState<ViewDefinition | null>(null);
  const [savedViewText, setSavedViewText] = useState<string | null>(null);
  const [viewName, setViewName] = useState('');
  const [viewChoice, setViewChoice] = useState('');
  const [viewSaving, setViewSaving] = useState(false);
  const [viewConflict, setViewConflict] = useState(false);
  const [viewStatus, setViewStatus] = useState<{ key: MessageKey; params?: MessageParams } | null>(null);
  const [viewError, setViewError] = useState<ServiceError | null>(null);
  const [closeArmed, setCloseArmed] = useState(false);

  const currentDefinition: ViewDefinition = {
    formatVersion: baseDefinition?.formatVersion ?? VIEW_FORMAT_VERSION,
    name: baseDefinition?.name ?? '',
    source: { folder: folder.trim(), recursive },
    viewType: view,
    filters,
    sort,
    groupKey,
    dateKey,
    extra: baseDefinition?.extra ?? {},
  };
  const currentViewText = serializeViewDefinition(currentDefinition);
  const viewDirty = viewFile !== null && savedViewText !== currentViewText;
  useEffect(() => { setCloseArmed(false); }, [currentViewText]);

  const refreshViewFiles = useCallback(async (): Promise<void> => {
    const listed = await service.listViews();
    if (listed.ok && mounted.current) setViewFiles(listed.value);
  }, [service]);
  useEffect(() => { void refreshViewFiles(); }, [refreshViewFiles]);

  const applyDefinition = (definition: ViewDefinition): void => {
    setFolder(definition.source.folder);
    setRecursive(definition.source.recursive);
    setView(definition.viewType);
    setFilters(definition.filters);
    setSort(definition.sort);
    setGroupKey(definition.groupKey);
    setDateKey(definition.dateKey);
  };

  const openView = async (path: string, force = false): Promise<void> => {
    if (viewDirty && !closeArmed && !force) {
      setCloseArmed(true);
      setViewStatus({ key: 'database.viewFile.discardFirst' });
      return;
    }
    const loaded = await service.readView(path);
    if (!mounted.current) return;
    if (!loaded.ok) {
      setViewError(loaded.error);
      return;
    }
    const parsed = parseViewDefinition(loaded.value.source);
    if (!parsed.ok) {
      setViewStatus({ key: 'database.viewFile.invalid', params: { name: loaded.value.relativePath } });
      return;
    }
    applyDefinition(parsed.value);
    setBaseDefinition(parsed.value);
    setSavedViewText(serializeViewDefinition(parsed.value));
    setViewFile({ path: loaded.value.relativePath, sha256: loaded.value.sha256, readOnly: parsed.readOnly });
    setViewConflict(false);
    setViewError(null);
    setCloseArmed(false);
    setViewStatus({
      key: parsed.readOnly ? 'database.viewFile.newerFormat' : 'database.viewFile.opened',
      params: { name: viewNameFromPath(loaded.value.relativePath) },
    });
  };

  const saveView = async (path: string, expected: string | null): Promise<boolean> => {
    if (viewSaving) return false;
    const definition = expected === null
      ? { ...currentDefinition, formatVersion: VIEW_FORMAT_VERSION, name: viewNameFromPath(path) }
      : currentDefinition;
    const text = serializeViewDefinition(definition);
    setViewSaving(true);
    const result = await service.saveView(path, text, expected, newOperationId());
    if (!mounted.current) return result.ok;
    setViewSaving(false);
    if (!result.ok) {
      if (result.error.code === 'VERSION_CONFLICT') {
        setViewConflict(true);
        setViewStatus({ key: 'database.viewFile.conflict' });
      } else {
        setViewError(result.error);
      }
      return false;
    }
    setBaseDefinition(definition);
    setSavedViewText(text);
    setViewFile({ path: result.value.relativePath, sha256: result.value.sha256, readOnly: false });
    setViewConflict(false);
    setViewError(null);
    setCloseArmed(false);
    setViewStatus({ key: 'database.viewFile.saved', params: { name: viewNameFromPath(result.value.relativePath) } });
    void refreshViewFiles();
    return true;
  };

  // An exported rescue copy stops the exit warning until the next edit.
  const [rescuedViewText, setRescuedViewText] = useState<string | null>(null);
  usePanelExitGuard(registerExitGuard, 'database-view', {
    active: viewDirty && !(viewFile?.readOnly ?? false) && currentViewText !== rescuedViewText,
    label: viewFile?.path ?? translator('database.viewFile.untitled'),
    status: viewSaving ? 'saving' : viewConflict ? 'conflict' : 'dirty',
    save: () => (viewFile === null || viewConflict ? Promise.resolve(false) : saveView(viewFile.path, viewFile.sha256)),
    draft: () => ({ relativePath: viewFile?.path ?? 'view.onview.json', source: currentViewText }),
    rescued: () => setRescuedViewText(currentViewText),
  });

  const requestClose = (): void => {
    if (viewDirty && !closeArmed) {
      setCloseArmed(true);
      setViewStatus({ key: 'database.viewFile.unsavedClose' });
      return;
    }
    onClose();
  };

  // The last successful projection is the only trusted source of column keys
  // and types; it decides which conditions can run (F23 §7 FILTER_INVALID).
  const columnsRef = useRef<readonly DatabaseColumn[]>([]);

  const load = useCallback(async (): Promise<void> => {
    const generation = ++loadGeneration.current;
    setLoading(true);
    const request = (filtersToSend: readonly DatabaseFilter[]) =>
      service.databaseTable({
        filters: filtersToSend,
        sort,
        folder: folder.trim() === '' ? null : folder.trim(),
        recursive,
      });
    let usable = usableFilters(filters, columnsRef.current);
    let result = await request(usable);
    if (result.ok) {
      // A type change is only visible in the answer, so drop the conditions it
      // invalidated and ask once more instead of rendering a filtered-away table.
      const stillUsable = usableFilters(filters, result.value.table.columns);
      if (stillUsable.length !== usable.length) {
        usable = stillUsable;
        result = await request(usable);
      }
    }
    if (!mounted.current || generation !== loadGeneration.current) return;
    setLoading(false);
    if (result.ok) {
      columnsRef.current = result.value.table.columns;
      setOutcome(result.value);
      setError(null);
    } else {
      setError(result.error);
    }
  }, [service, filters, sort, folder, recursive]);

  const loadRef = useRef(load);
  loadRef.current = load;

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => { mounted.current = false; loadGeneration.current += 1; };
  }, [load, createdNote?.noteId]);

  useEffect(() => {
    const pending = rowCreationRef.current;
    if (!createdNote || !pending || pending.after === createdNote.noteId || pending.path !== createdNote.relativePath) return;
    rowCreationRef.current = null;
    if (rowDraftVersion.current === pending.draftVersion) {
      setRowTitle('');
      setRowError(null);
    }
    setCreatedRowTitle(createdNote.relativePath.split('/').pop()?.replace(/\.md$/i, '') ?? createdNote.relativePath);
  }, [createdNote]);

  const columns = outcome?.table.columns ?? [];
  const summary = outcome === null ? null : tableSummary(outcome.table);
  const activeFilters = describeFilters(filters, columns);
  const inactiveFilters = activeFilters.filter((filter) => filter.problem !== null);

  const columnOptions = useMemo(() => columns.map((column) => column.key), [columns]);
  const effectiveKey = draftKey !== '' && columnOptions.includes(draftKey)
    ? draftKey
    : columnOptions[0] ?? '';

  const selectColumns = useMemo(
    () => columns.filter((column) => column.kind === 'select' || column.kind === 'multiselect'),
    [columns],
  );
  const dateColumns = useMemo(
    () => columns.filter((column) => column.kind === 'date'),
    [columns],
  );
  const effectiveGroupKey = selectColumns.some((column) => column.key === groupKey)
    ? groupKey
    : selectColumns[0]?.key ?? '';
  const effectiveDateKey = dateColumns.some((column) => column.key === dateKey)
    ? dateKey
    : dateColumns[0]?.key ?? '';
  const boardGroups = useMemo(
    () =>
      outcome === null || effectiveGroupKey === ''
        ? []
        : groupRowsBySelect(outcome.table, effectiveGroupKey),
    [outcome, effectiveGroupKey],
  );
  const calendar = useMemo(
    () =>
      outcome === null || effectiveDateKey === ''
        ? { dated: [], unscheduled: [] }
        : calendarRows(outcome.table, effectiveDateKey),
    [outcome, effectiveDateKey],
  );
  const grid = useMemo(
    () => monthGrid(month, firstDayOfWeek(region)),
    [month, region],
  );
  const weeks = useMemo(() => {
    const result: (string | null)[][] = [];
    for (let index = 0; index < grid.length; index += 7) {
      result.push(grid.slice(index, index + 7));
    }
    return result;
  }, [grid]);
  const weekdays = useMemo(() => {
    const offset = firstDayOfWeek(region) - 1;
    return Array.from({ length: 7 }, (_, index) => WEEKDAY_KEYS[(offset + index) % 7]!);
  }, [region]);
  const datedByDay = useMemo(() => {
    const byDay = new Map<string, DatabaseRow[]>();
    for (const entry of calendar.dated) {
      const existing = byDay.get(entry.date);
      if (existing === undefined) {
        byDay.set(entry.date, [entry.row]);
      } else {
        existing.push(entry.row);
      }
    }
    return byDay;
  }, [calendar]);
  const monthLabel = formatMonthLabel(month, region);
  const groupColumn = columns.find((column) => column.key === effectiveGroupKey);
  const canGroupWrite = canWrite && groupColumn?.kind === 'select';

  function groupValueOf(row: DatabaseRow): string | null {
    const index = columns.findIndex((column) => column.key === effectiveGroupKey);
    const cell = index < 0 ? undefined : row.cells[index];
    return cell?.kind === 'select' && cell.value !== '' ? cell.value : null;
  }

  async function writeCell(
    row: DatabaseRow,
    name: string,
    value: PropertyValue,
    retryFailed = false,
  ): Promise<void> {
    if (!canWrite || row.readOnly || writeBusy.current || creating) return;
    writeBusy.current = true;
    loadGeneration.current += 1;
    setLoading(false);
    setWriting(`${row.noteId}:${name}`);
    setWriteError(null);
    setFailedWrite(null);
    setWriteNotice(false);
    try {
      const result = await onSetCell(row, name, value, retryFailed);
      if (!mounted.current) return;
      if (!result.ok) {
        // Save failures may have committed; the controller retains the original request.
        setFailedWrite({ row, name, value });
        setWriteError(result.error);
      } else {
        setWriteNotice(result.value.changed);
      }
      await loadRef.current();
    } finally {
      writeBusy.current = false;
      if (mounted.current) setWriting(null);
    }
  }

  function removeInactiveFilters(): void {
    setFilters((current) => usableFilters(current, columns));
  }

  async function submitRow(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (!canWrite || creating || writing !== null) {
      return;
    }
    const built = newRowPath(folder, rowTitle);
    if (!built.ok) {
      setCreatedRowTitle(null);
      setRowError(built.reason);
      return;
    }
    setCreating(true);
    const draftVersion = rowDraftVersion.current;
    rowCreationRef.current = { path: built.path, after: createdNote?.noteId ?? null, draftVersion };
    setRowError(null);
    setCreatedRowTitle(null);
    const outcome = await onCreateRow(built.path);
    setCreating(false);
    if (!outcome.ok) {
      setRowError(outcome.error);
      return;
    }
    if (rowDraftVersion.current === draftVersion) setRowTitle('');
    rowCreationRef.current = null;
    setCreatedRowTitle(built.name);
    await load();
  }

  function startDrag(event: DragEvent, row: DatabaseRow): void {
    if (!canWrite || row.readOnly) {
      return;
    }
    draggedRowRef.current = row;
    event.dataTransfer.setData('text/plain', row.noteId);
    event.dataTransfer.effectAllowed = 'move';
  }

  function droppedRow(event: DragEvent): DatabaseRow | null {
    event.preventDefault();
    const noteId = event.dataTransfer.getData('text/plain');
    const row = draggedRowRef.current;
    draggedRowRef.current = null;
    return row && !row.readOnly && row.noteId === noteId &&
      outcome?.table.rows.some(current => current.noteId === noteId && !current.readOnly) ? row : null;
  }

  function addFilter(): void {
    if (effectiveKey === '') {
      return;
    }
    const filter = toFilter({ key: effectiveKey, op: draftOp, value: draftValue });
    if (filter === null) {
      setNeedsFilterValue(true);
      return;
    }
    setNeedsFilterValue(false);
    setFilters((current) => [...current, filter]);
    setDraftValue('');
  }

  return (
    <aside
      className="database-panel"
      onDragEnd={() => { draggedRowRef.current = null; }}
      aria-labelledby="database-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          requestClose();
        }
      }}
    >
      <header className="database-head">
        <h2 id="database-heading">{translator('database.title')}</h2>
        <button
          type="button"
          className="database-close"
          aria-label={translator('database.close')}
          title={translator('database.close')}
          onClick={requestClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <div className="database-view-file" role="group" aria-label={translator('database.viewFile.label')}>
        <span className="database-view-file-name">
          {viewFile === null ? translator('database.viewFile.untitled') : viewNameFromPath(viewFile.path)}
        </span>
        <span className="database-view-file-state" role="status">
          {translator(viewSaving ? 'database.viewFile.state.saving'
            : viewFile === null ? 'database.viewFile.state.unsaved'
              : viewDirty ? 'database.viewFile.state.dirty' : 'database.viewFile.state.saved')}
        </span>
        {viewFile !== null ? (
          <button
            type="button"
            className="primary"
            disabled={viewSaving || !viewDirty || viewConflict || viewFile.readOnly}
            onClick={() => void saveView(viewFile.path, viewFile.sha256)}
          >
            {translator('database.viewFile.save')}
          </button>
        ) : null}
        <label className="database-view-file-field">
          <span>{translator('database.viewFile.newName')}</span>
          <input type="text" value={viewName} onChange={(event) => setViewName(event.target.value)} />
        </label>
        <button
          type="button"
          className="secondary"
          disabled={viewSaving || viewPathFromName(viewName) === null}
          onClick={() => {
            const target = viewPathFromName(viewName);
            if (target !== null) void saveView(target, null);
          }}
        >
          {translator(viewFile === null ? 'database.viewFile.saveAs' : 'database.viewFile.saveCopy')}
        </button>
        {viewFiles.length > 0 ? (
          <label className="database-view-file-field">
            <span>{translator('database.viewFile.open')}</span>
            <select
              value={viewChoice}
              onChange={(event) => {
                setViewChoice(event.target.value);
                if (event.target.value !== '') void openView(event.target.value);
              }}
            >
              <option value="">{translator('database.viewFile.choose')}</option>
              {viewFiles.map((path) => <option key={path} value={path}>{viewNameFromPath(path)}</option>)}
            </select>
          </label>
        ) : null}
        {viewConflict && viewFile !== null ? (
          <button type="button" className="secondary" onClick={() => void openView(viewFile.path, true)}>
            {translator('database.viewFile.reload')}
          </button>
        ) : null}
        {viewStatus !== null ? (
          <p className="database-view-file-status" role="status">{translator(viewStatus.key, viewStatus.params)}</p>
        ) : null}
        {viewError !== null ? (
          <p className="database-view-file-status" role="alert">{serviceErrorMessage(locale, viewError.code, viewError.messageKey)}</p>
        ) : null}
      </div>

      <div className="database-views" role="group" aria-label={translator('database.viewLabel')}>
        {DATABASE_VIEWS.map((entry) => {
          const Icon = VIEW_ICONS[entry];
          return (
            <button
              key={entry}
              type="button"
              aria-pressed={view === entry}
              onClick={() => setView(entry)}
            >
              <Icon size={15} aria-hidden="true" />
              {translator(`database.view.${entry}`)}
            </button>
          );
        })}
      </div>

      <div className="database-controls">
        <label className="database-folder">
          <span>{translator('database.folder')}</span>
          <input
            type="text"
            value={folder}
            placeholder={translator('database.folderPlaceholder')}
            onChange={(event) => {
              rowDraftVersion.current += 1;
              setFolder(event.target.value);
              setRowError(null);
            }}
          />
        </label>
        <label className="database-recursive">
          <input
            type="checkbox"
            checked={recursive}
            onChange={(event) => setRecursive(event.target.checked)}
          />
          {translator('database.recursive')}
        </label>
      </div>

      {canWrite ? (
        <form className="database-new-row" onSubmit={(event) => void submitRow(event)}>
          <label>
            <span>{translator('database.newRow')}</span>
            <input
              type="text"
              value={rowTitle}
              placeholder={translator('database.newRowTitle')}
              disabled={creating || writing !== null}
              onChange={(event) => {
                rowDraftVersion.current += 1;
                setRowTitle(event.target.value);
                setRowError(null);
              }}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && event.nativeEvent.isComposing) {
                  event.preventDefault();
                }
              }}
            />
          </label>
          <button type="submit" className="secondary" disabled={creating || writing !== null}>
            <Plus size={15} aria-hidden="true" /> {translator('database.createRow')}
          </button>
        </form>
      ) : null}
      {createdRowTitle !== null ? (
        <p className="database-hint" role="status">
          {translator('database.newRowCreated', { title: createdRowTitle })}
        </p>
      ) : null}
      {rowError !== null ? (
        <p className="database-row-error" role="alert">
          {typeof rowError === 'string'
            ? translator(rowError === 'title-required' ? 'database.newRowTitleRequired' : 'database.newRowFolderInvalid')
            : serviceErrorMessage(locale, rowError.code)}
        </p>
      ) : null}

      <div className="database-filter-builder" role="group" aria-label={translator('database.addFilter')}>
        <label>
          <span>{translator('database.column')}</span>
          <select value={effectiveKey} onChange={(event) => setDraftKey(event.target.value)}>
            {columnOptions.map((key) => (
              <option key={key} value={key}>{key}</option>
            ))}
          </select>
        </label>
        <label>
          <span>{translator('database.condition')}</span>
          <select
            value={draftOp}
            onChange={(event) => setDraftOp(event.target.value as DatabaseFilter['op'])}
          >
            {operatorsFor(columns.find((column) => column.key === effectiveKey)?.kind ?? 'text').map(
              (op) => (
                <option key={op} value={op}>
                  {translator(`database.op.${op}`)}
                </option>
              ),
            )}
          </select>
        </label>
        <label>
          <span>{translator('database.value')}</span>
          <input
            type="text"
            value={draftValue}
            disabled={draftOp === 'isSet' || draftOp === 'isUnset'}
            onChange={(event) => setDraftValue(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
                event.preventDefault();
                addFilter();
              }
            }}
          />
        </label>
        <button type="button" className="secondary" onClick={addFilter}>
          <Filter size={15} aria-hidden="true" /> {translator('database.addFilter')}
        </button>
      </div>

      {needsFilterValue ? (
        <p className="database-hint" role="status">
          {translator('database.filterNeedsValue')}
        </p>
      ) : null}
      {writeNotice ? (
        <p className="database-hint" role="status">
          {translator('database.writeSaved')}
        </p>
      ) : null}
      {writeError !== null ? (
        <div className="database-error" role="alert">
          <p>{writeError.messageKey === 'database.sourceChanged' ? translator('database.sourceChanged') : serviceErrorMessage(locale, writeError.code)}</p>
          <div className="database-error-actions">
            {failedWrite !== null ? (
              <button
                type="button"
                className="secondary"
                disabled={writing !== null}
                onClick={() => void writeCell(failedWrite.row, failedWrite.name, failedWrite.value, true)}
              >
                {translator('database.writeRetry')}
              </button>
            ) : null}
            <button
              type="button"
              className="secondary"
              onClick={() => {
                setWriteError(null);
                setFailedWrite(null);
              }}
            >
              {translator('database.writeDismiss')}
            </button>
          </div>
        </div>
      ) : null}

      {activeFilters.length > 0 ? (
        <ul className="database-active-filters" aria-label={translator('database.activeFilters')}>
          {activeFilters.map((filter, index) => (
            <li
              key={`${filter.key}:${index}`}
              data-inactive={filter.problem !== null ? 'true' : undefined}
            >
              <span>
                {filter.column} {translator(`database.op.${filter.op}`)}
                {filter.value !== '' ? ` ${filter.value}` : ''}
              </span>
              {filter.problem !== null ? (
                <small className="database-filter-inactive">
                  {translator(
                    filter.problem === 'missing-column'
                      ? 'database.filterMissingColumn'
                      : 'database.filterOperatorMismatch',
                  )}
                </small>
              ) : null}
              <button
                type="button"
                className="database-drop-filter"
                aria-label={translator('database.removeFilter')}
                onClick={() => setFilters((current) => current.filter((_, at) => at !== index))}
              >
                <X size={13} aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      {inactiveFilters.length > 0 ? (
        <div className="database-warning database-warning-block" role="alert">
          <p>{translator('database.inactiveFilters', { count: inactiveFilters.length })}</p>
          <button type="button" className="secondary" onClick={removeInactiveFilters}>
            {translator('database.removeInactiveFilters')}
          </button>
        </div>
      ) : null}

      {loading ? <p role="status">{translator('database.loading')}</p> : null}
      {error !== null ? (
        <div className="database-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
          <button type="button" className="secondary" onClick={() => void load()}>
            {translator('database.retry')}
          </button>
        </div>
      ) : null}
      {outcome !== null && outcome.unreadable.length > 0 ? (
        <p className="database-warning" role="status">
          {translator('database.unreadable', { count: outcome.unreadable.length })}
        </p>
      ) : null}

      {outcome !== null && isEmptyDueToFilters(outcome.table) ? (
        <div className="database-empty" role="status">
          <p>{translator('database.emptyFiltered')}</p>
          <button type="button" className="secondary" onClick={() => setFilters([])}>
            {translator('database.clearFilters')}
          </button>
        </div>
      ) : null}
      {outcome !== null && summary !== null && summary.total === 0 ? (
        <p className="database-empty-text">{translator('database.empty')}</p>
      ) : null}

      {view === 'table' && outcome !== null && outcome.table.rows.length > 0 ? (
        <div className="database-scroll">
          <table className="database-table">
            <caption className="visually-hidden">{translator('database.caption')}</caption>
            <thead>
              <tr>
                <th scope="col">{translator('database.note')}</th>
                {columns.map((column) => (
                  <th key={column.key} scope="col" aria-sort={ariaSort(sort, column.key)}>
                    <button
                      type="button"
                      className="database-sort"
                      onClick={() => setSort((current) => nextSort(current, column.key))}
                    >
                      {column.key}
                      <small>{column.kind}</small>
                    </button>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {outcome.table.rows.map((row) => (
                <tr key={rowKey(row)}>
                  <th scope="row">
                    <button type="button" className="database-open" onClick={() => onOpenNote(row.noteId)}>
                      {row.title}
                    </button>
                  </th>
                  {row.cells.map((cell, index) => (
                    <td
                      key={`${row.noteId}:${columns[index]?.key ?? index}`}
                      data-unset={isUnset(cell) ? 'true' : undefined}
                    >
                        {cellText(cell, region, translator('database.unset'), timeZone)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}

      {view === 'board' && outcome !== null && outcome.table.rows.length > 0 ? (
        selectColumns.length === 0 ? (
          <p className="database-empty-text" role="status">
            {translator('database.boardNoSelect')}
          </p>
        ) : (
          <>
            <label className="database-view-field">
              <span>{translator('database.groupBy')}</span>
              <select value={effectiveGroupKey} onChange={(event) => setGroupKey(event.target.value)}>
                {selectColumns.map((column) => (
                  <option key={column.key} value={column.key}>{column.key}</option>
                ))}
              </select>
            </label>
            <div
              className="database-board"
              role="group"
              aria-label={translator('database.view.board')}
            >
              {boardGroups.map((group) => (
                <section
                  className="database-board-column"
                  key={group.value ?? 'ungrouped'}
                  aria-label={group.value ?? translator('database.ungrouped')}
                  onDragOver={canGroupWrite ? (event) => event.preventDefault() : undefined}
                  onDrop={canGroupWrite ? (event) => {
                    const row = droppedRow(event);
                    if (row === null || groupValueOf(row) === group.value) {
                      return;
                    }
                    void writeCell(
                      row,
                      effectiveGroupKey,
                      group.value === null
                        ? { kind: 'unset' }
                        : { kind: 'select', text: group.value },
                    );
                  } : undefined}
                >
                  <h3>
                    {group.value ?? translator('database.ungrouped')}
                    <small>{group.rows.length}</small>
                  </h3>
                  <ul>
                    {group.rows.map((row) => (
                      <li
                        key={row.noteId}
                        draggable={canGroupWrite && !row.readOnly}
                        onDragStart={canGroupWrite ? (event) => startDrag(event, row) : undefined}
                      >
                        <button
                          type="button"
                          className="database-open"
                          onClick={() => onOpenNote(row.noteId)}
                        >
                          {row.title}
                        </button>
                        <small>{row.relativePath}</small>
                        {canGroupWrite ? (
                          <label className="database-cell-edit">
                            <span className="visually-hidden">
                              {translator('database.changeStatus', { title: row.title })}
                            </span>
                            <select
                              value={groupValueOf(row) ?? ''}
                              disabled={writing !== null || row.readOnly}
                              onChange={(event) => {
                                const next = event.target.value;
                                void writeCell(
                                  row,
                                  effectiveGroupKey,
                                  next === ''
                                    ? { kind: 'unset' }
                                    : { kind: 'select', text: next },
                                );
                              }}
                            >
                              <option value="">{translator('database.ungrouped')}</option>
                              {selectChoices(outcome.table, effectiveGroupKey).map((choice) => (
                                <option key={choice} value={choice}>{choice}</option>
                              ))}
                            </select>
                          </label>
                        ) : null}
                      </li>
                    ))}
                  </ul>
                </section>
              ))}
            </div>
          </>
        )
      ) : null}

      {view === 'gallery' && outcome !== null && outcome.table.rows.length > 0 ? (
        <div
          className="database-gallery"
          role="list"
          aria-label={translator('database.view.gallery')}
        >
          {outcome.table.rows.map((row) => (
            <article className="database-gallery-card" role="listitem" key={row.noteId}>
              <div className="database-gallery-cover">{translator('database.noCover')}</div>
              <button
                type="button"
                className="database-open"
                onClick={() => onOpenNote(row.noteId)}
              >
                {row.title}
              </button>
              <p>{row.relativePath}</p>
            </article>
          ))}
        </div>
      ) : null}

      {view === 'calendar' && outcome !== null && outcome.table.rows.length > 0 ? (
        dateColumns.length === 0 ? (
          <p className="database-empty-text" role="status">
            {translator('database.calendarNoDate')}
          </p>
        ) : (
          <>
            <div className="database-calendar-controls">
              <label className="database-view-field">
                <span>{translator('database.dateField')}</span>
                <select value={effectiveDateKey} onChange={(event) => setDateKey(event.target.value)}>
                  {dateColumns.map((column) => (
                    <option key={column.key} value={column.key}>{column.key}</option>
                  ))}
                </select>
              </label>
              <div className="database-month">
                <button
                  type="button"
                  aria-label={translator('database.previousMonth')}
                  title={translator('database.previousMonth')}
                  onClick={() => setMonth((current) => shiftMonth(current, -1))}
                >
                  <ChevronLeft size={18} aria-hidden="true" />
                </button>
                <span aria-live="polite">{monthLabel}</span>
                <button
                  type="button"
                  aria-label={translator('database.nextMonth')}
                  title={translator('database.nextMonth')}
                  onClick={() => setMonth((current) => shiftMonth(current, 1))}
                >
                  <ChevronRight size={18} aria-hidden="true" />
                </button>
              </div>
            </div>
            <div className="database-calendar-scroll">
              <table className="database-calendar-table">
                <caption className="visually-hidden">
                  {translator('database.view.calendar')} — {monthLabel}
                </caption>
                <thead>
                  <tr>
                    {weekdays.map((key) => (
                      <th key={key} scope="col">{translator(key)}</th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {weeks.map((week, weekIndex) => (
                    <tr key={weekIndex}>
                      {week.map((date, dayIndex) => (
                        <td
                          key={date ?? `blank-${weekIndex}-${dayIndex}`}
                          onDragOver={canWrite && date !== null ? (event) => event.preventDefault() : undefined}
                          onDrop={canWrite && date !== null ? (event) => {
                            const row = droppedRow(event);
                            if (
                              row === null ||
                              dateCellValue(row, outcome.table, effectiveDateKey) === date
                            ) {
                              return;
                            }
                            void writeCell(row, effectiveDateKey, { kind: 'date', date });
                          } : undefined}
                        >
                          {date === null ? null : (
                            <>
                              <span className="visually-hidden">
                                {formatCalendarDate(date, region, 'medium')}
                              </span>
                              <span className="database-calendar-date" aria-hidden="true">
                                {Number(date.slice(-2))}
                              </span>
                              <ul>
                                {(datedByDay.get(date) ?? []).map((row) => (
                                  <li
                                    key={row.noteId}
                                    draggable={canWrite && !row.readOnly}
                                    onDragStart={canWrite ? (event) => startDrag(event, row) : undefined}
                                  >
                                    <button
                                      type="button"
                                      className="database-open"
                                      onClick={() => onOpenNote(row.noteId)}
                                    >
                                      {row.title}
                                    </button>
                                    {canWrite ? (
                                      <input
                                        type="date"
                                        className="database-date-edit"
                                        aria-label={translator('database.changeDate', { title: row.title })}
                                        value={dateCellValue(row, outcome.table, effectiveDateKey) ?? ''}
                                        disabled={writing !== null || row.readOnly}
                                        onChange={(event) => {
                                          const next = event.target.value;
                                          if (next === '') {
                                            void writeCell(row, effectiveDateKey, { kind: 'unset' });
                                            return;
                                          }
                                          if (/^\d{4}-\d{2}-\d{2}$/.test(next)) {
                                            void writeCell(row, effectiveDateKey, { kind: 'date', date: next });
                                          }
                                        }}
                                      />
                                    ) : null}
                                  </li>
                                ))}
                              </ul>
                            </>
                          )}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <section
              className="database-unscheduled"
              aria-labelledby="database-unscheduled-heading"
            >
              <h3 id="database-unscheduled-heading">
                {translator('database.unscheduled')}
                <small>{calendar.unscheduled.length}</small>
              </h3>
              <ul>
                {calendar.unscheduled.map((row) => (
                  <li
                    key={row.noteId}
                    draggable={canWrite && !row.readOnly}
                    onDragStart={canWrite ? (event) => startDrag(event, row) : undefined}
                  >
                    <button
                      type="button"
                      className="database-open"
                      onClick={() => onOpenNote(row.noteId)}
                    >
                      {row.title}
                    </button>
                    <small>{row.relativePath}</small>
                    {canWrite ? (
                      <input
                        type="date"
                        className="database-date-edit"
                        aria-label={translator('database.changeDate', { title: row.title })}
                        value={dateCellValue(row, outcome.table, effectiveDateKey) ?? ''}
                        disabled={writing !== null || row.readOnly}
                        onChange={(event) => {
                          const next = event.target.value;
                          if (next === '') {
                            void writeCell(row, effectiveDateKey, { kind: 'unset' });
                            return;
                          }
                          if (/^\d{4}-\d{2}-\d{2}$/.test(next)) {
                            void writeCell(row, effectiveDateKey, { kind: 'date', date: next });
                          }
                        }}
                      />
                    ) : null}
                  </li>
                ))}
              </ul>
            </section>
          </>
        )
      ) : null}

      {summary !== null ? (
        <p className="database-summary" role="status">
          {summary.hidden === 0
            ? translator('database.summaryAll', { shown: summary.shown })
            : translator('database.summaryFiltered', {
                shown: summary.shown,
                hidden: summary.hidden,
              })}
        </p>
      ) : null}
      {view !== 'table' ? (
        <p className="database-view-note" role="note">
          {canWrite ? translator('database.viewsWritable') : translator('database.viewsReadOnly')}
        </p>
      ) : null}
      <p className="database-footnote">{translator('database.footnote')}</p>
    </aside>
  );
}
