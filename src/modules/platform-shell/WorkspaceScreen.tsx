import { WorkspaceSidebar, SidebarRail, type SidebarTool } from './WorkspaceSidebar';
import { NoteBar, type BarAction } from './NoteBar';
import { ToastStack, useAutoDismiss, type ToastItem } from './Toast';
import { observeCatalog } from './observe-catalog';
import { createCatalogSync } from './catalog-sync';
import { CatalogNotice } from './CatalogNotice';
import { observeSources } from './observe-sources';
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type RefObject } from 'react';
import type { NoteDocument, NoteSummary, RecoverOutcome, ServiceError, Vault } from '../domain';
import type { MessageKey, SupportedLocaleId, Translator } from '../presentation-i18n';
import { saveFailureMessages, serviceErrorMessage } from '../presentation-i18n';
import {
  CHECKPOINT_INTERVAL_MS,
  EditorWorkflow,
  IDLE_SAVE_DELAY_MS,
  INLINE_ATTACHMENT_FOLDER,
  MAX_INLINE_ATTACHMENT_BYTES,
  NoteEditor,
  attachmentCandidates,
  createImageLoader,
  itemMarkdown,
  pastedFileName,
  type AttachmentHost,
  SaveController,
  type EditorMode,
  type AssistApplier,
  type NoteSaveSnapshot,
  type NoteService,
  type RenamePlan,
  type TagWriteRunner,
  type CatalogRefresh,
  type SaveStatus,
  type WorkflowSchedule,
} from '../editor';
import { newOperationId } from './tauri-note-service';
import { folderOf, NoteCreationController, untitledPath } from './note-creation';
import { PanelGuardRegistry, panelGuardId, type PanelGuard } from './panel-guards';
import type { FindKeys } from '../editor/find-replace';
import { NoteTitle } from './NoteTitle';
import { canCommit } from './rename-view';
import { serviceErrorFromThrown } from './port';
import { resolveLinkTarget, type AnchorCandidate, type LinkCandidate } from '../editor/link-picker';
import { embedTargetIds, resolveEmbedTree, type EmbedTree } from '../editor/embed-resolution';
import { CheckpointController } from '../editor/checkpoint-controller';
import { DraftRecoveryPanel } from './DraftRecoveryPanel';
import { hasDraftCheckpoint } from '../editor/draft-recovery';
import type { CreateCopyRequest, NoteDraft } from '../editor/ports';
import { NoteTree, TreeMenu, treeKey, type TreeMenuItem, type TreeNaming, type TreeProject, type TreeTarget } from './NoteTree';
import { FolderActionDialog, type FolderActionPhase } from './FolderActionDialog';
import { droppedPath, planBatchDelete, planFolderMove, runSteps, type FolderMove, type Step } from './folder-ops';
import { batchDeleteSteps, linkSummary, moveStep, moveSteps, type FolderActionsIo } from './folder-actions';
import { Archive, Command, Download, FileClock, FileSearch, FileText, LayoutDashboard, LayoutTemplate, ListChecks, Network, Settings2, Stethoscope, Table, X } from 'lucide-react';
import { splitFrontMatter } from '../editor/markdown-render';
import { HistoryPanel } from '../history-backup/HistoryPanel';
import { MetadataPanel } from '../metadata-links/MetadataPanel';
import { BacklinksFooter } from '../metadata-links/BacklinksFooter';
import { ConflictPanel } from './ConflictPanel';
import { DeleteNoteDialog, type TrashMutationRunner } from '../history-backup/DeleteNoteDialog';
import { TrashPanel } from '../history-backup/TrashPanel';
import { BackupPanel } from '../history-backup/BackupPanel';
import type { BackupService, Overwritten, Upgraded } from '../history-backup/ports';
import { SearchPanel } from './SearchPanel';
import { CommandPalette, QuickOpen } from './CommandSurface';
import { OutlinePanel } from './OutlinePanel';
import { TagPanel } from './TagPanel';
import { PropertyPanel } from './PropertyPanel';
import { ExportPanel } from './ExportPanel';
import { ImportPanel } from './ImportPanel';
import { TaskPanel } from './TaskPanel';
import { writeTaskSource } from './task-view';
import { TemplatePanel } from './TemplatePanel';
import { GraphPanel } from './GraphPanel';
import { DatabasePanel } from './DatabasePanel';
import { writeDatabaseCell } from './database-write';
import { CanvasPanel } from './CanvasPanel';
import { DiagnosticsPanel } from './DiagnosticsPanel';
import { ProjectWizardPanel } from './ProjectWizardPanel';
import { START_RECENT_LIMIT, StartPanel } from './StartPanel';
import { LatexToolbar } from './LatexToolbar';
import { InsertPanel } from './InsertPanel';
import { CitationPanel } from './CitationPanel';
import { ProjectInspectPanel } from './ProjectInspectPanel';
import { resolveLatexBuilds, resolveLatexEnvironment, resolveLatexService } from './tauri-latex-service';
import { LatexEnvironmentPanel } from './LatexEnvironmentPanel';
import { LatexWorkspace } from './LatexWorkspace';
import { RenameDialog } from './RenameDialog';
import { duplicatePath } from './duplicate-note';
import { AttachmentPanel } from './AttachmentPanel';
import { PdfReaderPanel } from './PdfReaderPanel';
import { SplitControls } from './SplitControls';
import {
  applySelection,
  initialLayout,
  isNarrow,
  selectionOf,
  type PaneLayout,
  type PaneSelection,
} from './panes';
import { closeTab, cycleTab, isFavorite, recentLabel, resolveFavorites, toggleFavorite } from './tabs';
import { buildOutline, toggleCollapsed, type OutlineRow } from './outline';
import { currentOutline, observeOutline, type OutlineSnapshot } from './observe-outline';
import type { CommandAvailability, CommandDefinition } from './commands';
import {
  clampSidebarWidth,
  favoriteNoteLimit,
  pushRecent,
  readDevicePreferences,
  recentCommandLimit,
  recentNoteLimit,
  writeDevicePreferences,
} from './device-prefs';
import {
  chordMatcher,
  chordText,
  effectiveBinding,
  resolveStep,
  type BindableCommandId,
  type Chord,
  type ChordPrefix,
} from './shortcuts';
import { bindableCommand } from './command-catalogue';
import { formatNumber, localCalendarDate, type RegionalPreferences } from './regional';
import { applyTemplateInsert, sameTemplateTarget, noteNameFor, type TemplateInsertTarget } from './template-view';
import { actionFor, chosenDate, shiftDate, statusKey } from './daily-view';
import { dailySettingsFor, effectiveDailyPattern, withDailySettings } from './daily-settings';
import { DailySettingsPanel } from './DailySettingsPanel';

export type VaultAction = 'create' | 'open';

type DuplicateOutcome =
  | { kind: 'created'; path: string }
  | { kind: 'failed'; error: ServiceError };

export interface GuardBlockedEntry {
  noteId: string;
  path: string | null;
  status: SaveStatus | 'working';
}

export interface WorkspaceGate {
  blockedEntries(): readonly GuardBlockedEntry[];
  saveBlockedAndWait(): Promise<boolean>;
  exportBlocked(): Promise<'exported' | 'retained'>;
}

export interface WorkspaceGateInputs {
  saveController: SaveController;
  noteService: NoteService;
  pathOf(noteId: string): string | undefined;
  bufferOf(noteId: string): string | undefined;
  pendingOperation(): { noteId: string; path: string; done: Promise<void> } | null;
  creation?: Pick<NoteCreationController, 'snapshot' | 'retry'>;
  /** Canvas and database-view panels with unsaved work. */
  panels?: () => readonly PanelGuard[];
}

export function createWorkspaceGate(inputs: WorkspaceGateInputs): WorkspaceGate {
  const { saveController, noteService, pathOf, bufferOf } = inputs;
  return {
    blockedEntries(): readonly GuardBlockedEntry[] {
      const entries: GuardBlockedEntry[] = saveController.notesNeedingAttentionOnExit().map((entry) => ({
        noteId: entry.noteId,
        path: pathOf(entry.noteId) ?? null,
        status: entry.status,
      }));
      const pending = inputs.pendingOperation();
      if (pending && !entries.some(entry => entry.noteId === pending.noteId)) entries.push({ noteId: pending.noteId, path: pending.path, status: 'working' });
      const creation = inputs.creation?.snapshot();
      if (creation?.request) entries.push({ noteId: `creation:${creation.request.operationId}`, path: creation.request.relativePath, status: creation.working ? 'working' : 'failed' });
      for (const panel of inputs.panels?.() ?? []) {
        entries.push({ noteId: panelGuardId(panel), path: panel.label(), status: panel.status() });
      }
      return entries;
    },
    async saveBlockedAndWait(): Promise<boolean> {
      const pending = inputs.pendingOperation();
      if (pending) await pending.done;
      if (inputs.creation?.snapshot().request && !(await inputs.creation.retry()).ok) return false;
      const blocked = saveController.notesNeedingAttentionOnExit();
      const panels = inputs.panels?.() ?? [];
      const [, panelResults] = await Promise.all([
        Promise.all(
          blocked.map((entry) =>
            Promise.resolve(saveController.requestSave(entry.noteId)).then(
              () => undefined,
              () => undefined,
            ),
          ),
        ),
        Promise.all(panels.map((panel) => panel.save().catch(() => false))),
      ]);
      return inputs.pendingOperation() === null && !inputs.creation?.snapshot().request &&
        saveController.notesNeedingAttentionOnExit().length === 0 &&
        panelResults.every(Boolean);
    },
    async exportBlocked(): Promise<'exported' | 'retained'> {
      if (inputs.pendingOperation() || inputs.creation?.snapshot().working) return 'retained';
      const blocked = saveController.notesNeedingAttentionOnExit();
      const creation = inputs.creation?.snapshot().request;
      const panels = inputs.panels?.() ?? [];
      let allExported = blocked.length > 0 || panels.length > 0 || creation !== null && creation !== undefined;
      for (const panel of panels) {
        const draft = panel.draft();
        const outcome = await noteService.exportDraft({ noteId: panelGuardId(panel), relativePath: draft.relativePath, source: draft.source });
        if (outcome.ok && outcome.value.outcome === 'saved') panel.rescued();
        else allExported = false;
      }
      if (creation) {
        const outcome = await noteService.exportDraft({ noteId: creation.operationId, relativePath: creation.relativePath, source: creation.source });
        allExported = outcome.ok && outcome.value.outcome === 'saved';
      }
      for (const entry of blocked) {
        const relativePath = pathOf(entry.noteId);
        const source = bufferOf(entry.noteId);
        if (relativePath === undefined || source === undefined) {
          allExported = false;
          continue;
        }
        const outcome = await noteService.exportDraft({
          noteId: entry.noteId,
          relativePath,
          source,
        });
        if (!(outcome.ok && outcome.value.outcome === 'saved')) {
          allExported = false;
        }
      }
      return allExported && inputs.pendingOperation() === null && !inputs.creation?.snapshot().working ? 'exported' : 'retained';
    },
  };
}

export interface WorkspaceScreenProps {
  translator: Translator;
  locale: SupportedLocaleId;
  regional: RegionalPreferences;
  vault: Vault;
  noteService: NoteService;
  backupService: BackupService;
  saveController: SaveController;
  gateRef: RefObject<WorkspaceGate | null>;
  onNavigate?: (screen: 'home' | 'settings') => void;
  onUpgraded(value: Upgraded): 'accepted' | 'wait' | 'rejected';
  onOverwritten(value: Overwritten): 'accepted' | 'wait' | 'rejected';
  /** False while another top-level screen is visible; keeps the shortcut scoped. */
  active: boolean;
  shortcutBindings: Readonly<Partial<Record<BindableCommandId, Chord>>>;
}

type NotesView =
  | { kind: 'loading' }
  | { kind: 'ready'; notes: readonly NoteSummary[] }
  | { kind: 'failed'; error: ServiceError };

type DocView =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'failed'; error: ServiceError };

const setStatusKeys: Record<SaveStatus, MessageKey> = {
  clean: 'editor.save.saved',
  dirty: 'editor.save.dirty',
  saving: 'editor.save.saving',
  failed: 'editor.save.failed.title',
  conflict: 'editor.conflict.title',
};

const setTimeoutSchedule: WorkflowSchedule['schedule'] = (fn, delayMs) => {
  const timer = setTimeout(fn, delayMs);
  return () => {
    clearTimeout(timer);
  };
};

/**
 * The readable title of a note: its file stem without `.md`. This mirrors the
 * host's `title_of`, so the S17 picker shows the same title the index resolves
 * against.
 */
function noteTitle(relativePath: string): string {
  const name = relativePath.split(/[/\\]/).pop() ?? relativePath;
  return name.replace(/\.md$/i, '');
}

export function WorkspaceScreen({
  translator,
  locale,
  regional,
  vault,
  noteService,
  backupService,
  saveController,
  gateRef,
  onUpgraded,
  onOverwritten,
  active,
  shortcutBindings,
  onNavigate,
}: WorkspaceScreenProps) {
  const [notesView, setNotesView] = useState<NotesView>({ kind: 'loading' });
  const [catalogResult, setCatalogResult] = useState<CatalogRefresh | null>(null);
  const [catalogError, setCatalogError] = useState<ServiceError | null>(null);
  const [catalogWatchError, setCatalogWatchError] = useState<ServiceError | null>(null);
  const [catalogRetry, setCatalogRetry] = useState(0);
  const [catalogReadErrors, setCatalogReadErrors] = useState<Record<string, ServiceError>>({});
  const [docState, setDocState] = useState<DocView>({ kind: 'idle' });
  const [activeNoteId, setActiveNoteId] = useState<string | null>(null);
  const [activeDoc, setActiveDoc] = useState<NoteDocument | null>(null);
  const [buffers, setBuffers] = useState<Record<string, string>>({});
  const [snapshots, setSnapshots] = useState<Record<string, NoteSaveSnapshot>>({});
  const [mode, setMode] = useState<EditorMode>('live');
  const [otherCreateBusy, setOtherCreateBusy] = useState(false);
  // The new note whose title takes focus once it opens.
  const [titleFocus, setTitleFocus] = useState<{ noteId: string; token: number } | null>(null);
  const [titleError, setTitleError] = useState<ServiceError | null>(null);
  // The destination a title rename hands to the rename dialog when links must change.
  const [renameTarget, setRenameTarget] = useState<string | null>(null);
  const [creationResult, setCreationResult] = useState<Pick<NoteDocument, 'noteId' | 'relativePath'> | null>(null);
  const [createError, setCreateError] = useState<ServiceError | null>(null);
  const [duplicateOutcome, setDuplicateOutcome] = useState<DuplicateOutcome | null>(null);
  // A failed copy is retried with its original operation id so a request that
  // already committed cannot become a second file (F03 §6).
  const duplicateRequestRef = useRef<CreateCopyRequest | null>(null);
  const [recovery, setRecovery] = useState<RecoverOutcome | null>(null);
  const [recoveryBusy, setRecoveryBusy] = useState(false);
  const [recoveryError, setRecoveryError] = useState<ServiceError | null>(null);
  const [draftAvailable, setDraftAvailable] = useState(false);
  const [checkpointNotice, setCheckpointNotice] = useState<ServiceError | null>(null);
  const [exportNotice, setExportNotice] = useState<'saved' | 'cancelled' | null>(null);
  const [exportError, setExportError] = useState<ServiceError | null>(null);
  const [composing, setComposing] = useState(false);
  const [draftsOpen, setDraftsOpen] = useState(false);
  const [restored, setRestored] = useState(false);
  const [editorVersion, setEditorVersion] = useState(0);
  // F04: the controls own the device-only layout and report it here so the
  // workspace can render one editor per pane over the same buffer.
  const [paneLayout, setPaneLayout] = useState<PaneLayout>(() => initialLayout(null));
  const [paneSelections, setPaneSelections] = useState<readonly PaneSelection[]>([]);
  const [viewportWidth, setViewportWidth] = useState(() =>
    typeof window === 'undefined' ? 1280 : window.innerWidth,
  );
  const paneLayoutRef = useRef(paneLayout);
  paneLayoutRef.current = paneLayout;
  const paneSelectionsRef = useRef(paneSelections);
  paneSelectionsRef.current = paneSelections;
  const assistAppliersRef = useRef(new Map<string, AssistApplier>());
  const [paneFocusRequest, setPaneFocusRequest] = useState<{
    paneId: string;
    nonce: number;
  } | null>(null);
  const paneFocusNonceRef = useRef(0);
  // Folders on disk, empty ones included; read again whenever the note list is.
  const [folders, setFolders] = useState<readonly string[]>([]);
  // A folder being named in place: a new one, or a rename.
  const [namingFolder, setNamingFolder] = useState<TreeNaming | null>(null);
  // A folder rename, move or delete under review or running; steps resume where they stopped.
  const [folderOp, setFolderOp] = useState<{
    title: string; summary: readonly string[]; confirmLabel: string; danger: boolean;
    phase: FolderActionPhase; steps: readonly Step[];
  } | null>(null);
  const [namingError, setNamingError] = useState<string | null>(null);
  const [treeMenu, setTreeMenu] = useState<{ target: TreeTarget; x: number; y: number } | null>(null);
  // A note picked for deletion from the tree; the dialog opens once it is the active note.
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [openNotes, setOpenNotes] = useState<NoteSummary[]>([]);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [metadataOpen, setMetadataOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [outlineOpen, setOutlineOpen] = useState(false);
  const [tagsOpen, setTagsOpen] = useState(false);
  const [propertiesOpen, setPropertiesOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  // A two-step chord pauses after its first step; the text is kept for the
  // visible status line, the prefix for matching the second step.
  const [shortcutPrefix, setShortcutPrefix] = useState<{
    prefix: ChordPrefix;
    text: string;
  } | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [tasksOpen, setTasksOpen] = useState(false);
  const [templatesOpen, setTemplatesOpen] = useState(false);
  const [templateTarget, setTemplateTarget] = useState<TemplateInsertTarget | null>(null);
  const [graphOpen, setGraphOpen] = useState(false);
  const [databaseOpen, setDatabaseOpen] = useState(false);
  const [canvasOpen, setCanvasOpen] = useState(false);
  const [diagnosticsOpen, setDiagnosticsOpen] = useState(false);
  const [wizardOpen, setWizardOpen] = useState(false);
  const [latexToolbarOpen, setLatexToolbarOpen] = useState(false);
  const [insertOpen, setInsertOpen] = useState(false);
  const [citationOpen, setCitationOpen] = useState(false);
  const [inspectOpen, setInspectOpen] = useState(false);
  const latexService = useMemo(() => resolveLatexService(), []);
  const latexEnvironment = useMemo(() => resolveLatexEnvironment(), []);
  const latexBuilds = useMemo(() => resolveLatexBuilds(), []);
  // The registered project open in the LaTeX workspace, if any.
  const [latexProjectId, setLatexProjectId] = useState<string | null>(null);
  // Projects by folder, so the tree can mark them and open them in place.
  const [latexProjects, setLatexProjects] = useState<ReadonlyMap<string, TreeProject>>(new Map());
  const [wizardParent, setWizardParent] = useState('');
  const [latexReload, setLatexReload] = useState(0);
  // Rows picked with Ctrl/Shift in the tree; the anchor is where a Shift range starts.
  const [treeSelection, setTreeSelection] = useState<readonly TreeTarget[]>([]);
  const selectionAnchor = useRef<string | null>(null);
  const selectedKeys = useMemo(() => new Set(treeSelection.map(treeKey)), [treeSelection]);
  // The sidebar lists the vault's projects; reread whenever a LaTeX surface closes,
  // since creating, importing and removing all happen there.
  useEffect(() => {
    if (wizardOpen || inspectOpen || latexProjectId !== null) return;
    let cancelled = false;
    void latexService.listProjects(vault.vaultId).then((outcome) => {
      if (cancelled || !outcome.ok) return;
      setLatexProjects(new Map(outcome.value.filter((project) => project.readable).map((project) => [
        (project.folder ?? '').replace(/\/+$/, ''),
        { projectId: project.projectId, entry: project.entryPath?.split('/').pop() || 'main.tex' },
      ])));
    });
    return () => { cancelled = true; };
  }, [latexService, vault.vaultId, wizardOpen, inspectOpen, latexProjectId, latexReload]);
  // The same projects, by name, for the sidebar's own LaTeX section.
  const sidebarProjects = useMemo(() => [...latexProjects].map(([folder, project]) => ({
    projectId: project.projectId,
    name: folder === '' ? project.entry.replace(/\.tex$/i, '') : folder.split('/').pop() ?? folder,
    folder,
  })).sort((a, b) => a.name.localeCompare(b.name)), [latexProjects]);
  const [latexEnvOpen, setLatexEnvOpen] = useState(false);
  const [renameOpen, setRenameOpen] = useState(false);
  const [attachmentOpen, setAttachmentOpen] = useState(false);
  const [attachmentNotice, setAttachmentNotice] = useState<string | null>(null);
  const [pdfReader, setPdfReader] = useState<{ path: string; page: number | null; target: TemplateInsertTarget | null } | null>(null);
  const imageLoader = useMemo(() => createImageLoader(noteService), [noteService]);
  useEffect(() => () => imageLoader.dispose(), [imageLoader]);
  const attachmentHost = useMemo<AttachmentHost>(() => ({
    loadImage: imageLoader.load,
    report: setAttachmentNotice,
    async storeFile(notePath, file) {
      const name = pastedFileName(file.name, file.type, new Date());
      if (file.size > MAX_INLINE_ATTACHMENT_BYTES) return { ok: false, message: translator('attachment.dropTooLarge', { name }) };
      const failed = { ok: false as const, message: translator('attachment.dropFailed', { name }) };
      const bytes = await file.arrayBuffer().catch(() => null);
      if (bytes === null) return failed;
      const stored = await noteService.storeAttachmentBytes({
        destination: INLINE_ATTACHMENT_FOLDER, name, bytes: new Uint8Array(bytes), operationId: crypto.randomUUID(),
      });
      if (!stored.ok || !(stored.value.status === 'stored' || stored.value.status === 'alreadyPresent')) return failed;
      return { ok: true, markdown: itemMarkdown(notePath, stored.value) };
    },
  }), [imageLoader, noteService, translator]);
  const [attachmentTarget, setAttachmentTarget] = useState<TemplateInsertTarget | null>(null);
  // The editor registers its apply function so an assist action is one
  // transaction, and therefore one undo step (F47-R02).
  const [assistApplier, setAssistApplier] = useState<AssistApplier | null>(null);
  const [dailyDate, setDailyDate] = useState(() => localCalendarDate(new Date()));
  const [dailyError, setDailyError] = useState<ServiceError | null>(null);
  const [dailyNotice, setDailyNotice] = useState<ReturnType<typeof statusKey> | null>(null);
  // Only "created" and "opened" are plain news; the other daily states need an answer and stay put.
  const dailyToast = dailyNotice === 'daily.created' || dailyNotice === 'daily.opened';
  useAutoDismiss(dailyToast, () => setDailyNotice(null));
  useAutoDismiss(duplicateOutcome?.kind === 'created', () => setDuplicateOutcome(null));
  useAutoDismiss(exportNotice !== null, () => setExportNotice(null));
  useAutoDismiss(restored, () => setRestored(false));
  const [linkMissing, setLinkMissing] = useState<string | null>(null);
  useAutoDismiss(linkMissing !== null, () => setLinkMissing(null));
  useAutoDismiss(attachmentNotice !== null, () => setAttachmentNotice(null));
  const [dailySettingsOpen, setDailySettingsOpen] = useState(false);
  const [dailyTemplatePrompt, setDailyTemplatePrompt] = useState<{ date: string } | null>(null);
  const [dailyChoices, setDailyChoices] = useState<
    readonly { noteId: string; relativePath: string; propertyDate: string | null }[]
  >([]);
  const [selection, setSelection] = useState<{ from: number; to: number } | null>(null);
  // F04: each pane's selection is remembered separately. Switching the active
  // pane restores that pane's selection for the insert commands without ever
  // letting one view overwrite the other's caret.
  useEffect(() => {
    const active = selectionOf(paneSelectionsRef.current, paneLayout.activePaneId);
    if (active !== null) {
      setSelection({ from: active.from, to: active.to });
    }
    const apply = assistAppliersRef.current.get(paneLayout.activePaneId);
    if (apply !== undefined) {
      setAssistApplier(() => apply);
    }
  }, [paneLayout.activePaneId]);
  useEffect(() => {
    setPaneSelections(current => {
      const live = new Set(paneLayout.panes.map(pane => pane.paneId));
      const next = current.filter(item => live.has(item.paneId));
      return next.length === current.length ? current : next;
    });
    for (const paneId of assistAppliersRef.current.keys()) {
      if (!paneLayout.panes.some(pane => pane.paneId === paneId)) {
        assistAppliersRef.current.delete(paneId);
      }
    }
  }, [paneLayout.panes]);
  useEffect(() => {
    function onResize(): void {
      setViewportWidth(window.innerWidth);
    }
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => readDevicePreferences().sidebarCollapsed);
  const [sidebarWidth, setSidebarWidth] = useState(() => readDevicePreferences().sidebarWidth ?? 264);
  // The width when a drag began; each move is measured from it.
  const sidebarDragFrom = useRef<number | null>(null);
  const [favorites, setFavorites] = useState<readonly string[]>(
    () => readDevicePreferences().favoriteNotes,
  );
  const [outlineSnapshot, setOutlineSnapshot] = useState<OutlineSnapshot | null>(null);
  const [outlineRetry, setOutlineRetry] = useState(0);
  const [caretLine, setCaretLine] = useState<number | null>(null);
  const [collapsedOutline, setCollapsedOutline] = useState<readonly string[]>(
    () => readDevicePreferences().collapsedOutlineIds,
  );
  const [quickOpen, setQuickOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [recentCommands, setRecentCommands] = useState<readonly string[]>(
    () => readDevicePreferences().recentCommands,
  );
  const [recentNotes, setRecentNotes] = useState<readonly string[]>(
    () => readDevicePreferences().recentNotes,
  );
  const [searchJump, setSearchJump] = useState<{ noteId: string; offset: number; nonce: number } | null>(null);
  const [historyRestoring, setHistoryRestoring] = useState(false);
  const [conflictLocal, setConflictLocal] = useState<NoteDocument | null>(null);
  const trashFlight = useRef<{ noteId: string; path: string; done: Promise<void> } | null>(null);
  const dailyFlight = useRef<{ noteId: string; path: string; done: Promise<void> } | null>(null);
  const backupFlight = useRef<{ noteId: string; path: string; done: Promise<void> } | null>(null);
  const [backupOpen, setBackupOpen] = useState(false);
  const [backupBusy, setBackupBusy] = useState(false);
  const [trashOpen, setTrashOpen] = useState(false);
  const [trashBusy, setTrashBusy] = useState(false);
  const [trashInitial, setTrashInitial] = useState<string | null>(null);
  const [deleteLocal, setDeleteLocal] = useState<NoteDocument | null>(null);
  // S17 anchors: fetched on demand per note and cached, because the picker asks
  // synchronously while the user types. A note is fetched at most once until
  // the vault reloads, which keeps typing responsive without going stale across
  // an edit that changes headings (the cache is cleared on save).
  const anchorCacheRef = useRef<Map<string, readonly AnchorCandidate[]>>(new Map());
  const anchorPendingRef = useRef<Set<string>>(new Set());
  const [, setAnchorTick] = useState(0);

  const anchorsOf = useCallback(
    (noteId: string): readonly AnchorCandidate[] => {
      const cached = anchorCacheRef.current.get(noteId);
      if (cached !== undefined) {
        return cached;
      }
      if (!anchorPendingRef.current.has(noteId)) {
        anchorPendingRef.current.add(noteId);
        void noteService.readNoteMetadata(noteId).then(outcome => {
          anchorPendingRef.current.delete(noteId);
          if (!outcome.ok) {
            return;
          }
          const anchors: AnchorCandidate[] = [
            ...outcome.value.headings.map(heading => ({
              id: heading.slug,
              text: heading.text,
              level: heading.level,
            })),
            ...outcome.value.blockAnchors.map(block => ({
              id: block.id,
              text: null,
              level: null,
            })),
          ];
          anchorCacheRef.current.set(noteId, anchors);
          setAnchorTick(tick => tick + 1);
        });
      }
      return [];
    },
    [noteService],
  );

  const checkpoints = useMemo(() => new CheckpointController(noteService, newOperationId), [noteService]);


  // S17 targets: every known note, with the title M04 would use. Derived from
  // the note list rather than a separate query so the picker cannot offer a
  // note the tree does not show.
  const linkCandidates = useMemo<readonly LinkCandidate[]>(() => {
    if (notesView.kind !== 'ready') {
      return [];
    }
    return notesView.notes.map(note => ({
      noteId: note.noteId,
      relativePath: note.relativePath,
      title: noteTitle(note.relativePath),
    }));
  }, [notesView]);

  const lifetimeActiveRef = useRef(true);
  const creationAllowedRef = useRef(false);
  const creation = useMemo(() => new NoteCreationController({
    createNote: request => noteService.createNote(request),
    newOperationId,
    canCreate: () => lifetimeActiveRef.current && creationAllowedRef.current && !dailyFlight.current,
  }), [noteService]);
  const [creationSnapshot, setCreationSnapshot] = useState(() => creation.snapshot());
  const createBusy = otherCreateBusy || creationSnapshot.working;
  const dailyBusy = createBusy || composing || historyRestoring || trashBusy || backupBusy || creationSnapshot.uncertain;
  const creationActivateRef = useRef(true);
  const creationRetryRef = useRef(() => creation.retry());
  useEffect(() => creation.subscribe(snapshot => {
    setCreationSnapshot(snapshot);
    setCreateError(snapshot.error);
  }), [creation]);
  // A new note's title is selected once; returning to it later does not reselect it.
  useEffect(() => {
    if (titleFocus !== null && activeNoteId !== null && activeNoteId !== titleFocus.noteId) setTitleFocus(null);
    setTitleError(null);
  }, [activeNoteId, titleFocus]);

  const activeNoteIdRef = useRef<string | null>(null);
  const pathsRef = useRef<Map<string, string>>(new Map());
  const catalogObserverRef = useRef<ReturnType<typeof observeCatalog> | null>(null);
  const catalogFingerprintRef = useRef<string | null>(null);
  const catalogOpenNotesRef = useRef(openNotes);
  catalogOpenNotesRef.current = openNotes;
  const catalogActiveRef = useRef(active);
  catalogActiveRef.current = active;
  const catalogAllowedRef = useRef(false);
  const notesSessionRef = useRef(0);
  const recoverySessionRef = useRef(0);
  const noteSessionRef = useRef(0);
  const searchJumpNonceRef = useRef(0);
  // F11-A01: a source note can change while the referencing note is open. Its
  // id is recorded after each resolution so only that commit (or a window
  // focus, which covers edits made elsewhere) re-resolves the tree.
  const [embedRefreshTick, setEmbedRefreshTick] = useState(0);
  const embedSourceIdsRef = useRef<ReadonlySet<string>>(new Set());

  useEffect(() => {
    if (!active) return;
    const observer = observeCatalog(noteService, {
      refreshed: acceptCatalog,
      watching: () => setCatalogWatchError(null),
      watchFailed: setCatalogWatchError,
      scanFailed: error => { setCatalogError(error); setNotesView(previous => previous.kind === 'loading' ? { kind: 'failed', error } : previous); },
    });
    catalogObserverRef.current = observer;
    void observer.refresh();
    return () => { observer.stop(); if (catalogObserverRef.current === observer) catalogObserverRef.current = null; };
  }, [active, noteService, catalogRetry]);

  const workflowCallbacksRef = useRef<{ run: (noteId: string) => void }>({
    run: () => undefined,
  });
  const workflow = useMemo(
    () =>
      new EditorWorkflow(
        {
          onSaveDue: (noteId) => {
            void saveController.requestSave(noteId);
          },
          onCheckpointDue: (noteId) => {
            workflowCallbacksRef.current.run(noteId);
          },
        },
        {
          idleSaveDelayMs: IDLE_SAVE_DELAY_MS,
          checkpointIntervalMs: CHECKPOINT_INTERVAL_MS,
          schedule: setTimeoutSchedule,
        },
      ),
    [saveController],
  );

  useEffect(() => {
    const unsubscribe = saveController.subscribe((event) => {
      setSnapshots((prev) => ({ ...prev, [event.noteId]: event.snapshot }));
      if (event.snapshot.status === 'clean') {
        workflow.markCommitted(event.noteId);
        workflow.bufferCleared(event.noteId);
        setCheckpointNotice((current) => (current === null ? current : null));
        // A commit may have added, renamed or removed headings, so the cached
        // anchor list for that note is no longer trustworthy.
        anchorCacheRef.current.delete(event.noteId);
        // Saving a note that is embedded on screen changes what the reader
        // should show (F11-A01); unrelated saves do not rebuild the tree.
        if (embedSourceIdsRef.current.has(event.noteId)) {
          setEmbedRefreshTick(tick => tick + 1);
        }
      }
    });
    return unsubscribe;
  }, [saveController, workflow]);

  // An embed source may have changed on disk while the window was in the
  // background. Re-resolve on return; the backend rebuilds its index per call,
  // so this reads the current version rather than a cached copy.
  useEffect(() => {
    const refresh = (): void => setEmbedRefreshTick(tick => tick + 1);
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, []);

  async function runCheckpoint(noteId: string): Promise<void> {
    const relativePath = pathsRef.current.get(noteId);
    const source = saveController.getBufferText(noteId);
    const snapshot = saveController.snapshot(noteId);
    if (relativePath === undefined || source === null || !snapshot?.dirty) {
      return;
    }
    const outcome = await checkpoints.write({ noteId, relativePath, source, baseRevision: snapshot.revision, baseContentHash: snapshot.contentHash });
    if (!lifetimeActiveRef.current) {
      return;
    }
    if (outcome !== null) {
      setCheckpointNotice(outcome.ok ? null : outcome.error);
    }
  }

  function acceptCatalog(value: CatalogRefresh): void {
    if (!catalogActiveRef.current || !lifetimeActiveRef.current) return;
    notesSessionRef.current += 1;
    setCatalogError(null);
    const changed = catalogFingerprintRef.current !== value.fingerprint;
    catalogFingerprintRef.current = value.fingerprint;
    setCatalogResult(previous => previous?.fingerprint === value.fingerprint ? previous : value);
    const unavailable = new Set(value.unavailableNoteIds);
    const notes = value.notes.filter(note => !unavailable.has(note.noteId));
    if (value.deferred) { setNotesView(previous => previous.kind === 'ready' ? previous : { kind: 'ready', notes }); return; }
    for (const note of value.notes) pathsRef.current.set(note.noteId, note.relativePath);
    setNotesView(previous => !changed && previous.kind === 'ready' ? previous : { kind: 'ready', notes });
    if (changed) { anchorCacheRef.current.clear(); setEmbedRefreshTick(tick => tick + 1); }
  }

  async function refreshNotes(options?: { silent?: boolean }): Promise<void> {
    if (options?.silent !== true) setNotesView({ kind: 'loading' });
    const observer = catalogObserverRef.current;
    if (observer) { await observer.refresh(); return; }
    const session = ++notesSessionRef.current;
    const outcome = await noteService.refreshCatalog();
    if (session !== notesSessionRef.current || !lifetimeActiveRef.current || !catalogActiveRef.current) return;
    if (outcome.ok) acceptCatalog(outcome.value);
    else { setCatalogError(outcome.error); setNotesView({ kind: 'failed', error: outcome.error }); }
  }

  useEffect(() => {
    if (notesView.kind !== 'ready') return;
    let current = true;
    void noteService.listFolders().then((outcome) => { if (current && outcome.ok) setFolders(outcome.value); });
    return () => { current = false; };
  }, [notesView, noteService]);

  useEffect(() => {
    if (pendingDelete === null || activeDoc?.noteId !== pendingDelete) return;
    setPendingDelete(null);
    beginDelete();
    // beginDelete reads the active note it was waiting for.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeDoc, pendingDelete]);

  async function reloadFolders(): Promise<void> {
    const listed = await noteService.listFolders();
    if (listed.ok) setFolders(listed.value);
  }

  async function nameFolder(name: string): Promise<void> {
    if (namingFolder === null) return;
    if (namingFolder.mode === 'rename') {
      const path = namingFolder.path;
      const parent = folderOf(path);
      setNamingFolder(null);
      setNamingError(null);
      await beginFolderMove(path, parent === '' ? name : `${parent}/${name}`);
      return;
    }
    const parent = namingFolder.parent;
    const outcome = await noteService.createFolder(parent === '' ? name : `${parent}/${name}`);
    if (!outcome.ok) {
      setNamingError(translator(outcome.error.code === 'PATH_COLLISION' ? 'workspace.folder.exists' : 'workspace.folder.invalid'));
      return;
    }
    setNamingFolder(null);
    setNamingError(null);
    await reloadFolders();
  }

  function beginFolder(parent: string): void {
    setNamingError(null);
    setNamingFolder({ mode: 'create', parent });
  }

  /** Saves every edited note first: moves and deletes work on what is on disk. */
  async function saveAllBeforeFolderOp(): Promise<boolean> {
    if (!writable || composing || createBusy || historyRestoring || trashFlight.current || backupFlight.current || folderOp !== null) return false;
    for (const entry of saveController.notesNeedingAttentionOnExit()) {
      if (await saveActiveBeforeActivation(entry.noteId)) return false;
    }
    return saveController.notesNeedingAttentionOnExit().length === 0;
  }

  function openFolderOp(title: string, confirmLabel: string, danger: boolean): void {
    setFolderOp({ title, summary: [], confirmLabel, danger, phase: { kind: 'checking' }, steps: [] });
  }

  function refuseFolderOp(refusal: string, otherFiles: readonly string[] = [], otherCount = 0): void {
    setFolderOp((op) => op === null ? op : { ...op, phase: { kind: 'review', refusal, otherFiles, otherCount } });
  }

  /** A refusal when the folder holds more than notes, which only notes can be carried. */
  async function folderRefusal(path: string): Promise<boolean> {
    const contents = await noteService.inspectFolder(path);
    if (!contents.ok) { refuseFolderOp(serviceErrorMessage(locale, contents.error.code)); return true; }
    if (contents.value.hasLink) { refuseFolderOp(translator('folderOp.refuseLink')); return true; }
    if (contents.value.otherCount > 0) {
      refuseFolderOp(translator('folderOp.refuseFiles'), contents.value.otherFiles, contents.value.otherCount);
      return true;
    }
    return false;
  }

  // The vault calls behind folder moves and deletes; the steps themselves live in folder-actions.
  const folderIo: FolderActionsIo = {
    notePath: async (noteId) => {
      const current = await noteService.readNote(noteId);
      return current.ok ? current.value.relativePath : null;
    },
    previewNoteRename: (noteId, toPath) => noteService.previewNoteRename(noteId, toPath),
    applyRename: (plan, expectedRevision, operationId) => applyRename(plan, expectedRevision, operationId),
    createFolder: (path) => noteService.createFolder(path),
    removeEmptyFolder: (path) => noteService.removeEmptyFolder(path),
    recycleFolder: (path) => noteService.recycleFolder(path),
    renameEmptyFolder: (from, to) => noteService.renameEmptyFolder(from, to),
    trashNote: (note, operationId) => runTrash(note.noteId, note.relativePath, () => noteService.deleteNote({
      noteId: note.noteId, expectedRevision: note.localRevision, expectedHash: note.contentHash, operationId,
    })),
    afterTrash: (noteId) => reconcileTrash(noteId),
    newOperationId,
  };

  async function moveSummary(moves: readonly FolderMove[]): Promise<string[]> {
    const counted = await linkSummary(moves, folderIo);
    const lines = [translator('folderOp.linkSummary', { links: counted.links, notes: counted.notes })];
    if (counted.untouched > 0) lines.push(translator('folderOp.untouched', { count: counted.untouched }));
    return lines;
  }

  async function beginFolderMove(from: string, to: string): Promise<void> {
    if (!(await saveAllBeforeFolderOp())) return;
    openFolderOp(translator('folderOp.moveTitle', { from, to }), translator('folderOp.moveConfirm'), false);
    const [listedNotes, listedFolders] = await Promise.all([noteService.listNotes(), noteService.listFolders()]);
    if (!listedNotes.ok) { refuseFolderOp(serviceErrorMessage(locale, listedNotes.error.code)); return; }
    if (!listedFolders.ok) { refuseFolderOp(serviceErrorMessage(locale, listedFolders.error.code)); return; }
    const plan = planFolderMove(listedNotes.value, listedFolders.value, from, to);
    if (!plan.ok) {
      if (plan.reason === 'same') setFolderOp(null);
      else refuseFolderOp(translator(`folderOp.refuse.${plan.reason}`));
      return;
    }
    if (await folderRefusal(from)) return;
    const steps = moveSteps(plan, folderIo);
    const summary = [translator('folderOp.moveSummary', { count: plan.moves.length, from, to }), ...(await moveSummary(plan.moves))];
    setFolderOp((op) => op === null ? op : { ...op, summary, steps, phase: { kind: 'review', refusal: null, otherFiles: [], otherCount: 0 } });
  }

  async function beginNoteMove(noteId: string, from: string, to: string): Promise<void> {
    if (from === to || !(await saveAllBeforeFolderOp())) return;
    openFolderOp(translator('folderOp.moveTitle', { from, to }), translator('folderOp.moveConfirm'), false);
    const move = { noteId, from, to };
    const summary = [translator('folderOp.moveSummary', { count: 1, from, to }), ...(await moveSummary([move]))];
    setFolderOp((op) => op === null ? op : { ...op, summary, steps: [moveStep(move, folderIo)], phase: { kind: 'review', refusal: null, otherFiles: [], otherCount: 0 } });
  }

  async function beginFolderDelete(path: string): Promise<void> {
    await beginBatchDelete([{ kind: 'folder', path }], translator('folderOp.deleteTitle', { path }));
  }

  /** Ctrl/Cmd toggles one row; Shift takes every visible row between the anchor and this one. */
  function selectInTree(target: TreeTarget, mode: 'toggle' | 'range'): void {
    const key = treeKey(target);
    if (mode === 'range' && selectionAnchor.current !== null) {
      const rows = Array.from(document.querySelectorAll<HTMLElement>('.file-tree [data-tree-key]'))
        .filter((row) => row.getClientRects().length > 0)
        .map((row) => row.dataset.treeKey ?? '');
      const from = rows.indexOf(selectionAnchor.current);
      const to = rows.indexOf(key);
      if (from >= 0 && to >= 0) {
        const keys = rows.slice(Math.min(from, to), Math.max(from, to) + 1);
        setTreeSelection(keys.flatMap((rowKey) => targetOfKey(rowKey) ?? []));
        return;
      }
    }
    selectionAnchor.current = key;
    setTreeSelection((current) => current.some((item) => treeKey(item) === key)
      ? current.filter((item) => treeKey(item) !== key)
      : [...current, target]);
  }

  function targetOfKey(key: string): TreeTarget | null {
    if (key.startsWith('folder:')) return { kind: 'folder', path: key.slice('folder:'.length) };
    const noteId = key.slice('note:'.length);
    const note = notesView.kind === 'ready' ? notesView.notes.find((item) => item.noteId === noteId) : undefined;
    return note === undefined ? null : { kind: 'note', noteId, path: note.relativePath };
  }

  /** Trashes the selected notes and removes the selected folders, skipping folders that hold other files. */
  async function beginBatchDelete(targets: readonly TreeTarget[], title?: string): Promise<void> {
    if (!(await saveAllBeforeFolderOp())) return;
    setTreeSelection([]);
    openFolderOp(title ?? translator('folderOp.batchTitle', { count: targets.length }), translator('folderOp.deleteConfirm'), true);
    const listed = await noteService.listNotes();
    if (!listed.ok) { refuseFolderOp(serviceErrorMessage(locale, listed.error.code)); return; }
    // Links are refused; other files send the folder to the recycle bin.
    const kinds = new Map<string, 'link' | 'files'>();
    const otherFiles: string[] = [];
    let otherCount = 0;
    for (const target of targets) {
      if (target.kind !== 'folder' || target.path === '') continue;
      const contents = await noteService.inspectFolder(target.path);
      if (!contents.ok) { refuseFolderOp(serviceErrorMessage(locale, contents.error.code)); return; }
      if (contents.value.hasLink) kinds.set(target.path, 'link');
      else if (contents.value.otherCount > 0) {
        kinds.set(target.path, 'files');
        otherFiles.push(...contents.value.otherFiles);
        otherCount += contents.value.otherCount;
      }
    }
    const plan = planBatchDelete(targets, listed.value, kinds);
    const skipped = plan.skipped.length > 0 ? [translator('folderOp.batchSkipped', { paths: plan.skipped.join(', ') })] : [];
    if (plan.notes.length === 0 && plan.folders.length === 0 && plan.recycle.length === 0) {
      refuseFolderOp([translator('folderOp.batchNothing'), ...skipped].join(' '));
      return;
    }
    const shown = otherFiles.slice(0, 20);
    const recycled = plan.recycle.length === 0 ? [] : [
      translator('folderOp.recycleSummary', { paths: plan.recycle.join(', ') }),
      translator('folderOp.recycleFiles', { files: shown.join(', '), count: otherCount }),
    ];
    const summary = [
      ...(plan.notes.length > 0 || plan.folders.length > 0 ? [translator('folderOp.batchSummary', { notes: plan.notes.length, folders: plan.folders.length })] : []),
      ...recycled,
      ...skipped,
      ...(plan.notes.length > 0 ? [translator('folderOp.trashHint')] : []),
    ];
    const steps = batchDeleteSteps(plan, folderIo);
    setFolderOp((op) => op === null ? op : { ...op, summary, steps, phase: { kind: 'review', refusal: null, otherFiles: [], otherCount: 0 } });
  }

  /** Stops treating a folder as a LaTeX project; its files stay where they are. */
  async function removeLatexProject(projectId: string): Promise<void> {
    const outcome = await latexService.unregisterProject(vault.vaultId, projectId, newOperationId());
    if (!outcome.ok) setTitleError(outcome.error);
    setLatexReload((value) => value + 1);
  }

  async function runFolderOp(start: number): Promise<void> {
    const op = folderOp;
    if (op === null) return;
    const total = op.steps.length;
    const result = await runSteps(op.steps, start, (done) => setFolderOp((current) => current === null ? current : { ...current, phase: { kind: 'running', done, total } }));
    await refreshNotes({ silent: true });
    await reloadFolders();
    setFolderOp((current) => current === null ? current : {
      ...current,
      phase: result.error === null
        ? { kind: 'done', total }
        : { kind: 'stopped', done: result.done, total, error: serviceErrorMessage(locale, result.error.code) },
    });
  }

  /** F20: follows a PDF link from a note, trying the same places an image would. */
  async function openPdfLink(notePath: string, target: string, page: number | null): Promise<void> {
    for (const path of attachmentCandidates(notePath, target)) {
      const info = await noteService.attachmentInfo(path);
      if (info.ok) {
        setPdfReader({ path, page, target: templateInsertionTarget() });
        return;
      }
      if (info.error.code !== 'OBJECT_NOT_FOUND') break;
    }
    setAttachmentNotice(translator('pdfReader.missing', { name: target }));
  }

  function dropOnFolder(source: TreeTarget, folder: string): void {
    if (!writable) return;
    if (source.kind === 'note') {
      void beginNoteMove(source.noteId, source.path, droppedPath(source.path, folder));
      return;
    }
    const to = droppedPath(source.path, folder);
    if (folder === source.path || to === source.path) return;
    void beginFolderMove(source.path, to);
  }

  function treeMenuItems(target: TreeTarget): TreeMenuItem[] {
    const blocked = !writable || createBusy || creationSnapshot.uncertain;
    if (treeSelection.length > 1 && selectedKeys.has(treeKey(target))) {
      return [
        { key: 'deleteMany', label: translator('workspace.tree.deleteSelected', { count: treeSelection.length }), disabled: !writable, run: () => void beginBatchDelete(treeSelection) },
        { key: 'clear', label: translator('workspace.tree.clearSelection'), run: () => setTreeSelection([]) },
      ];
    }
    if (target.kind === 'folder') {
      const project = latexProjects.get(target.path);
      const items: TreeMenuItem[] = [
        ...(project !== undefined ? [{ key: 'project', label: translator('workspace.tree.openLatex'), run: () => setLatexProjectId(project.projectId) }] : []),
        { key: 'note', label: translator('workspace.tree.newNote'), disabled: blocked, run: () => void quickCreate(target.path) },
        { key: 'folder', label: translator('workspace.tree.newFolder'), disabled: !writable, run: () => beginFolder(target.path) },
        { key: 'latex', label: translator('sidebar.newLatexProject'), disabled: !writable, run: () => { setWizardParent(target.path); setWizardOpen(true); } },
        ...(project !== undefined ? [{ key: 'unregister', label: translator('workspace.tree.removeLatex'), disabled: !writable, run: () => void removeLatexProject(project.projectId) }] : []),
      ];
      if (target.path !== '') {
        items.push(
          { key: 'rename', label: translator('workspace.tree.rename'), disabled: !writable, run: () => { setNamingError(null); setNamingFolder({ mode: 'rename', path: target.path }); } },
          { key: 'delete', label: translator('workspace.tree.delete'), disabled: !writable, run: () => void beginFolderDelete(target.path) },
        );
      }
      return items;
    }
    return [
      { key: 'open', label: translator('workspace.tree.open'), run: () => void openNote(target.noteId) },
      {
        key: 'rename', label: translator('workspace.tree.rename'), disabled: !writable,
        run: () => void openNote(target.noteId).then((opened) => { if (opened) setTitleFocus({ noteId: target.noteId, token: Date.now() }); }),
      },
      {
        key: 'delete', label: translator('workspace.tree.delete'), disabled: !writable,
        run: () => { setPendingDelete(target.noteId); void openNote(target.noteId); },
      },
    ];
  }

  function applyDocument(doc: NoteDocument): void {
    setOpenNotes(previous => previous.some(note => note.noteId === doc.noteId) ? previous : [...previous, doc]);
    setEditorVersion(version => version + 1);
    setRestored(false);
    pathsRef.current.set(doc.noteId, doc.relativePath);
    saveController.registerDocument(doc);
    const authoritativeSource = saveController.getBufferText(doc.noteId) ?? doc.source;
    setBuffers((prev) =>
      prev[doc.noteId] === authoritativeSource
        ? prev
        : { ...prev, [doc.noteId]: authoritativeSource },
    );
    setActiveNoteId(doc.noteId);
    activeNoteIdRef.current = doc.noteId;
    setActiveDoc(doc);
    setDocState({ kind: 'ready' });
    setExportNotice(null);
    setExportError(null);
    setMode(doc.readOnly || !writable ? 'read' : 'live');
    workflow.noteActivated(doc.noteId);
  }

  async function saveActiveBeforeActivation(noteId: string): Promise<boolean> {
    const needsAttention = (blocked: readonly { noteId: string; status: SaveStatus }[]) =>
      blocked.some((entry) => entry.noteId === noteId);
    if (!needsAttention(saveController.notesNeedingAttentionOnExit())) {
      return false;
    }
    await Promise.resolve(saveController.requestSave(noteId)).then(
      () => undefined,
      () => undefined,
    );
    return needsAttention(saveController.notesNeedingAttentionOnExit());
  }

  async function openNote(noteId: string): Promise<boolean> {
    if (historyRestoring || trashFlight.current || backupFlight.current) return false;
    setDailyNotice(null);
    // A pending search jump belongs to the note that was current when it was
    // requested; opening anything else must not apply it.
    setSearchJump(null);
    const session = ++noteSessionRef.current;
    if (activeNoteId !== null && activeNoteId !== noteId) {
      const blocked = await saveActiveBeforeActivation(activeNoteId);
      if (session !== noteSessionRef.current || !lifetimeActiveRef.current) {
        return false;
      }
      if (blocked) {
        return false;
      }
    }
    setDocState({ kind: 'loading' });
    const outcome = await noteService.readNote(noteId);
    if (session !== noteSessionRef.current || !lifetimeActiveRef.current) {
      return false;
    }
    if (!outcome.ok) {
      setDocState({ kind: 'failed', error: outcome.error });
      return false;
    }
    applyDocument(outcome.value);
    return true;
  }

  workflowCallbacksRef.current = {
    run: (noteId: string) => {
      void runCheckpoint(noteId);
    },
  };

  useEffect(() => {
    if (!active) {
      setSearchOpen(false);
      setQuickOpen(false);
      setPaletteOpen(false);
      setShortcutPrefix(null);
      return;
    }
    const handler = (event: KeyboardEvent) => {
      const anotherSurfaceOpen =
        metadataOpen || tagsOpen || draftsOpen || historyOpen || trashOpen || backupOpen || renameOpen || conflictLocal !== null || deleteLocal !== null;
      if (anotherSurfaceOpen) {
        if (shortcutPrefix !== null) {
          setShortcutPrefix(null);
        }
        return;
      }
      // F30: the resolved binding decides which command runs, so a stored
      // override actually changes behaviour instead of only being validated.
      const resolved = resolveStep(
        shortcutBindings,
        event,
        shortcutPrefix?.prefix ?? null,
      );
      if (resolved.kind === 'reserved') {
        // The OS owns this combination; the app must not swallow it.
        if (shortcutPrefix !== null) {
          setShortcutPrefix(null);
        }
        return;
      }
      // Find and replace belong to the focused editor, which handles its own keys.
      if ((resolved.kind === 'prefix' || resolved.kind === 'command') &&
        bindableCommand(resolved.kind === 'prefix' ? resolved.prefix.command : resolved.command).surface === 'editor') {
        return;
      }
      if (resolved.kind === 'prefix') {
        // A two-step chord is a state, not a keystroke: it needs a visible pause
        // the user can finish or cancel.
        event.preventDefault();
        const first = effectiveBinding(shortcutBindings, resolved.prefix.command)?.[0];
        setShortcutPrefix({
          prefix: resolved.prefix,
          text: first !== undefined ? chordText([first]) : resolved.prefix.key,
        });
        return;
      }
      if (resolved.kind === 'none') {
        if (shortcutPrefix !== null) {
          setShortcutPrefix(null);
        }
        // Ctrl+Tab and Ctrl+Shift+Tab are not rebindable and stay hard-coded.
        if (event.ctrlKey && event.key === 'Tab') {
          if (openNotes.length < 2) {
            return;
          }
          event.preventDefault();
          void switchDocumentTab(event.shiftKey ? -1 : 1);
        }
        return;
      }
      event.preventDefault();
      if (shortcutPrefix !== null) {
        setShortcutPrefix(null);
      }
      const target = bindableCommand(resolved.command).surface;
      setQuickOpen(target === 'quickOpen');
      setPaletteOpen(target === 'palette');
      setSearchOpen(target === 'search');
      setExportOpen(target === 'export');
      setImportOpen(target === 'import');
      if (target === 'rename') void beginRename();
      if (target === 'create' && writable) void quickCreate();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [active, metadataOpen, tagsOpen, draftsOpen, historyOpen, trashOpen, backupOpen, renameOpen, conflictLocal, deleteLocal, openNotes, activeNoteId, activeDoc, notesView, composing, createBusy, historyRestoring, trashBusy, backupBusy, shortcutBindings, shortcutPrefix]);

  // A half-entered chord must not stay armed forever: clear it on timeout or
  // when the window loses focus.
  useEffect(() => {
    if (shortcutPrefix === null) {
      return;
    }
    const timer = window.setTimeout(() => setShortcutPrefix(null), 4000);
    const clear = (): void => setShortcutPrefix(null);
    window.addEventListener('blur', clear);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener('blur', clear);
    };
  }, [shortcutPrefix]);

  useEffect(() => {
    void refreshNotes();
    void (async () => {
      const session = ++recoverySessionRef.current;
      const outcome = await noteService.recoverVault();
      if (session !== recoverySessionRef.current) {
        return;
      }
      if (outcome.ok) {
        setRecovery(outcome.value);
        setRecoveryError(null);
      } else {
        setRecoveryError(outcome.error);
      }
      // F07-R02: a restart checks drafts as well as the journal. This only
      // surfaces their existence; loading one still goes through the panel's
      // compare-before-restore step.
      const drafts = await noteService.listDrafts({ limit: 1 });
      if (session !== recoverySessionRef.current) {
        return;
      }
      setDraftAvailable(hasDraftCheckpoint(drafts));
    })();
  }, []);

  async function retryRecovery(): Promise<void> {
    const session = ++recoverySessionRef.current;
    setRecoveryBusy(true);
    const outcome = await noteService.recoverVault();
    if (session !== recoverySessionRef.current) {
      return;
    }
    setRecoveryBusy(false);
    if (outcome.ok) {
      setRecovery(outcome.value);
      setRecoveryError(null);
    } else {
      setRecoveryError(outcome.error);
    }
  }

  function openDraftRecovery(): void {
    // The notice is a reminder, not a second recovery path: the panel owns the
    // comparison and explicit restore. Opening it settles the reminder.
    setDraftAvailable(false);
    setDraftsOpen(true);
  }

  useEffect(() => {
    lifetimeActiveRef.current = true;
    return () => {
      lifetimeActiveRef.current = false;
      notesSessionRef.current += 1;
      recoverySessionRef.current += 1;
      noteSessionRef.current += 1;
      workflow.dispose();
    };
  }, [workflow]);

  /**
   * F17: opens (or creates) the daily note for a date.
   *
   * Pending work joins the close/switch gate; an ambiguous day opens a chooser
   * instead of merging two notes (F17-R04).
   */
  async function openDaily(date: string, templateOverride?: string | null): Promise<void> {
    if (!lifetimeActiveRef.current || dailyFlight.current || trashFlight.current || backupFlight.current ||
        dailyBusy || creation.snapshot().working || creation.snapshot().uncertain) {
      return;
    }
    const originSession = noteSessionRef.current;
    let finish!: () => void;
    const done = new Promise<void>(resolve => { finish = resolve; });
    dailyFlight.current = { noteId: `daily:${date}`, path: date, done };
    setOtherCreateBusy(true);
    setDailyError(null);
    setDailyNotice(null);
    setDailyTemplatePrompt(null);
    try {
      const settings = dailySettingsFor(readDevicePreferences(), vault.vaultId);
      // `null` explicitly asks for a blank note after the host reported a missing
      // template; `undefined` keeps the stored choice.
      const templateId =
        templateOverride === undefined ? (settings.templateId ?? undefined) : (templateOverride ?? undefined);
      const outcome = await noteService.openDailyNote(
        date,
        effectiveDailyPattern(settings),
        templateId,
      );
      if (!lifetimeActiveRef.current) {
        return;
      }
      if (!outcome.ok) {
        setDailyError(outcome.error);
        return;
      }
      if (noteSessionRef.current !== originSession) {
        await refreshNotes({ silent: true });
        return;
      }
      const action = actionFor(outcome.value);
      if (action.kind === 'template-missing') {
        setDailyChoices([]);
        setDailyTemplatePrompt({ date });
        return;
      }
      if (action.kind === 'failed') {
        setDailyError({
          code: 'INVALID_INPUT',
          messageKey: action.reason,
          params: {},
          retryable: false,
          operationId: null,
          retryAfterSeconds: null,
        });
        return;
      }
      if (action.kind === 'choose') {
        // Two notes claim the day; keep the current note until the user chooses.
        setDailyChoices(action.candidates);
        return;
      }
      setDailyChoices([]);
      await refreshNotes({ silent: true });
      if (lifetimeActiveRef.current && noteSessionRef.current === originSession) {
        const opened = await openNote(action.noteId);
        if (opened && lifetimeActiveRef.current && noteSessionRef.current === originSession + 1) {
          setDailyNotice(statusKey(action));
        }
      }
    } catch (error) {
      if (lifetimeActiveRef.current) setDailyError(serviceErrorFromThrown(error));
    } finally {
      dailyFlight.current = null;
      finish();
      if (lifetimeActiveRef.current) setOtherCreateBusy(false);
    }
  }

  async function performCreate(relativePath: string, source: string, activate: boolean) {
    const originNote = activeNoteIdRef.current;
    const originSession = noteSessionRef.current;
    const pending = creation.snapshot();
    if (!pending.request || !pending.working && !pending.uncertain ||
        pending.request.relativePath === relativePath && pending.request.source === source) creationActivateRef.current = activate;
    const outcome = await creation.create(relativePath, source);
    if (!lifetimeActiveRef.current) return outcome;
    if (!outcome.ok) {
      setCreateError(outcome.error);
      return outcome;
    }
    setCreateError(null);
    setCreationResult({ noteId: outcome.value.noteId, relativePath: outcome.value.relativePath });
    await refreshNotes({ silent: true });
    if (activate && activeNoteIdRef.current === originNote && noteSessionRef.current === originSession) {
      await openNote(outcome.value.noteId);
    }
    return outcome;
  }

  creationRetryRef.current = () => {
    const snapshot = creation.snapshot();
    if (snapshot.working || !snapshot.request) return creation.retry();
    return performCreate(snapshot.request.relativePath, snapshot.request.source, creationActivateRef.current);
  };

  /** F18: keep the exact expanded source and request identity for retries. */
  async function createFromTemplate(title: string, text: string): Promise<void> {
    await performCreate(`${noteNameFor(title)}.md`, text, true);
  }

  /**
   * One step, as in other note apps: an untitled note beside the open one (or
   * at the root) opens with its title selected. A name taken since the list
   * was read moves on to the next number.
   */
  async function quickCreate(inFolder?: string): Promise<void> {
    if (!writable) return;
    const taken = new Set(notesView.kind === 'ready' ? notesView.notes.map((note) => note.relativePath) : []);
    const folder = inFolder ?? (activeDoc === null ? '' : folderOf(activeDoc.relativePath));
    for (let attempt = 0; attempt < 5; attempt += 1) {
      const path = untitledPath(taken, folder, translator('workspace.title.untitled'));
      const outcome = await performCreate(path, '', true);
      if (outcome.ok) {
        setTitleFocus({ noteId: outcome.value.noteId, token: Date.now() });
        return;
      }
      if (outcome.error.code !== 'PATH_COLLISION') return;
      taken.add(path);
    }
  }

  /**
   * Renames from the title in place when nothing else changes; a rename that
   * would rewrite or leave links goes to the rename dialog so it is previewed.
   */
  async function renameFromTitle(noteId: string, toPath: string): Promise<boolean> {
    setTitleError(null);
    if (activeDoc?.noteId !== noteId || !writable || activeDoc.readOnly || composing || createBusy || historyRestoring) return false;
    for (const entry of saveController.notesNeedingAttentionOnExit()) {
      if (await saveActiveBeforeActivation(entry.noteId)) return false;
    }
    const planned = await noteService.previewNoteRename(noteId, toPath);
    if (!planned.ok) {
      setTitleError(planned.error);
      return false;
    }
    const plan = planned.value;
    const committed = await noteService.previewRenameCommit(plan);
    const baseline = plan.expectedHashes.find((item) => item.noteId === noteId);
    if (plan.edits.length > 0 || plan.untouched.length > 0 || !committed.ok || !canCommit(committed.value) || !baseline) {
      setRenameTarget(toPath);
      setRenameOpen(true);
      return false;
    }
    const outcome = await applyRename(plan, baseline.expectedRevision, newOperationId());
    if (!outcome.ok) {
      setTitleError(outcome.error);
      return false;
    }
    return outcome.value.completed;
  }

  /** F22-R03: rows share the ordinary note creation/recovery path. */
  async function createDatabaseRow(relativePath: string) {
    return performCreate(relativePath, '', false);
  }

  /**
   * F03-R02: the copy gets a new note id and file, while the source keeps its
   * own id and bytes. The host rebinds the reserved identity inside the copy.
   */
  async function duplicateActiveNote(): Promise<void> {
    const doc = activeDoc;
    if (
      doc === null ||
      !writable ||
      doc.readOnly ||
      createBusy ||
      composing ||
      historyRestoring ||
      trashFlight.current ||
      backupFlight.current
    ) {
      return;
    }
    setOtherCreateBusy(true);
    setDuplicateOutcome(null);
    const blocked = await saveActiveBeforeActivation(doc.noteId);
    if (!lifetimeActiveRef.current) {
      return;
    }
    if (blocked) {
      setOtherCreateBusy(false);
      return;
    }
    // The copy carries what the editor shows now, including text that was only
    // just saved by the step above.
    const source = saveController.getBufferText(doc.noteId) ?? doc.source;
    const existing = new Set<string>();
    if (notesView.kind === 'ready') {
      for (const note of notesView.notes) {
        existing.add(note.relativePath);
      }
    }
    for (const note of openNotes) {
      existing.add(note.relativePath);
    }
    const suffix = translator('workspace.duplicate.suffix');
    const pending = duplicateRequestRef.current;
    let request: CreateCopyRequest;
    if (pending !== null && pending.sourceNoteId === doc.noteId && pending.source === source) {
      // Same content and source: replaying the same id returns the original
      // copy instead of creating a second one if the first response was lost.
      request = pending;
    } else {
      request = {
        sourceNoteId: doc.noteId,
        relativePath: duplicatePath(doc.relativePath, existing, suffix),
        source,
        operationId: newOperationId(),
      };
    }
    let outcome = await noteService.createCopy(request);
    // The tree listing can lag behind disk. A collision writes nothing, so the
    // next candidate is a fresh request with its own operation id.
    for (
      let attempt = 0;
      !outcome.ok && outcome.error.code === 'PATH_COLLISION' && attempt < 20;
      attempt += 1
    ) {
      existing.add(request.relativePath);
      request = {
        sourceNoteId: doc.noteId,
        relativePath: duplicatePath(doc.relativePath, existing, suffix),
        source,
        operationId: newOperationId(),
      };
      outcome = await noteService.createCopy(request);
    }
    if (!lifetimeActiveRef.current) {
      return;
    }
    duplicateRequestRef.current =
      !outcome.ok && outcome.error.code !== 'PATH_COLLISION' ? request : null;
    setOtherCreateBusy(false);
    if (!outcome.ok) {
      setDuplicateOutcome({ kind: 'failed', error: outcome.error });
      return;
    }
    setDuplicateOutcome({ kind: 'created', path: outcome.value.relativePath });
    applyDocument(outcome.value);
    void refreshNotes({ silent: true });
  }

  function handleDocChanged(noteId: string, text: string): void {
    setRestored(false);
    // Every committed edit is a new document version, not only a history or
    // draft restore. The active editor already has this text, so its sync effect
    // is a no-op; a second pane receives the minimal diff (F04-R01).
    setEditorVersion(version => version + 1);
    setBuffers((prev) => (prev[noteId] === text ? prev : { ...prev, [noteId]: text }));
    saveController.setBufferText(noteId, text);
    workflow.updateSource(noteId);
  }

  // Write helpers own SaveController.setBufferText; this only feeds the
  // edited source back into the mounted editor and checkpoint scheduling.
  function showEditedSource(noteId: string, text: string): void {
    setRestored(false);
    setEditorVersion(version => version + 1);
    setBuffers((prev) => (prev[noteId] === text ? prev : { ...prev, [noteId]: text }));
    workflow.updateSource(noteId);
  }

  /**
   * F04/F47: each pane registers its own apply function, but assist commands
   * target whichever pane is active. A function passed directly to a state
   * setter would be mistaken for an updater, so every registration is wrapped.
   */
  function registerAssist(paneId: string, apply: AssistApplier): void {
    assistAppliersRef.current.set(paneId, apply);
    if (paneId === paneLayoutRef.current.activePaneId) {
      setAssistApplier(() => apply);
    }
  }

  function requestPaneFocus(paneId: string): void {
    paneFocusNonceRef.current += 1;
    setPaneFocusRequest({ paneId, nonce: paneFocusNonceRef.current });
  }

  function handleExplicitSave(): void {
    workflow.explicitSave();
  }

  function handleCompositionChange(noteId: string, composing: boolean): void {
    if (noteId !== activeNoteIdRef.current) {
      return;
    }
    setComposing(composing);
    if (composing) {
      workflow.compositionStarted();
      saveController.compositionStarted(noteId);
    } else {
      workflow.compositionEnded();
      saveController.compositionEnded(noteId);
    }
  }

  async function exportActiveNote(): Promise<void> {
    if (activeNoteId === null || activeDoc === null) {
      return;
    }
    const source = saveController.getBufferText(activeNoteId);
    if (source === null) {
      return;
    }
    const outcome = await noteService.exportDraft({
      noteId: activeNoteId,
      relativePath: activeDoc.relativePath,
      source,
    });
    if (!lifetimeActiveRef.current) {
      return;
    }
    if (outcome.ok && outcome.value.outcome === 'saved') {
      setExportNotice('saved');
      setExportError(null);
    } else if (outcome.ok) {
      setExportNotice('cancelled');
      setExportError(null);
    } else {
      setExportNotice(null);
      setExportError(outcome.error);
    }
  }

  const panelGuards = useMemo(() => new PanelGuardRegistry(), []);
  // Find/replace keys for the editors, read at press time so Settings changes apply at once.
  const bindingsRef = useRef(shortcutBindings);
  bindingsRef.current = shortcutBindings;
  const findKeys = useMemo<FindKeys>(() => ({
    find: chordMatcher(() => effectiveBinding(bindingsRef.current, 'editor.find')),
    replace: chordMatcher(() => effectiveBinding(bindingsRef.current, 'editor.replace')),
  }), []);
  const registerPanelGuard = useCallback(
    (key: string, guard: PanelGuard | null) => panelGuards.set(key, guard),
    [panelGuards],
  );
  const gate = useMemo<WorkspaceGate>(
    () =>
      createWorkspaceGate({
        panels: () => panelGuards.list(),
        saveController,
        noteService,
        pathOf: (noteId) => pathsRef.current.get(noteId),
        bufferOf: (noteId) =>
          lifetimeActiveRef.current ? saveController.getBufferText(noteId) ?? undefined : undefined,
        pendingOperation: () => dailyFlight.current ?? trashFlight.current ?? backupFlight.current,
        creation: { snapshot: () => creation.snapshot(), retry: () => creationRetryRef.current() },
      }),
    [saveController, noteService, creation, panelGuards],
  );

  useEffect(() => {
    gateRef.current = gate;
    return () => {
      gateRef.current = null;
    };
  }, [gate, gateRef]);

  const activeSnapshot = activeNoteId !== null ? snapshots[activeNoteId] ?? null : null;
  // The buttons must advertise the chord that actually fires, not a fixed hint.
  const shortcutHint = (commandId: BindableCommandId): string => {
    const binding = effectiveBinding(shortcutBindings, commandId);
    return binding === null ? '' : chordText(binding);
  };
  const labelOf = (commandId: BindableCommandId): string =>
    translator(bindableCommand(commandId).labelKey);
  const writable = vault.writable && (catalogResult === null || catalogResult.writable);
  creationAllowedRef.current = writable && !composing && !createBusy && !historyRestoring && !trashBusy && !backupBusy;
  const sourceWritableRef = useRef(false);
  sourceWritableRef.current = writable && active && !composing && !historyRestoring;
  catalogAllowedRef.current = active && !composing && !createBusy && !historyRestoring && !trashBusy && !backupBusy;
  const catalogSync = useMemo(() => createCatalogSync({
    service: noteService,
    controller: saveController,
    allowed: () => catalogAllowedRef.current,
    currentSummary: id => catalogOpenNotesRef.current.find(note => note.noteId === id) ?? null,
    metadata: summary => {
      pathsRef.current.set(summary.noteId, summary.relativePath);
      setOpenNotes(previous => previous.map(note => note.noteId === summary.noteId ? { ...note, relativePath: summary.relativePath, readOnly: summary.readOnly } : note));
      setActiveDoc(previous => previous?.noteId === summary.noteId ? { ...previous, relativePath: summary.relativePath, readOnly: summary.readOnly } : previous);
    },
    document: document => {
      setCatalogReadErrors(previous => { if (!(document.noteId in previous)) return previous; const next = { ...previous }; delete next[document.noteId]; return next; });
      workflow.bufferCleared(document.noteId);
      pathsRef.current.set(document.noteId, document.relativePath);
      setOpenNotes(previous => previous.map(note => note.noteId === document.noteId ? document : note));
      setBuffers(previous => ({ ...previous, [document.noteId]: document.source }));
      setActiveDoc(previous => previous?.noteId === document.noteId ? document : previous);
      anchorCacheRef.current.clear();
      setEditorVersion(version => version + 1);
      setEmbedRefreshTick(tick => tick + 1);
    },
    failed: (error, noteId) => setCatalogReadErrors(previous => ({ ...previous, [noteId]: error })),
  }), [noteService, saveController, workflow, catalogRetry]);
  useEffect(() => () => catalogSync.dispose(), [catalogSync]);
  useEffect(() => { if (catalogResult) void catalogSync.apply(catalogResult); }, [catalogResult, catalogSync, snapshots, active, composing, createBusy, historyRestoring, trashBusy, backupBusy]);
  const editorSource = activeDoc !== null ? buffers[activeDoc.noteId] ?? activeDoc.source : '';

  function templateInsertionTarget(): TemplateInsertTarget | null {
    const noteId = activeNoteIdRef.current;
    const paneId = paneLayoutRef.current.activePaneId;
    const pane = paneLayoutRef.current.panes.find(item => item.paneId === paneId);
    const selected = selectionOf(paneSelectionsRef.current, paneId);
    if (!noteId || pane?.noteId !== noteId || !selected) return null;
    const source = saveController.getBufferText(noteId);
    return source === null ? null : { noteId, paneId, source, selection: { from: selected.from, to: selected.to } };
  }


  // F14: parse the live buffer, not the last saved file, so the outline follows
  // typing. A debounce keeps a large note from re-parsing on every keystroke,
  // and cleanup drops older answers so a removed heading cannot be resurrected
  // (F14-R02). Parsing runs in M04 through the host, so the
  // outline and link anchors can never disagree about what a heading is.
  useEffect(() => {
    if (!outlineOpen || activeDoc === null) {
      return;
    }
    return observeOutline(noteService, activeDoc.noteId, editorSource, setOutlineSnapshot);
  }, [outlineOpen, editorSource, activeDoc, noteService, outlineRetry]);

  const outlineOutcome = currentOutline(outlineSnapshot, activeDoc?.noteId ?? null, editorSource);
  const outlineError = outlineOutcome !== null && !outlineOutcome.ok ? outlineOutcome.error : null;
  const outlineHeadings = useMemo(() => outlineOutcome?.ok ? outlineOutcome.value : [], [outlineOutcome]);
  const outlineRows = useMemo(
    () => buildOutline(outlineHeadings, editorSource, collapsedOutline),
    [outlineHeadings, editorSource, collapsedOutline],
  );

  // The editor reports a one-based line; rows compare against a zero-based one.
  const activeOutlineId = useMemo(() => {
    if (caretLine === null) {
      return null;
    }
    const targetLine = caretLine - 1;
    let current: string | null = null;
    for (const row of outlineRows) {
      if (row.line > targetLine) {
        break;
      }
      current = row.id;
    }
    return current;
  }, [outlineRows, caretLine]);

  function toggleOutlineFold(id: string): void {
    setCollapsedOutline((current) => {
      const next = toggleCollapsed(current, id);
      writeDevicePreferences({ collapsedOutlineIds: next });
      return next;
    });
  }

  function jumpToHeading(row: OutlineRow): void {
    if (activeDoc === null || !outlineOutcome?.ok || !outlineRows.includes(row)) {
      return;
    }
    // The reading pane has no selectable source, so a heading jump switches to
    // the live editor and selects the heading line there (F14 §3). The request
    // is never applied to a different note than the one that was open.
    setMode((current) => (current === 'read' ? 'live' : current));
    searchJumpNonceRef.current += 1;
    setSearchJump({
      noteId: activeDoc.noteId,
      offset: row.offset,
      nonce: searchJumpNonceRef.current,
    });
  }

  // F11 inline embeds: resolve the embed tree of the open note when read mode is
  // shown. Keyed `target#anchor` because two embeds of the same section share
  // one expansion; nesting and the sibling-chain rule live in `resolveEmbedTree`.
  // Nothing is fetched while editing, so typing stays local. The refresh tick
  // re-resolves after a source commit, focus, or native source notification.
  const [embedExpansions, setEmbedExpansions] = useState<EmbedTree>({});
  const [embedWatchIds, setEmbedWatchIds] = useState<readonly string[]>([]);
  const [embedWatchError, setEmbedWatchError] = useState<ServiceError | null>(null);
  const [embedWatchRetry, setEmbedWatchRetry] = useState(0);
  const embedWatchSourceRef = useRef<string | null>(null);
  const embedSessionRef = useRef(0);
  const embedNoteIdRef = useRef<string | null>(null);
  useEffect(() => {
    if (mode !== 'read' || activeDoc === null) {
      embedSessionRef.current += 1;
      embedNoteIdRef.current = null;
      embedSourceIdsRef.current = new Set();
      setEmbedWatchIds([]);
      embedWatchSourceRef.current = null;
      setEmbedExpansions({});
      return;
    }
    const session = ++embedSessionRef.current;
    if (embedWatchSourceRef.current !== editorSource || embedNoteIdRef.current !== activeDoc.noteId) {
      setEmbedWatchIds([]);
      embedWatchSourceRef.current = editorSource;
    }
    if (embedNoteIdRef.current !== activeDoc.noteId) {
      // Drop the previous note's tree before the new resolution finishes: keys
      // are `target#anchor`, so a stale `![[b]]` would otherwise briefly show
      // another note's content. Refreshing the same note keeps its tree until
      // the new version arrives instead of flashing placeholders.
      embedNoteIdRef.current = activeDoc.noteId;
      embedSourceIdsRef.current = new Set();
      setEmbedExpansions({});
    }
    // `body`, not `source`: frontmatter stays frontmatter, and an embed written
    // inside it is not part of the reading view.
    const body = splitFrontMatter(editorSource).body;
    void resolveEmbedTree(noteService, activeDoc.noteId, body).then(resolved => {
      if (session === embedSessionRef.current) {
        setEmbedExpansions(resolved);
        embedSourceIdsRef.current = embedTargetIds(resolved);
        // Keep known identities while a source is missing so its return is observed.
        setEmbedWatchIds(previous => [...new Set([...previous, ...embedSourceIdsRef.current])].sort());
      }
    });
  }, [mode, activeDoc, editorSource, noteService, embedRefreshTick]);

  const wantedEmbedSources = new Set(embedWatchIds);
  const currentEmbedSources = linkCandidates.filter(note => wantedEmbedSources.has(note.noteId));
  const embedWatchKey = currentEmbedSources.map(note => note.noteId).sort().join(',');
  const embedWatchPathsKey = JSON.stringify(currentEmbedSources.map(note => [note.noteId, note.relativePath]));
  useEffect(() => {
    setEmbedWatchError(null);
    if (!active || mode !== 'read' || embedWatchKey === '') return;
    return observeSources(noteService, embedWatchKey.split(','), {
      ready: () => { setEmbedWatchError(null); setEmbedRefreshTick(tick => tick + 1); },
      changed: () => setEmbedRefreshTick(tick => tick + 1),
      failed: setEmbedWatchError,
    });
  }, [active, mode, noteService, embedWatchKey, embedWatchPathsKey, embedWatchRetry]);

  function restoreDraft(draft: NoteDraft, disk: NoteDocument): boolean {
    if (!lifetimeActiveRef.current || composing || !writable || disk.readOnly || saveController.notesNeedingAttentionOnExit().length > 0) return false;
    // Keep the draft's original CAS baseline so later external edits still conflict on save.
    applyDocument({ ...disk, localRevision: draft.baseRevision, contentHash: draft.baseContentHash });
    if (!saveController.restoreDraft(draft.noteId, draft.source)) return false;
    workflow.bufferCleared(draft.noteId);
    setBuffers(previous => ({ ...previous, [draft.noteId]: draft.source }));
    setEditorVersion(version => version + 1);
    setRestored(true);
    return true;
  }

  async function restoreHistory(historical: NoteDocument, disk: NoteDocument, revisionId: string): Promise<boolean> {
    if (!lifetimeActiveRef.current || composing || historyRestoring || !writable || disk.readOnly || saveController.notesNeedingAttentionOnExit().length > 0) return false;
    setHistoryRestoring(true);
    saveController.registerDocument(disk);
    workflow.bufferCleared(disk.noteId);
    const saving = saveController.requestHistoryRestore(disk.noteId, historical, revisionId);
    setBuffers(previous => ({ ...previous, [disk.noteId]: saveController.getBufferText(disk.noteId) ?? disk.source }));
    setEditorVersion(version => version + 1);
    setRestored(false);
    await saving;
    if (lifetimeActiveRef.current) setHistoryRestoring(false);
    return true;
  }

  function compareConflict() {
    if (!activeDoc || composing) return;
    const snapshot = saveController.snapshot(activeDoc.noteId);
    const source = saveController.getBufferText(activeDoc.noteId);
    if (!snapshot || source === null || snapshot.inFlight || snapshot.status !== 'conflict') return;
    setConflictLocal({ ...activeDoc, source, localRevision: snapshot.revision, contentHash: snapshot.contentHash });
  }

  function resolveConflict(disk: NoteDocument, local: NoteDocument): boolean {
    if (!lifetimeActiveRef.current || composing || activeNoteIdRef.current !== local.noteId || saveController.getBufferText(local.noteId) !== local.source || saveController.snapshot(local.noteId)?.status !== 'conflict') return false;
    workflow.bufferCleared(local.noteId);
    saveController.adoptExternalContent(local.noteId, disk);
    applyDocument(disk);
    void refreshNotes({ silent: true });
    return true;
  }

  async function beginRename(): Promise<void> {
    if (!activeDoc || !writable || activeDoc.readOnly || composing || createBusy || historyRestoring || trashFlight.current || backupFlight.current) return;
    for (const entry of saveController.notesNeedingAttentionOnExit()) {
      if (await saveActiveBeforeActivation(entry.noteId)) return;
    }
    setRenameTarget(null);
    setRenameOpen(true);
  }

  async function refreshWrittenNotes(noteIds: readonly string[]): Promise<void> {
      await refreshNotes({ silent: true });
      for (const noteId of noteIds) {
        if (!saveController.snapshot(noteId) && !openNotes.some(n => n.noteId === noteId)) continue;
        const current = await noteService.readNote(noteId);
        if (!lifetimeActiveRef.current) break;
        if (current.ok) {
          const local = saveController.snapshot(noteId);
          if (local?.dirty || local?.inFlight) continue;
          workflow.bufferCleared(noteId);
          saveController.adoptExternalContent(noteId, current.value);
          pathsRef.current.set(noteId, current.value.relativePath);
          setOpenNotes(previous => previous.map(n => n.noteId === noteId ? current.value : n));
          setBuffers(previous => ({ ...previous, [noteId]: current.value.source }));
          if (activeNoteIdRef.current === noteId) setActiveDoc(current.value);
        }
      }
      anchorCacheRef.current.clear();
      setEditorVersion(v => v + 1);
      setEmbedRefreshTick(v => v + 1);
  }

  async function prepareTagWrite(): Promise<boolean> {
    if (!writable || composing || createBusy || historyRestoring || trashFlight.current || backupFlight.current) return false;
    for (const entry of saveController.notesNeedingAttentionOnExit()) {
      if (await saveActiveBeforeActivation(entry.noteId)) return false;
    }
    return lifetimeActiveRef.current && !composing && saveController.notesNeedingAttentionOnExit().length === 0;
  }

  const runTagWrite: TagWriteRunner = async (action, noteIds) => {
    if (!await prepareTagWrite()) return { ok: false, error: { code: 'VERSION_CONFLICT', messageKey: 'error.pendingLocalEdits', params: {}, retryable: false, operationId: null, retryAfterSeconds: null } };
    const first = noteIds[0] ?? 'tag-update';
    return runTrash(first, pathsRef.current.get(first) ?? translator('tags.title'), async () => {
      const outcome = await action();
      await refreshWrittenNotes(noteIds);
      return outcome;
    });
  };

  const applyRename: NoteService['rename'] = async (plan: RenamePlan, expectedRevision, operationId) => {
    if (saveController.notesNeedingAttentionOnExit().length > 0) return { ok: false, error: { code: 'VERSION_CONFLICT', messageKey: 'error.pendingLocalEdits', params: {}, retryable: false, operationId: null, retryAfterSeconds: null } };
    return runTrash(plan.noteId, plan.fromPath, async () => {
      const outcome = await noteService.rename(plan, expectedRevision, operationId);
      await refreshWrittenNotes(plan.expectedHashes.map(item => item.noteId));
      return outcome;
    });
  };

  function canMutateTrash(noteId: string, allowLocalEdits = false): boolean {
    const snapshot = saveController.snapshot(noteId);
    return writable && !dailyFlight.current && !trashFlight.current && !backupFlight.current && !composing && (!snapshot || (!snapshot.inFlight && (allowLocalEdits || (snapshot.status === 'clean' && !snapshot.dirty))));
  }

  const runTrash: TrashMutationRunner = async (noteId, path, action, allowLocalEdits = false) => {
    if (!canMutateTrash(noteId, allowLocalEdits)) return { ok: false, error: { code: 'VERSION_CONFLICT', messageKey: 'error.pendingLocalEdits', params: {}, retryable: false, operationId: null, retryAfterSeconds: null } };
    let finish!: () => void;
    const done = new Promise<void>(resolve => { finish = resolve; });
    trashFlight.current = { noteId, path, done }; setTrashBusy(true);
    try { return await action(); }
    catch { return { ok: false, error: { code: 'IO_DENIED', messageKey: 'error.trashUnavailable', params: {}, retryable: true, operationId: null, retryAfterSeconds: null } }; }
    finally { trashFlight.current = null; finish(); if (lifetimeActiveRef.current) setTrashBusy(false); }
  };

  async function reconcileTrash(noteId: string) {
    await refreshNotes({ silent: true });
    const current = await noteService.readNote(noteId);
    if (!lifetimeActiveRef.current) return;
    const snapshot = saveController.snapshot(noteId);
    if (current.ok) {
      pathsRef.current.set(noteId, current.value.relativePath);
      if (activeNoteIdRef.current === noteId) applyDocument(current.value);
      else setOpenNotes(previous => previous.map(note => note.noteId === noteId ? current.value : note));
    } else if (current.error.code === 'OBJECT_NOT_FOUND' && (!snapshot || (snapshot.status === 'clean' && !snapshot.dirty && !snapshot.inFlight))) {
      workflow.bufferCleared(noteId);
      setOpenNotes(previous => previous.filter(note => note.noteId !== noteId));
      if (activeNoteIdRef.current === noteId) { activeNoteIdRef.current = null; setActiveNoteId(null); setActiveDoc(null); setDocState({ kind: 'idle' }); }
    }
  }

  function beginDelete() {
    if (!activeDoc || !canMutateTrash(activeDoc.noteId)) return;
    const snapshot = saveController.snapshot(activeDoc.noteId);
    const source = saveController.getBufferText(activeDoc.noteId);
    if (snapshot && source !== null) setDeleteLocal({ ...activeDoc, source, localRevision: snapshot.revision, contentHash: snapshot.contentHash });
  }

  async function runBackup(action: () => Promise<void>, reattach = false): Promise<boolean> {
    if (backupFlight.current || (!reattach && (dailyFlight.current || trashFlight.current || composing || createBusy || historyRestoring || saveController.notesNeedingAttentionOnExit().length > 0))) return false;
    let finish!: () => void;
    const done = new Promise<void>(resolve => { finish = resolve; });
    backupFlight.current = { noteId: `backup:${vault.vaultId}`, path: translator('backup.title'), done };
    setBackupBusy(true);
    try { await action(); return true; }
    finally { backupFlight.current = null; finish(); if (lifetimeActiveRef.current) setBackupBusy(false); }
  }

  // F04 tabs. Closing the active tab activates its right neighbour, then the
  // left, and the last tab returns to the empty workspace instead of closing
  // the app. Unsaved work is offered save/cancel first (F04-A02), and a failed
  // save keeps the tab instead of discarding text.
  async function closeDocumentTab(noteId: string): Promise<void> {
    if (noteId === activeNoteId && activeNoteId !== null) {
      const blocked = await saveActiveBeforeActivation(activeNoteId);
      if (!lifetimeActiveRef.current) {
        return;
      }
      if (blocked) {
        return;
      }
    }
    const next = closeTab({ tabs: openNotes, activeId: activeNoteId }, noteId);
    setOpenNotes([...next.tabs]);
    if (next.activeId === activeNoteId) {
      return;
    }
    if (next.activeId === null) {
      noteSessionRef.current += 1;
      activeNoteIdRef.current = null;
      setActiveNoteId(null);
      setActiveDoc(null);
      setDocState({ kind: 'idle' });
      return;
    }
    await openNote(next.activeId);
  }

  async function switchDocumentTab(step: number): Promise<void> {
    const next = cycleTab(openNotes, activeNoteId, step);
    if (next === null || next === activeNoteId) {
      return;
    }
    await openNote(next);
  }

  async function openSearchResult(noteId: string, sourceOffset: number): Promise<void> {
    await openNote(noteId);
    if (activeNoteIdRef.current !== noteId || !lifetimeActiveRef.current) {
      return;
    }
    setSearchOpen(false);
    searchJumpNonceRef.current += 1;
    setSearchJump({ noteId, offset: sourceOffset, nonce: searchJumpNonceRef.current });
  }

  function rememberNote(noteId: string): void {
    setRecentNotes((current) => {
      const next = pushRecent(current, noteId, recentNoteLimit());
      writeDevicePreferences({ recentNotes: next });
      return next;
    });
  }

  function rememberCommand(commandId: string): void {
    setRecentCommands((current) => {
      const next = pushRecent(current, commandId, recentCommandLimit());
      writeDevicePreferences({ recentCommands: next });
      return next;
    });
  }

  async function openQuickOpenNote(noteId: string): Promise<void> {
    setQuickOpen(false);
    rememberNote(noteId);
    await openNote(noteId);
  }

  const commandAvailability: CommandAvailability = {
    hasActiveNote: activeDoc !== null,
    writable,
    busy: composing || createBusy || historyRestoring || backupBusy || trashBusy,
    canDelete: activeDoc !== null && canMutateTrash(activeDoc.noteId) && !activeDoc.readOnly,
    canDuplicate: activeDoc !== null && writable && !activeDoc.readOnly,
  };

  // Every command forwards to the existing action; the palette never carries
  // its own persistence path (F13 §1). Destructive commands still open their
  // own confirmation, so Enter can never skip it (F13-R03). The list is rebuilt
  // per render so each closure sees the current document, mode and vault flags.
  const commands: CommandDefinition[] = [
    {
      id: 'workspace.search',
      labelKey: bindableCommand('workspace.search').labelKey,
      aliases: ['search', 'find'],
      enabledReason: () => null,
      run: () => setSearchOpen(true),
    },
    {
      id: 'workspace.newNote',
      labelKey: bindableCommand('workspace.newNote').labelKey,
      aliases: ['new note', 'create'],
      enabledReason: (availability) =>
        availability.writable ? null : 'workspace.vault.readOnly',
      run: () => void quickCreate(),
    },
    {
      id: 'workspace.duplicateNote',
      labelKey: 'workspace.duplicateNote',
      aliases: ['duplicate', 'copy', 'clone'],
      enabledReason: (availability) => {
        if (!availability.hasActiveNote) {
          return 'workspace.select';
        }
        if (!availability.writable) {
          return 'workspace.vault.readOnly';
        }
        if (!availability.canDuplicate) {
          return 'editor.draft.blocked';
        }
        return null;
      },
      run: () => {
        void duplicateActiveNote();
      },
    },
    {
      id: 'workspace.rename',
      labelKey: 'rename.title',
      aliases: ['rename', 'move'],
      enabledReason: availability => !availability.hasActiveNote ? 'workspace.select' :
        !availability.writable ? 'workspace.vault.readOnly' :
        availability.busy || !availability.canDuplicate ? 'editor.draft.blocked' : null,
      run: () => { void beginRename(); },
    },
    {
      id: 'workspace.drafts',
      labelKey: 'editor.draft.open',
      aliases: ['drafts', 'recover'],
      enabledReason: () => null,
      run: () => openDraftRecovery(),
    },
    {
      id: 'workspace.history',
      labelKey: 'history.title',
      aliases: ['history', 'versions'],
      enabledReason: (availability) =>
        availability.hasActiveNote ? null : 'workspace.select',
      run: () => setHistoryOpen(true),
    },
    {
      id: 'workspace.metadata',
      labelKey: 'metadata.title',
      aliases: ['metadata', 'tags', 'backlinks'],
      enabledReason: () => null,
      run: () => setMetadataOpen(true),
    },
    {
      id: 'workspace.trash',
      labelKey: 'trash.title',
      aliases: ['trash', 'deleted'],
      enabledReason: () => null,
      run: () => {
        setBackupOpen(false);
        setTrashOpen(true);
      },
    },
    {
      id: 'workspace.export',
      labelKey: bindableCommand('workspace.export').labelKey,
      aliases: ['export', 'zip', 'archive'],
      enabledReason: () => null,
      run: () => setExportOpen(true),
    },
    {
      id: 'workspace.backup',
      labelKey: 'backup.title',
      aliases: ['backup', 'restore'],
      enabledReason: () => null,
      run: () => {
        setTrashOpen(false);
        setBackupOpen(true);
      },
    },
    {
      id: 'workspace.delete',
      labelKey: 'trash.delete',
      aliases: ['delete', 'trash note'],
      enabledReason: (availability) => {
        if (!availability.hasActiveNote) {
          return 'workspace.select';
        }
        if (!availability.canDelete) {
          return availability.writable ? 'editor.draft.blocked' : 'workspace.vault.readOnly';
        }
        return null;
      },
      // Deletion still opens the delete dialog; nothing is removed here.
      run: () => beginDelete(),
    },
    {
      id: 'editor.mode.live',
      labelKey: 'editor.mode.live',
      aliases: ['live', 'preview'],
      enabledReason: (availability) =>
        availability.hasActiveNote ? null : 'workspace.select',
      run: () => setMode('live'),
    },
    {
      id: 'editor.mode.source',
      labelKey: 'editor.mode.source',
      aliases: ['source'],
      enabledReason: (availability) =>
        availability.hasActiveNote ? null : 'workspace.select',
      run: () => setMode('source'),
    },
    {
      id: 'editor.mode.read',
      labelKey: 'editor.mode.read',
      aliases: ['read', 'reading'],
      enabledReason: (availability) =>
        availability.hasActiveNote ? null : 'workspace.select',
      run: () => setMode('read'),
    },
  ];

  const busyNote = composing || createBusy || historyRestoring;
  const toasts: ToastItem[] = [
    ...(dailyToast && dailyNotice !== null ? [{ key: 'daily', text: translator(dailyNotice), onDismiss: () => setDailyNotice(null) }] : []),
    ...(duplicateOutcome?.kind === 'created' ? [{ key: 'duplicate', text: translator('workspace.duplicate.created', { path: duplicateOutcome.path }), onDismiss: () => setDuplicateOutcome(null) }] : []),
    ...(exportNotice !== null ? [{ key: 'export', text: translator(exportNotice === 'saved' ? 'editor.export.saved' : 'editor.export.cancelled'), onDismiss: () => setExportNotice(null) }] : []),
    ...(restored ? [{ key: 'restored', text: translator('editor.draft.loaded'), onDismiss: () => setRestored(false) }] : []),
    ...(attachmentNotice !== null ? [{ key: 'attachment', text: attachmentNotice, onDismiss: () => setAttachmentNotice(null) }] : []),
    ...(linkMissing !== null ? [{ key: 'link', text: translator('editor.link.unresolved', { target: linkMissing }), onDismiss: () => setLinkMissing(null) }] : []),
  ];
  const dailyOptions = (
    <section className="workspace-action-group daily-options">
      <button type="button" className="secondary" disabled={dailyBusy} onClick={() => { const next = shiftDate(dailyDate, -1); setDailyDate(next); void openDaily(next); }}>‹ {translator('daily.previous')}</button>
      <button type="button" className="secondary" disabled={dailyBusy} onClick={() => { const next = shiftDate(dailyDate, 1); setDailyDate(next); void openDaily(next); }}>{translator('daily.next')} ›</button>
      <label className="daily-pick">
        <span>{translator('daily.chooseDate')}</span>
        <input
          type="date"
          className="daily-picker"
          value={dailyDate}
          disabled={dailyBusy}
          onChange={(event) => {
            const next = chosenDate(event.currentTarget.value);
            if (next === null) {
              return;
            }
            setDailyDate(next);
            void openDaily(next);
          }}
        />
      </label>
      <button type="button" className="secondary" disabled={dailyBusy} aria-haspopup="dialog" onClick={() => setDailySettingsOpen(true)}><Settings2 size={16} aria-hidden="true" /> {translator('daily.settings')}</button>
    </section>
  );
  const sidebarTools: SidebarTool[] = [
    { key: 'search', label: labelOf('workspace.search'), icon: FileSearch, onClick: () => setSearchOpen(true) },
    { key: 'palette', label: labelOf('workspace.commandPalette'), icon: Command, onClick: () => { setQuickOpen(false); setPaletteOpen(true); } },
    { key: 'graph', label: translator('graph.title'), icon: Network, onClick: () => setGraphOpen(true) },
    { key: 'canvas', label: translator('canvas.title'), icon: LayoutDashboard, onClick: () => setCanvasOpen(true) },
    { key: 'database', label: translator('database.title'), icon: Table, onClick: () => setDatabaseOpen(true) },
    { key: 'tasks', label: translator('tasks.title'), icon: ListChecks, onClick: () => setTasksOpen(true) },
    { key: 'templates', label: translator('templates.title'), icon: LayoutTemplate, onClick: () => { setTemplateTarget(templateInsertionTarget()); setTemplatesOpen(true); } },
    ...(writable ? [{ key: 'import', label: labelOf('workspace.import'), icon: Download, onClick: () => setImportOpen(true) }] : []),
    { key: 'drafts', label: translator('editor.draft.open'), icon: FileClock, disabled: backupBusy, onClick: openDraftRecovery },
    { key: 'backup', label: translator('backup.title'), icon: Archive, disabled: composing || trashBusy || createBusy || historyRestoring, onClick: () => { setTrashOpen(false); setBackupOpen(true); } },
    { key: 'diagnostics', label: translator('diagnostics.title'), icon: Stethoscope, onClick: () => setDiagnosticsOpen(true) },
  ];
  const latexActions: BarAction[] = activeDoc === null ? [] : [
    { key: 'insert', label: translator('latexInsert.title'), onClick: () => { setCitationOpen(false); setInsertOpen(true); } },
    { key: 'citation', label: translator('citation.title'), onClick: () => { setInsertOpen(false); setCitationOpen(true); } },
    { key: 'toolbar', label: translator('latex.toolbar'), pressed: latexToolbarOpen, onClick: () => setLatexToolbarOpen((open) => !open) },
    { key: 'projects', label: translator('project.title'), onClick: () => setInspectOpen(true) },
  ];
  const noteActions: BarAction[][] = activeDoc === null ? [] : [
    [
      { key: 'rename', label: translator('rename.title'), disabled: !writable || composing || createBusy || historyRestoring || trashBusy || backupBusy, title: shortcutHint('workspace.rename') || undefined, onClick: () => void beginRename() },
      { key: 'duplicate', label: translator('workspace.duplicateNote'), disabled: !writable || activeDoc.readOnly || createBusy || composing || historyRestoring, title: activeDoc.readOnly ? translator('editor.draft.blocked') : undefined, onClick: () => void duplicateActiveNote() },
      { key: 'delete', label: translator('trash.delete'), disabled: !canMutateTrash(activeDoc.noteId) || activeDoc.readOnly || createBusy || historyRestoring, onClick: beginDelete },
    ],
    [
      { key: 'history', label: translator('history.title'), disabled: busyNote, onClick: () => setHistoryOpen(true) },
      { key: 'export', label: labelOf('workspace.export'), onClick: () => setExportOpen(true) },
      { key: 'attachment', label: translator('attachment.title'), onClick: () => { setAttachmentTarget(templateInsertionTarget()); setAttachmentOpen(true); } },
    ],
    [
      { key: 'outline', label: translator('outline.title'), pressed: outlineOpen, disabled: busyNote, onClick: () => setOutlineOpen((open) => !open) },
      { key: 'tags', label: translator('tags.title'), pressed: tagsOpen, disabled: busyNote, onClick: () => setTagsOpen((open) => !open) },
      { key: 'properties', label: translator('properties.title'), pressed: propertiesOpen, disabled: busyNote, onClick: () => setPropertiesOpen((open) => !open) },
      { key: 'metadata', label: translator('metadata.title'), disabled: busyNote || metadataOpen, onClick: () => setMetadataOpen(true) },
    ],
    [
      { key: 'source', label: translator('editor.mode.source'), pressed: mode === 'source', disabled: busyNote, onClick: () => setMode(mode === 'source' ? 'live' : 'source') },
      // With one note open there is no tab strip, so closing lives here too.
      { key: 'close', label: translator('noteBar.close'), disabled: createBusy || composing || backupBusy, onClick: () => void closeDocumentTab(activeDoc.noteId) },
    ],
  ];

  function setSidebarCollapse(collapsed: boolean): void {
    setSidebarCollapsed(collapsed);
    writeDevicePreferences({ sidebarCollapsed: collapsed });
  }

  /** Opens and scrolls to a folder in the tree, from a crumb above the note. */
  function revealFolder(path: string): void {
    setSidebarCollapse(false);
    requestAnimationFrame(() => {
      const summary = document.querySelector<HTMLElement>(`.file-tree summary[data-path="${CSS.escape(path)}"]`);
      if (summary === null) return;
      for (let node = summary.parentElement; node !== null; node = node.parentElement) {
        if (node instanceof HTMLDetailsElement) node.open = true;
      }
      summary.scrollIntoView({ block: 'nearest' });
      summary.focus();
    });
  }

  useEffect(() => {
    if (treeSelection.length === 0) return;
    const onKey = (event: KeyboardEvent) => {
      const target = event.target;
      if (target instanceof HTMLElement && (target.isContentEditable || target.closest('input, textarea, select, .cm-editor') !== null)) return;
      if (event.key === 'Escape') setTreeSelection([]);
      else if (event.key === 'Delete' && writable) { event.preventDefault(); void beginBatchDelete(treeSelection); }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  function runCommand(command: CommandDefinition): void {
    setPaletteOpen(false);
    rememberCommand(command.id);
    command.run();
  }

  return (
    <div className="workspace" data-assist={insertOpen || citationOpen ? 'true' : 'false'}>
      <ToastStack closeLabel={translator('toast.dismiss')} items={toasts} />
      <header className="workspace-head">
        <h1 className="visually-hidden">{translator('workspace.status.vault', { name: vault.displayName })}</h1>
        {backupBusy && !backupOpen ? <p role="status">{translator('trash.working')}</p> : null}
        {dailyError !== null ? <p role="alert" className="notice-panel">{translator('daily.openFailed')}</p> : null}
        {dailyNotice !== null && !dailyToast ? <p role="status" className="notice-panel">{translator(dailyNotice)}</p> : null}
        {dailyChoices.length > 0 ? (
          <div className="notice-panel" role="status">
            <strong>{translator('daily.ambiguous')}</strong>
            <ul className="daily-choices">
              {dailyChoices.map((candidate) => (
                <li key={candidate.noteId}>
                  <button type="button" className="secondary" onClick={() => { setDailyChoices([]); void openNote(candidate.noteId); }}>
                    {candidate.relativePath}
                  </button>
                </li>
              ))}
            </ul>
            <button type="button" className="secondary" onClick={() => setDailyChoices([])}>
              {translator('daily.dismiss')}
            </button>
          </div>
        ) : null}
        <CatalogNotice result={catalogResult} error={catalogError} watchError={catalogWatchError} readFailures={Object.entries(catalogReadErrors).map(([noteId, error]) => ({ path: pathsRef.current.get(noteId) ?? noteId, error }))} translator={translator} locale={locale} onRetry={() => { setCatalogReadErrors({}); setCatalogRetry(value => value + 1); }} />
        {!writable ? <p className="notice">{translator('workspace.vault.readOnly')}</p> : null}
        {shortcutPrefix !== null ? (
          <p className="notice" role="status">
            {translator('shortcut.prefixArmed', { chord: shortcutPrefix.text })}
          </p>
        ) : null}
        {vault.findings.some(finding => finding.code === 'SchemaUpgradeRequired') ? <p className="notice" role="status">{translator('workspace.vault.upgradeRequired')}</p> : null}
        {vault.findings.some(finding => finding.code === 'SchemaUpgradeInterrupted') ? <p className="notice" role="status">{translator('workspace.vault.upgradeInterrupted')}</p> : null}
        {vault.findings.some(finding => finding.code === 'SchemaNewerThanSupported') ? <p className="notice" role="status">{translator('workspace.vault.newerSchema')}</p> : null}
        {vault.findings.some(finding => finding.code === 'LegacyCatalogUnavailable') ? <p className="notice" role="status">{translator('workspace.vault.catalogUnavailable')}</p> : null}
        {recovery !== null && recovery.pending.length > 0 ? (
          <div className="notice-panel" role="status">
            <strong>{translator('recovery.pendingTitle')}</strong>
            <p>{translator('recovery.pendingBody')}</p>
            <div className="panel-actions">
              <button type="button" className="secondary" disabled={recoveryBusy} onClick={() => void retryRecovery()}>
                {translator('recovery.retry')}
              </button>
            </div>
          </div>
        ) : null}
        {draftAvailable ? (
          <div className="notice-panel" role="status">
            <strong>{translator('editor.draft.availableTitle')}</strong>
            <p>{translator('editor.draft.body')}</p>
            <div className="panel-actions">
              <button type="button" className="secondary" onClick={openDraftRecovery}>{translator('editor.draft.open')}</button>
              <button type="button" className="secondary" onClick={() => setDraftAvailable(false)}>{translator('editor.draft.later')}</button>
            </div>
          </div>
        ) : null}
        {recoveryError !== null ? (
          <p role="alert">{serviceErrorMessage(locale, recoveryError.code)}</p>
        ) : null}
        {duplicateOutcome?.kind === 'failed' ? (
          <div className="notice-panel" role="alert">
            <p>{serviceErrorMessage(locale, duplicateOutcome.error.code)}</p>
            <button type="button" className="secondary" onClick={() => setDuplicateOutcome(null)}>
              {translator('workspace.duplicate.dismiss')}
            </button>
          </div>
        ) : null}
      </header>
      {writable && dailySettingsOpen ? (
        <DailySettingsPanel
          service={noteService}
          initial={dailySettingsFor(readDevicePreferences(), vault.vaultId)}
          translator={translator}
          locale={locale}
          onClose={() => setDailySettingsOpen(false)}
          onSave={(settings) => {
            writeDevicePreferences({
              dailySettings: withDailySettings(
                readDevicePreferences(),
                vault.vaultId,
                settings,
              ),
            });
            setDailySettingsOpen(false);
            // After a missing-template prompt, retry with the newly chosen
            // template (or the blank default) instead of making the user find
            // the same day again.
            if (dailyTemplatePrompt !== null) {
              void openDaily(dailyTemplatePrompt.date);
            }
          }}
        />
      ) : null}
      {writable && dailyTemplatePrompt !== null ? (
        <div className="notice-panel" role="status">
          <strong>{translator('daily.templateMissingPrompt')}</strong>
          <div className="daily-template-actions">
            <button
              type="button"
              className="secondary"
              autoFocus
              onClick={() => void openDaily(dailyTemplatePrompt.date, null)}
            >
              {translator('daily.createBlank')}
            </button>
            <button
              type="button"
              className="secondary"
              onClick={() => setDailySettingsOpen(true)}
            >
              {translator('daily.chooseTemplate')}
            </button>
            <button
              type="button"
              className="secondary"
              onClick={() => setDailyTemplatePrompt(null)}
            >
              {translator('daily.dismiss')}
            </button>
          </div>
        </div>
      ) : null}
      {trashOpen ? <TrashPanel service={noteService} t={translator} locale={locale} regional={regional} initialObjectId={trashInitial} run={runTrash} canMutate={canMutateTrash} onChanged={noteId => void reconcileTrash(noteId)} onClose={() => { setTrashOpen(false); setTrashInitial(null); }} /> : null}
      <div hidden={!backupOpen}><BackupPanel service={backupService} t={translator} locale={locale} regional={regional} visible={backupOpen} canSchedule={writable} canOverwrite={vault.schemaVersion === 3 && writable} canUpgrade={[1, 2].includes(vault.schemaVersion) || vault.findings.some(finding => finding.code === 'SchemaUpgradeInterrupted')} canStart={!composing && !trashBusy && !createBusy && !historyRestoring && saveController.notesNeedingAttentionOnExit().length === 0} run={runBackup} onClose={() => setBackupOpen(false)} onUpgraded={onUpgraded} onOverwritten={onOverwritten} /></div>
      <div
        className="workspace-grid"
        data-outline={outlineOpen ? 'true' : 'false'}
        data-sidebar={sidebarCollapsed ? 'collapsed' : 'open'}
        style={{ ...(trashOpen || backupOpen || exportOpen || importOpen ? { display: 'none' } : {}), '--sidebar-col': `${sidebarWidth}px` } as CSSProperties}
      >
        {sidebarCollapsed ? <SidebarRail translator={translator} onExpand={() => setSidebarCollapse(false)} /> : <WorkspaceSidebar
          translator={translator}
          vaultName={vault.displayName}
          writable={writable}
          searchHint={shortcutHint('workspace.quickOpen')}
          newNoteHint={shortcutHint('workspace.newNote')}
          busy={createBusy || composing || backupBusy}
          newNoteDisabled={createBusy || creationSnapshot.uncertain}
          todayDisabled={dailyBusy}
          trashDisabled={composing || trashBusy || backupBusy || createBusy || historyRestoring}
          favorites={notesView.kind === 'ready' ? resolveFavorites(favorites, notesView.notes).map(entry => ({
            noteId: entry.noteId, label: recentLabel(entry), title: entry.note?.relativePath ?? '', available: entry.available,
          })) : []}
          dailyOptions={dailyOptions}
          tools={sidebarTools}
          onSwitchVault={onNavigate ? () => onNavigate('home') : undefined}
          onSettings={onNavigate ? () => onNavigate('settings') : undefined}
          onCollapse={() => setSidebarCollapse(true)}
          onSearch={() => { setPaletteOpen(false); setQuickOpen(true); }}
          onToday={() => void openDaily(localCalendarDate(new Date()))}
          onNewNote={() => void quickCreate()}
          onNewFolder={() => beginFolder(activeDoc === null ? '' : folderOf(activeDoc.relativePath))}
          onOpenFavorite={noteId => void openNote(noteId)}
          onRemoveFavorite={noteId => {
            setFavorites((current) => {
              const next = toggleFavorite(current, noteId, favoriteNoteLimit());
              writeDevicePreferences({ favoriteNotes: next });
              return next;
            });
          }}
          onLatex={() => setInspectOpen(true)}
          latexProjects={sidebarProjects}
          onNewLatexProject={() => { setWizardParent(''); setWizardOpen(true); }}
          onOpenProject={setLatexProjectId}
          onTrash={() => { setBackupOpen(false); setTrashOpen(true); }}
          onResize={(deltaX, done) => {
            const from = sidebarDragFrom.current ?? sidebarWidth;
            sidebarDragFrom.current = done ? null : from;
            const next = clampSidebarWidth(from + deltaX);
            setSidebarWidth(next);
            if (done) writeDevicePreferences({ sidebarWidth: next });
          }}
        >
          {writable && creationSnapshot.request !== null && !creationSnapshot.working && (createError !== null || creationSnapshot.uncertain) ? (
            <div className="create-notice" role="alert">
              <p>
                {creationSnapshot.uncertain || createError?.messageKey === 'workspace.create.pending'
                  ? translator('workspace.create.pending')
                  : serviceErrorMessage(locale, createError?.code ?? 'INVALID_INPUT')}
              </p>
              <div className="form-actions">
                <button type="button" className="primary" disabled={createBusy || backupBusy} onClick={() => void creationRetryRef.current()}>
                  {translator('workspace.create.retry')}
                </button>
                {!creationSnapshot.uncertain ? (
                  <button type="button" className="secondary" onClick={() => { creation.dismissRejected(); }}>
                    {translator('workspace.create.dismiss')}
                  </button>
                ) : null}
              </div>
            </div>
          ) : null}
          {notesView.kind === 'failed' ? (
            <div className="error-panel" role="alert">
              <p>
                <strong>{translator('workspace.error.notes.title')}</strong>
              </p>
              <p>{serviceErrorMessage(locale, notesView.error.code)}</p>
              <button type="button" className="secondary" onClick={() => void refreshNotes()}>
                {translator('workspace.error.retry')}
              </button>
            </div>
          ) : null}
          {notesView.kind === 'ready' && notesView.notes.length === 0 ? (
            <p className="notes-empty">{translator('workspace.notes.empty')}</p>
          ) : null}
          {notesView.kind === 'loading' ? <p role="status">{translator('workspace.loading')}</p> : null}
          {notesView.kind === 'ready' ? <NoteTree
            notes={notesView.notes}
            folders={folders}
            activeId={activeNoteId}
            disabled={createBusy || composing || backupBusy}
            naming={writable ? namingFolder : null}
            namingLabel={translator('workspace.folder.name')}
            namingError={namingError}
            onOpen={noteId => { setTreeSelection([]); void openNote(noteId); }}
            selected={selectedKeys}
            onSelect={selectInTree}
            onName={(name) => void nameFolder(name)}
            onDrop={dropOnFolder}
            onCancelName={() => { setNamingFolder(null); setNamingError(null); }}
            onMenu={(target, x, y) => {
              // Right-clicking outside the selection acts on that row alone.
              if (!selectedKeys.has(treeKey(target))) setTreeSelection([]);
              setTreeMenu({ target, x, y });
            }}
            projects={latexProjects}
            onOpenProject={setLatexProjectId}
            rowActions={{
              newNote: translator('workspace.tree.newNote'),
              more: translator('workspace.tree.more'),
              newNoteDisabled: !writable || createBusy || creationSnapshot.uncertain,
              onNewNote: folder => void quickCreate(folder),
            }}
          /> : null}
          {folderOp !== null ? <FolderActionDialog
            title={folderOp.title}
            summary={folderOp.summary}
            confirmLabel={folderOp.confirmLabel}
            danger={folderOp.danger}
            phase={folderOp.phase}
            translator={translator}
            onConfirm={() => void runFolderOp(0)}
            onResume={() => { if (folderOp.phase.kind === 'stopped') void runFolderOp(folderOp.phase.done); }}
            onClose={() => setFolderOp(null)}
          /> : null}
          {treeMenu !== null ? <TreeMenu x={treeMenu.x} y={treeMenu.y} label={translator('workspace.tree.menu')} items={treeMenuItems(treeMenu.target)} onClose={() => setTreeMenu(null)} /> : null}
        </WorkspaceSidebar>}
        <section className="editor-pane">
          {openNotes.length > 1 ? <nav className="document-tabs" aria-label={translator('workspace.notes.heading')}>
            {openNotes.map(note => {
              const snapshot = snapshots[note.noteId];
              const pending = snapshot !== undefined && snapshot.status !== 'clean';
              return <span className="document-tab" key={note.noteId}>
                <button type="button" className="document-tab-open" aria-current={activeNoteId === note.noteId ? 'page' : undefined} disabled={createBusy || composing || backupBusy} onClick={() => void openNote(note.noteId)}>
                  <FileText size={15} aria-hidden="true" />
                  <span>{note.relativePath.split('/').pop()?.replace(/\.md$/i, '')}</span>
                  {pending ? <span className="document-tab-pending" aria-label={translator('tabs.pending')} title={translator('tabs.pending')}>•</span> : null}
                </button>
                <button
                  type="button"
                  className="document-tab-close"
                  aria-label={translator('tabs.close', { name: note.relativePath.split('/').pop()?.replace(/\.md$/i, '') ?? note.relativePath })}
                  title={translator('tabs.close', { name: note.relativePath.split('/').pop()?.replace(/\.md$/i, '') ?? note.relativePath })}
                  disabled={createBusy || composing || backupBusy}
                  onClick={() => void closeDocumentTab(note.noteId)}
                >
                  <X size={14} aria-hidden="true" />
                </button>
              </span>;
            })}
          </nav> : null}
          {activeDoc !== null ? (
            <NoteBar
              translator={translator}
              vaultName={vault.displayName}
              relativePath={activeDoc.relativePath}
              status={activeSnapshot !== null ? (
                // Silent once saved (still announced); words only while something is pending.
                <span className={activeSnapshot.status === 'clean' ? 'save-status visually-hidden' : 'save-status'} role="status" data-state={activeSnapshot.status}>
                  {translator(setStatusKeys[activeSnapshot.status])}
                </span>
              ) : null}
              favorite={isFavorite(favorites, activeDoc.noteId)}
              mode={mode}
              busy={composing || createBusy || historyRestoring}
              latexActions={latexActions}
              noteActions={noteActions}
              noteExtras={<SplitControls translator={translator} noteId={activeDoc.noteId} onLayoutChange={setPaneLayout} focusRequest={paneFocusRequest} />}
              onCrumb={revealFolder}
              onToggleFavorite={() => {
                setFavorites((current) => {
                  const next = toggleFavorite(current, activeDoc.noteId, favoriteNoteLimit());
                  writeDevicePreferences({ favoriteNotes: next });
                  return next;
                });
              }}
              onMode={setMode}
            />
          ) : null}
          {docState.kind === 'loading' ? <p role="status">{translator('workspace.loading')}</p> : null}
          {activeDoc === null && docState.kind !== 'loading' ? <StartPanel
            translator={translator}
            vaultName={vault.displayName}
            writable={writable}
            newNoteHint={shortcutHint('workspace.newNote')}
            newNoteDisabled={createBusy || creationSnapshot.uncertain}
            todayDisabled={dailyBusy}
            recent={notesView.kind === 'ready' ? resolveFavorites(recentNotes, notesView.notes).slice(0, START_RECENT_LIMIT).map(entry => ({
              noteId: entry.noteId, label: recentLabel(entry), title: entry.note?.relativePath ?? '', available: entry.available,
            })) : []}
            onNewNote={() => void quickCreate()}
            onNewLatexProject={() => { setWizardParent(''); setWizardOpen(true); }}
            onImportLatex={() => setInspectOpen(true)}
            onToday={() => void openDaily(localCalendarDate(new Date()))}
            onOpenRecent={noteId => void openNote(noteId)}
          /> : null}
          {docState.kind === 'failed' ? (
            <p role="alert">{serviceErrorMessage(locale, docState.error.code)}</p>
          ) : null}
          {checkpointNotice !== null ? (
            <div className="notice-panel" role="status">
              <strong>{translator('editor.checkpoint.failed.title')}</strong>
              <p>{translator('editor.checkpoint.failed.body')}</p>
            </div>
          ) : null}
          {activeSnapshot?.status === 'failed' && activeSnapshot.error !== null ? (
            <div className="error-panel" role="alert">
              {saveFailureMessages(locale, activeSnapshot.error.code).map((message, index) => <p key={index}>{message}</p>)}
              <div className="panel-actions">
                <button
                  type="button"
                  className="secondary"
                  onClick={() => {
                    if (activeNoteId !== null) {
                      void saveController.retryFailed(activeNoteId);
                    }
                  }}
                >
                  {translator('editor.save.failed.retry')}
                </button>
                <button
                  type="button"
                  className="secondary"
                  onClick={() => void exportActiveNote()}
                >
                  {translator('editor.save.failed.export')}
                </button>
              </div>
            </div>
          ) : null}
          {activeSnapshot?.status === 'conflict' && activeSnapshot.error !== null ? (
            <div className="error-panel" role="alert">
              <p>
                <strong>{translator('editor.conflict.title')}</strong>
              </p>
              <p>{serviceErrorMessage(locale, activeSnapshot.error.code)}</p>
              <div className="panel-actions">
                <button type="button" className="primary" disabled={composing || createBusy || historyRestoring} onClick={compareConflict}>{translator('editor.conflict.compare')}</button>
                <button
                  type="button"
                  className="secondary"
                  onClick={() => void exportActiveNote()}
                >
                  {translator('editor.save.failed.export')}
                </button>
              </div>
            </div>
          ) : null}
          {exportError !== null ? (
            <p role="alert">{serviceErrorMessage(locale, exportError.code)}</p>
          ) : null}
          {activeDoc !== null ? (
            <div
              className="editor-split"
              data-orientation={paneLayout.orientation}
              data-narrow={isNarrow(viewportWidth) ? 'true' : undefined}
              data-panes={paneLayout.panes.length}
            >
              {paneLayout.panes.map((pane, index) => {
                // F04-R01: both panes are views of the one active note. The
                // toolbar, outline, metadata, and save controller stay aligned
                // with what either pane displays.
                const doc = activeDoc;
                if (doc === null) {
                  return null;
                }
                const source = buffers[doc.noteId] ?? doc.source;
                const active = pane.paneId === paneLayout.activePaneId;
                return (
                  <div
                    className="editor-view"
                    key={pane.paneId}
                    data-pane-id={pane.paneId}
                    data-active={active || undefined}
                  >
                    {paneLayout.panes.length > 1 ? (
                      <div className="editor-view-head">
                        <span className="editor-view-path">{doc.relativePath}</span>
                        <button
                          type="button"
                          className="editor-view-focus"
                          aria-pressed={active}
                          title={translator('split.focusOther')}
                          onClick={() => requestPaneFocus(pane.paneId)}
                        >
                          {translator('split.pane', { index: index + 1 })}
                        </button>
                      </div>
                    ) : null}
                    {mode === 'read' && embedWatchError ? <p className="workspace-warning" role="status">
                      {translator('embed.watchUnavailable')}
                      <button type="button" className="secondary" onClick={() => setEmbedWatchRetry(value => value + 1)}>{translator('embed.watchRetry')}</button>
                    </p> : null}
                    {/* Both panes show one note, so its title is edited in the first. */}
                    {index === 0 ? (
                      <div className="note-title-column">
                        <NoteTitle
                          relativePath={doc.relativePath}
                          editable={writable && !doc.readOnly && !composing && !historyRestoring}
                          focusToken={titleFocus?.noteId === doc.noteId ? titleFocus.token : null}
                          translator={translator}
                          onRename={(toPath) => renameFromTitle(doc.noteId, toPath)}
                        />
                        {titleError !== null ? <p className="workspace-warning" role="alert">{serviceErrorMessage(locale, titleError.code)}</p> : null}
                      </div>
                    ) : null}
                    <NoteEditor
                      findKeys={findKeys}
                      noteId={doc.noteId}
                      pathName={doc.relativePath}
                      source={source}
                      docVersion={editorVersion}
                      readOnly={
                        !writable ||
                        doc.readOnly ||
                        createBusy ||
                        historyRestoring ||
                        trashBusy ||
                        renameOpen ||
                        tagsOpen ||
                        backupBusy ||
                        !active
                      }
                      autoFocus={paneLayout.panes.length > 1 && active}
                      mode={mode}
                      locale={locale}
                      translator={translator}
                      onDocChanged={handleDocChanged}
                      onExplicitSave={handleExplicitSave}
                      onCompositionChange={handleCompositionChange}
                      attachments={attachmentHost}
                      onOpenPdf={active ? (target, page) => void openPdfLink(doc.relativePath, target, page) : undefined}
                      linkCandidates={linkCandidates}
                      anchorsOf={anchorsOf}
                      embedExpansions={embedExpansions}
                      onOpenEmbed={active ? noteId => void openNote(noteId) : undefined}
                      onFollowLink={active ? target => {
                        const found = resolveLinkTarget(linkCandidates, target);
                        if (found) void openNote(found.noteId); else setLinkMissing(target);
                      } : undefined}
                      onCaretLine={active ? setCaretLine : undefined}
                      onSelectionChange={next => {
                        setPaneSelections(current =>
                          applySelection(current, {
                            paneId: pane.paneId,
                            from: next.from,
                            to: next.to,
                          }),
                        );
                        if (active) {
                          setSelection(next);
                        }
                      }}
                      onReady={apply => registerAssist(pane.paneId, apply)}
                      headingAnchors={
                        active || doc.noteId === activeDoc.noteId
                          ? outlineHeadings.map(heading => ({
                              line: heading.line,
                              text: heading.text,
                            }))
                          : undefined
                      }
                      onDeferredMode={
                        active
                          ? pending => {
                              // The editor already applied the switch; keep the
                              // parent state in step so the next render does not
                              // revert it. Setting the same value is a no-op.
                              if (pending !== null) {
                                setMode(current => (current === pending ? current : pending));
                              }
                            }
                          : undefined
                      }
                      revealOffset={
                        active && searchJump !== null && searchJump.noteId === doc.noteId
                          ? searchJump.offset
                          : null
                      }
                      revealNonce={active ? searchJump?.nonce : undefined}
                    />
                    {active ? (
                      <BacklinksFooter
                        noteId={doc.noteId}
                        service={noteService}
                        translator={translator}
                        refreshKey={catalogResult?.fingerprint ?? ''}
                        onOpenNote={noteId => void openNote(noteId)}
                      />
                    ) : null}
                    <footer className="editor-footer">
                      <span>{doc.relativePath}</span>
                      <span>
                        {translator('workspace.count', {
                          count: formatNumber(Array.from(splitFrontMatter(source).body).length, regional.region),
                        })}{' '}
                        / UTF-8
                      </span>
                    </footer>
                  </div>
                );
              })}
            </div>
          ) : null}
          {outlineOpen && outlineError !== null ? (
            <div role="alert" className="outline-error">
              <p>{serviceErrorMessage(locale, outlineError.code)}</p>
              <button type="button" className="secondary" onClick={() => { setOutlineSnapshot(null); setOutlineRetry(value => value + 1); }}>{translator('workspace.error.retry')}</button>
            </div>
          ) : null}
        </section>
        {outlineOpen ? (
          <OutlinePanel
            rows={outlineRows}
            pending={activeDoc !== null && outlineOutcome === null}
            failed={outlineError !== null}
            activeId={activeOutlineId}
            collapsedIds={collapsedOutline}
            translator={translator}
            onToggle={toggleOutlineFold}
            onJump={jumpToHeading}
            onClose={() => setOutlineOpen(false)}
          />
        ) : null}
      </div>
      {tagsOpen ? (
        <TagPanel
          service={noteService}
          translator={translator}
          locale={locale}
          canWrite={writable && !composing && !createBusy && !historyRestoring && !trashBusy && !backupBusy}
          prepareWrite={prepareTagWrite}
          runWrite={runTagWrite}
          knownNoteIds={notesView.kind === 'ready' ? notesView.notes.map((note) => note.noteId) : []}
          onOpenNote={(noteId, offset) => {
            setTagsOpen(false);
            void (offset === null ? openNote(noteId) : openSearchResult(noteId, offset));
          }}
          onClose={() => setTagsOpen(false)}
        />
      ) : null}
      {citationOpen && activeDoc !== null ? (
        <CitationPanel
          service={noteService}
          translator={translator}
          locale={locale}
          source={editorSource}
          apply={(fragment) =>
            assistApplier?.([
              { start: selection?.from ?? editorSource.length, end: selection?.to ?? editorSource.length, text: fragment },
            ]) ?? false
          }
          onClose={() => setCitationOpen(false)}
        />
      ) : null}
      {insertOpen && activeDoc !== null ? (
        <InsertPanel
          service={noteService}
          translator={translator}
          locale={locale}
          source={editorSource}
          selection={selection}
          apply={(edits) => assistApplier?.(edits) ?? false}
          onClose={() => setInsertOpen(false)}
        />
      ) : null}
      {latexToolbarOpen && activeDoc !== null ? (
        <LatexToolbar
          service={noteService}
          translator={translator}
          locale={locale}
          source={editorSource}
          selection={selection}
          composing={composing}
          apply={(edits) => assistApplier?.(edits) ?? false}
          onReplaceSource={(next) => handleDocChanged(activeDoc.noteId, next)}
          onClose={() => setLatexToolbarOpen(false)}
        />
      ) : null}
      {attachmentOpen ? (
        <AttachmentPanel
          service={noteService}
          translator={translator}
          locale={locale}
          canInsert={writable && mode !== 'read' && !composing && !historyRestoring && activeDoc?.readOnly === false && sameTemplateTarget(attachmentTarget, templateInsertionTarget())}
          notePath={activeDoc?.relativePath ?? ''}
          onOpenPdf={(path) => { setAttachmentOpen(false); setPdfReader({ path, page: null, target: attachmentTarget ?? templateInsertionTarget() }); }}
          onInsert={(text) => {
            // Same guarded path as template insertion: the captured pane/note
            // must still be current, editable and not composing.
            const applied = applyTemplateInsert(attachmentTarget, templateInsertionTarget(),
              assistAppliersRef.current.get(attachmentTarget?.paneId ?? ''), text, text.length,
              lifetimeActiveRef.current && sourceWritableRef.current && mode !== 'read' &&
                activeDoc?.readOnly === false && !trashFlight.current && !backupFlight.current &&
                !saveController.isComposing(attachmentTarget?.noteId ?? ''));
            if (!applied || !attachmentTarget) return false;
            searchJumpNonceRef.current += 1;
            setSearchJump({ noteId: attachmentTarget.noteId, offset: attachmentTarget.selection.from + text.length, nonce: searchJumpNonceRef.current });
            setAttachmentTarget(null);
            return true;
          }}
          onClose={() => setAttachmentOpen(false)}
        />
      ) : null}
      {pdfReader !== null ? (
        <PdfReaderPanel
          key={`${pdfReader.path}#${pdfReader.page ?? ''}`}
          service={noteService}
          translator={translator}
          locale={locale}
          path={pdfReader.path}
          page={pdfReader.page}
          citeInto={writable && mode !== 'read' && !composing && activeDoc?.readOnly === false && pdfReader.target !== null && sameTemplateTarget(pdfReader.target, templateInsertionTarget()) ? activeDoc.relativePath : null}
          onCite={(text) => {
            const target = pdfReader.target;
            const applied = applyTemplateInsert(target, templateInsertionTarget(),
              assistAppliersRef.current.get(target?.paneId ?? ''), text, text.length,
              lifetimeActiveRef.current && sourceWritableRef.current && mode !== 'read' &&
                activeDoc?.readOnly === false && !trashFlight.current && !backupFlight.current &&
                !saveController.isComposing(target?.noteId ?? ''));
            // The insertion moved the caret, so the next citation goes after this one.
            if (applied) setPdfReader((current) => current === null ? null : { ...current, target: templateInsertionTarget() });
            return applied;
          }}
          onClose={() => setPdfReader(null)}
        />
      ) : null}
      {renameOpen && activeDoc !== null ? (
        <RenameDialog
          key={activeDoc.noteId}
          service={noteService}
          translator={translator}
          locale={locale}
          noteId={activeDoc.noteId}
          currentPath={activeDoc.relativePath}
          initialDestination={renameTarget ?? undefined}
          onApply={applyRename}
          onClose={() => setRenameOpen(false)}
        />
      ) : null}
      {inspectOpen ? (
        <ProjectInspectPanel
          latexService={latexService}
          environment={latexEnvironment}
          vaultId={vault.vaultId}
          writable={vault.writable}
          translator={translator}
          locale={locale}
          onOpenProject={(projectId) => { setInspectOpen(false); setLatexProjectId(projectId); }}
          onOpenEnvironment={() => { setInspectOpen(false); setLatexEnvOpen(true); }}
          onCreateProject={() => { setInspectOpen(false); setWizardParent(''); setWizardOpen(true); }}
          onClose={() => setInspectOpen(false)}
        />
      ) : null}
      {latexProjectId !== null ? (
        <LatexWorkspace
          key={`:${latexProjectId}`}
          projects={latexService}
          builds={latexBuilds}
          vaultId={vault.vaultId}
          projectId={latexProjectId}
          translator={translator}
          locale={locale}
          findKeys={findKeys}
          onOpenEnvironment={() => setLatexEnvOpen(true)}
          onClose={() => setLatexProjectId(null)}
        />
      ) : null}
      {latexEnvOpen ? (
        <LatexEnvironmentPanel
          service={latexEnvironment}
          translator={translator}
          locale={locale}
          onClose={() => setLatexEnvOpen(false)}
        />
      ) : null}
      {wizardOpen ? (
        <ProjectWizardPanel
          parentFolder={wizardParent}
          service={noteService}
          latexService={latexService}
          vaultId={vault.vaultId}
          writable={vault.writable}
          translator={translator}
          locale={locale}
          onOpenProject={(projectId) => { setWizardOpen(false); setLatexProjectId(projectId); }}
          onClose={() => setWizardOpen(false)}
        />
      ) : null}
      {diagnosticsOpen ? (
        <DiagnosticsPanel
          service={noteService}
          translator={translator}
          locale={locale}
          onClose={() => setDiagnosticsOpen(false)}
        />
      ) : null}
      {canvasOpen ? (
        <CanvasPanel
          service={noteService}
          translator={translator}
          locale={locale}
          knownNotes={
            notesView.kind === 'ready'
              ? notesView.notes.map((note) => ({
                  noteId: note.noteId,
                  title:
                    note.relativePath.split(/[/\\]/).pop()?.replace(/\.md$/i, '') ??
                    note.relativePath,
                }))
              : []
          }
          registerExitGuard={registerPanelGuard}
          onClose={() => setCanvasOpen(false)}
        />
      ) : null}
      {databaseOpen ? (
        <DatabasePanel
          service={noteService}
          translator={translator}
          locale={locale}
          region={regional.region}
          timeZone={regional.timeZone}
          onOpenNote={(noteId) => {
            setDatabaseOpen(false);
            void openNote(noteId);
          }}
          onSetCell={(row, name, value, retryFailed) => runTrash(row.noteId, row.relativePath, () =>
            writeDatabaseCell({
              service: noteService,
              controller: saveController,
              canWrite: noteId => lifetimeActiveRef.current && sourceWritableRef.current &&
                !catalogOpenNotesRef.current.some(note => note.noteId === noteId && note.readOnly),
              onSourceApplied: showEditedSource,
            }, { noteId: row.noteId, sourceRevision: row.sourceRevision, sourceHash: row.sourceHash, readOnly: row.readOnly, name, value, retryFailed }), true)
          }
          onCreateRow={createDatabaseRow}
          createdNote={creationResult}
          canWrite={writable && !composing && !historyRestoring}
          registerExitGuard={registerPanelGuard}
          onClose={() => setDatabaseOpen(false)}
        />
      ) : null}
      {graphOpen ? (
        <GraphPanel
          service={noteService}
          translator={translator}
          locale={locale}
          focusNoteId={activeDoc?.noteId ?? null}
          onOpenNote={(noteId) => {
            setGraphOpen(false);
            void openNote(noteId);
          }}
          onClose={() => setGraphOpen(false)}
        />
      ) : null}
      {templatesOpen ? (
        <TemplatePanel
          service={noteService}
          translator={translator}
          locale={locale}
          region={regional.region}
          suggestedTitle={activeDoc !== null ? noteTitle(activeDoc.relativePath) : ''}
          writable={writable}
          canInsert={writable && mode !== 'read' && !composing && !historyRestoring && activeDoc?.readOnly === false && sameTemplateTarget(templateTarget, templateInsertionTarget())}
          onInsert={(text, caret) => {
            const applied = applyTemplateInsert(templateTarget, templateInsertionTarget(),
              assistAppliersRef.current.get(templateTarget?.paneId ?? ''), text, caret,
              lifetimeActiveRef.current && sourceWritableRef.current && mode !== 'read' &&
                activeDoc?.readOnly === false && !trashFlight.current && !backupFlight.current &&
                !saveController.isComposing(templateTarget?.noteId ?? ''));
            if (!applied || !templateTarget) return false;
            setTemplatesOpen(false);
            searchJumpNonceRef.current += 1;
            setSearchJump({ noteId: templateTarget.noteId, offset: templateTarget.selection.from + caret, nonce: searchJumpNonceRef.current });
            return true;
          }}
          onCreateNote={(title, text) => {
            const pending = creation.snapshot();
            // The sidebar notice already shows a retained or uncertain request.
            if (!creationAllowedRef.current || pending.working || pending.uncertain) return;
            setTemplatesOpen(false);
            void createFromTemplate(title, text);
          }}
          onClose={() => setTemplatesOpen(false)}
        />
      ) : null}
      {tasksOpen ? (
        <TaskPanel
          service={noteService}
          translator={translator}
          locale={locale}
          regional={regional}
          writable={writable}
          onWrite={(task, rewrite) => runTrash(task.noteId, task.relativePath, () => writeTaskSource({
            service: noteService,
            controller: saveController,
            canWrite: () => lifetimeActiveRef.current && sourceWritableRef.current,
            onSourceApplied: showEditedSource,
          }, task, rewrite))}
          onOpenNote={(noteId, offset) => {
            setTasksOpen(false);
            void openSearchResult(noteId, offset);
          }}
          onClose={() => setTasksOpen(false)}
        />
      ) : null}
      {importOpen ? (
        <ImportPanel
          service={noteService}
          translator={translator}
          locale={locale}
          onImported={() => void refreshNotes({ silent: true })}
          onClose={() => setImportOpen(false)}
        />
      ) : null}
      {exportOpen ? (
        <ExportPanel
          service={noteService}
          translator={translator}
          locale={locale}
          activePath={activeDoc?.relativePath ?? null}
          activeSource={activeDoc === null ? null : editorSource}
          notePaths={notesView.kind === 'ready' ? notesView.notes.map((note) => note.relativePath) : []}
          defaultScope={activeDoc === null ? 'vault' : 'note'}
          onClose={() => setExportOpen(false)}
        />
      ) : null}
      {propertiesOpen && activeDoc !== null ? (
        <PropertyPanel
          service={noteService}
          noteId={activeDoc.noteId}
          source={editorSource}
          translator={translator}
          locale={locale}
          readOnly={!writable || activeDoc.readOnly}
          onApply={(previousSource, nextSource, _label) => {
            // The panel computed a targeted rewrite; the editor buffer receives
            // it so the normal save path (and its conflict handling) still runs.
            // A buffer that changed while the edit was in flight is left alone
            // rather than overwritten with a stale rewrite.
            if (editorSource !== previousSource) {
              return;
            }
            handleDocChanged(activeDoc.noteId, nextSource);
          }}
          onClose={() => setPropertiesOpen(false)}
        />
      ) : null}
      {draftsOpen ? <DraftRecoveryPanel service={noteService} translator={translator} locale={locale} regional={regional} onRestore={restoreDraft} onClose={() => setDraftsOpen(false)} /> : null}
      {historyOpen && activeDoc ? <HistoryPanel writable={writable} noteId={activeDoc.noteId} service={noteService} translator={translator} locale={locale} regional={regional} onRestore={restoreHistory} onClose={() => setHistoryOpen(false)} /> : null}
      {metadataOpen ? <MetadataPanel noteId={activeDoc?.noteId ?? null} service={noteService} translator={translator} locale={locale} onOpenNote={(noteId, _offset) => { setMetadataOpen(false); void openNote(noteId); }} onClose={() => setMetadataOpen(false)} /> : null}
      {quickOpen ? (
        <QuickOpen
          notes={notesView.kind === 'ready' ? notesView.notes : []}
          recentIds={recentNotes}
          translator={translator}
          onOpen={(noteId) => void openQuickOpenNote(noteId)}
          onRecent={rememberNote}
          onClose={() => setQuickOpen(false)}
        />
      ) : null}
      {paletteOpen ? (
        <CommandPalette
          commands={commands}
          recentIds={recentCommands}
          availability={commandAvailability}
          translator={translator}
          onRun={runCommand}
          onRecent={rememberCommand}
          onClose={() => setPaletteOpen(false)}
        />
      ) : null}
      {searchOpen ? <SearchPanel service={noteService} translator={translator} locale={locale} notes={notesView.kind === 'ready' ? notesView.notes : []} recentNotes={openNotes.length > 0 ? openNotes.slice().reverse() : notesView.kind === 'ready' ? notesView.notes.slice(0, 8) : []} onOpenNote={(noteId, sourceOffset) => void openSearchResult(noteId, sourceOffset)} onClose={() => setSearchOpen(false)} /> : null}
      {conflictLocal ? <ConflictPanel local={conflictLocal} service={noteService} translator={translator} locale={locale} onResolved={resolveConflict} onClose={() => { setConflictLocal(null); void refreshNotes({ silent: true }); }} /> : null}
      {deleteLocal ? <DeleteNoteDialog local={deleteLocal} service={noteService} t={translator} locale={locale} run={runTrash} onDeleted={entry => { void reconcileTrash(entry.noteId); setTrashInitial(entry.objectId); setTrashOpen(true); }} onConflict={local => { if (saveController.reportExternalConflict(local.noteId)) setConflictLocal(local); }} onClose={() => setDeleteLocal(null)} /> : null}
    </div>
  );
}
