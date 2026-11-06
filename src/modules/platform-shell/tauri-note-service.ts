import { watchChanges } from './source-watch-service';
import { invoke } from '@tauri-apps/api/core';
import type { BackupService } from '../history-backup/ports';
import { toHostValue, toPropertyValue, type HostValue } from './properties';
import type {
  ErrorCode,
  HistoryCleanupPreview,
  HistoryCleanupJob,
  HistoryListing,
  HistoryRetentionPlan,
  HistoryRetentionSettings,
  HistoryRevisionContent,
  NoteDocument,
  NoteSummary,
  RecoverOutcome,
  SaveResult,
  ServiceError,
  TrashEntry, TrashRecord, TrashRestoreResult, TrashOperation, TrashOperationResult, TrashCancellation,
} from '../domain';
import type {
  BacklinkReport,
  CheckpointRequest,
  CheckpointResult,
  CreateNoteRequest,
  FolderContents,
  CreateCopyRequest,
  DraftListRequest,
  DraftPage,
  EmbedExpansion,
  ExpandEmbedRequest,
  ExportDraftRequest,
  ExportDraftResult,
  LinkReport,
  NoteDraft,
  NoteMetadataReport,
  NoteService,
  HostProperty,
  ParsedHeading,
  PropertyValue,
  PropertyWrite,
  ExportOutcome,
  ExportRequest,
  ImportPrepareRequest,
  ImportPreview,
  ImportStatus,
  TaskRow,
  TaskStatus,
  ToggledSource,
  TaskRewrite,
  TemplateExpansion,
  TemplateRow,
  TemplateVariables,
  DailyOutcome,
  GraphReport,
  DatabaseOutcome,
  CanvasDocument,
  TexBuildReport,
  TexLogContext,
  BuildOutcome,
  PreviewArtifact,
  PreviewState,
  CompileJob,
  JobState,
  LatexExportKind,
  LatexExportPlan,
  LatexProjectFile,
  LatexMetadata,
  LatexTemplateId,
  ScaffoldPlan,
  AssistRange,
  AssistResult,
  CompletionCandidate,
  LanguageMode,
  InsertParameters,
  InsertionPlan,
  BibBackend,
  Bibliography,
  CitationPlan,
  CitationRenamePlan,
  InspectFile,
  ProjectInspection,
  TagNote,
  TagRenamePlan,
  TagPlanIssue,
  TagWritePlan,
  TagUpdatePreview,
  TagBatchResult,
  RenameCommit,
  AttachmentInput,
  AttachmentPlan,
  AttachmentBytesRequest,
  AttachmentImportItem,
  AttachmentImportOutcome,
  AttachmentInfo,
  StoredCanvas,
  StoredView,
  CanvasSaveResult,
  AttachmentImportRequest,
  DiagnosticStatus,
  DiagnosticsHostFacts,
  DiagnosticsPreview,
  DiagnosticsProbe,
  DiagnosticsRequest,
  DatabaseFilter,
  DatabaseSort,
  SaveNoteRequest,
  RenamePlan,
  RenameResult,
  RestoreHistoryRequest,
  SearchPage,
  SearchJob,
  SearchRequest,
  ServiceOutcome,
  TagReport,
} from '../editor/ports';

export type { NoteService };

const GENERIC_SERVICE_ERROR_KEY = 'opennote.service.error';
const NOT_IMPLEMENTED_ERROR_KEY = 'opennote.service.notImplemented';

function noVaultScopeFailure(): ServiceError {
  return {
    code: 'INVALID_INPUT',
    messageKey: 'error.noVaultOpen',
    params: {},
    retryable: false,
    operationId: null,
    retryAfterSeconds: null,
  };
}

const KNOWN_ERROR_CODES: readonly ErrorCode[] = [
  'INVALID_INPUT',
  'UNSUPPORTED_SCHEMA',
  'OBJECT_NOT_FOUND',
  'VERSION_CONFLICT',
  'IDEMPOTENCY_MISMATCH',
  'IO_DENIED',
  'DISK_FULL',
  'FILE_LOCKED',
  'RESCAN_REQUIRED',
  'NOT_IMPLEMENTED',
  'HISTORY_MISSING',
  'PATH_COLLISION',
  'BACKUP_INCOMPLETE',
  'RESTORE_INTERRUPTED',
  'OPERATION_CANCELLED',
];

function isKnownErrorCode(value: unknown): value is ErrorCode {
  return typeof value === 'string' && (KNOWN_ERROR_CODES as readonly string[]).includes(value);
}

function unknownFailure(): ServiceError {
  return {
    code: 'IO_DENIED',
    messageKey: GENERIC_SERVICE_ERROR_KEY,
    params: {},
    retryable: false,
    operationId: null,
    retryAfterSeconds: null,
  };
}

function notImplementedFailure(): ServiceError {
  return {
    code: 'NOT_IMPLEMENTED',
    messageKey: NOT_IMPLEMENTED_ERROR_KEY,
    params: {},
    retryable: false,
    operationId: null,
    retryAfterSeconds: null,
  };
}

function serviceErrorFromThrown(thrown: unknown): ServiceError {
  if (
    thrown !== null &&
    typeof thrown === 'object' &&
    isKnownErrorCode((thrown as { code: unknown }).code)
  ) {
    const candidate = thrown as {
      code: ErrorCode;
      messageKey?: unknown;
      params?: unknown;
      retryable?: unknown;
      operationId?: unknown;
      retryAfterSeconds?: unknown;
    };
    const params: Record<string, string> = {};
    if (
      candidate.params !== null &&
      typeof candidate.params === 'object' &&
      !Array.isArray(candidate.params)
    ) {
      for (const [key, value] of Object.entries(candidate.params as Record<string, unknown>)) {
        if (typeof value === 'string') {
          params[key] = value;
        }
      }
    }
    return {
      code: candidate.code,
      messageKey:
        typeof candidate.messageKey === 'string' ? candidate.messageKey : GENERIC_SERVICE_ERROR_KEY,
      params,
      retryable: candidate.retryable === true,
      operationId: typeof candidate.operationId === 'string' ? candidate.operationId : null,
      retryAfterSeconds:
        typeof candidate.retryAfterSeconds === 'number' && Number.isFinite(candidate.retryAfterSeconds)
          ? candidate.retryAfterSeconds
          : null,
    };
  }
  return unknownFailure();
}

function invokeOutcome<T>(command: string, args: Record<string, unknown>): Promise<ServiceOutcome<T>> {
  return invoke<T>(command, args).then(
    (value) => ({ ok: true as const, value }),
    (thrown: unknown) => ({ ok: false as const, error: serviceErrorFromThrown(thrown) }),
  );
}

const ATTACHMENT_CHUNK_BYTES = 8 * 1024 * 1024;

function attachmentChanged(): { ok: false; error: ServiceError } {
  return { ok: false as const, error: { code: 'VERSION_CONFLICT', messageKey: 'error.attachmentSourceChanged', params: {}, retryable: true, operationId: null, retryAfterSeconds: null } };
}

/** `[u32 little-endian JSON length][JSON][file bytes]`: a raw body, so bytes stay bytes. */
export function attachmentBody(vaultId: string, request: AttachmentBytesRequest): Uint8Array {
  const header = new TextEncoder().encode(JSON.stringify({
    vaultId, destination: request.destination, name: request.name, operationId: request.operationId,
  }));
  const body = new Uint8Array(4 + header.byteLength + request.bytes.byteLength);
  new DataView(body.buffer).setUint32(0, header.byteLength, true);
  body.set(header, 4);
  body.set(request.bytes, 4 + header.byteLength);
  return body;
}

type HostTagRenamePlan = Omit<TagRenamePlan, 'conflicted'> & { conflicted: readonly (readonly [string, TagPlanIssue])[] };
function tagPlanFromHost(plan: HostTagRenamePlan): TagRenamePlan {
  return { ...plan, conflicted: plan.conflicted.map(([noteId, issue]) => ({ noteId, issue })) };
}

interface HostPropertyWrite {
  source: string;
  previous: HostValue;
  changed: boolean;
}

/** The host serializes `PropertyValue` externally tagged, never with `kind`. */
function applyPropertyEditOnHost(
  source: string,
  name: string,
  value: PropertyValue,
): Promise<ServiceOutcome<PropertyWrite>> {
  return invokeOutcome<HostPropertyWrite>('apply_property_edit', {
    source,
    name,
    value: toHostValue(value),
  }).then((outcome) =>
    outcome.ok
      ? {
          ok: true as const,
          value: {
            source: outcome.value.source,
            previous: toPropertyValue(outcome.value.previous),
            changed: outcome.value.changed,
          },
        }
      : outcome,
  );
}

function suggestedFileName(relativePath: string): string | null {
  const segment = relativePath.split(/[\\/]/).pop()?.trim() ?? '';
  return segment === '' ? null : segment;
}

export function newOperationId(): string {
  if (typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (char) => {
    const random = Math.trunc(Math.random() * 16);
    const value = char === 'x' ? random : (random & 0x3) | 0x8;
    return value.toString(16);
  });
}

export const disconnectedNoteService: NoteService = {
  listTrash: async () => ({ ok: false, error: notImplementedFailure() }),
  readTrash: async () => ({ ok: false, error: notImplementedFailure() }),
  deleteNote: async () => ({ ok: false, error: notImplementedFailure() }),
  restoreTrash: async () => ({ ok: false, error: notImplementedFailure() }),
  purgeTrash: async () => ({ ok: false, error: notImplementedFailure() }),
  listTrashOperations: async () => ({ ok: false, error: notImplementedFailure() }),
  resumeTrash: async () => ({ ok: false, error: notImplementedFailure() }),
  cancelTrash: async () => ({ ok: false, error: notImplementedFailure() }),
  createCopy() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  prepareConflict() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  listNotes() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listFolders() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  createFolder() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  inspectFolder() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  removeEmptyFolder() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  recycleFolder() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  renameEmptyFolder() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  readNote() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  createNote() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  saveNote() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  recoverVault() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  writeCheckpoint() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listDrafts() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  readDraft() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listHistory() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  readHistory() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  restoreHistory() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  historyRetentionSettings() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  setHistoryRetention() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  previewHistoryCleanup() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  applyHistoryCleanup() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  historyCleanupStatus() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  cancelHistoryCleanup() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  stopHistoryCleanup() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  exportDraft() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listTags() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listBacklinks() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listLinkIssues() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  readNoteMetadata() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  searchNotes() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  startSearch() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  searchJobStatus() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  cancelSearch() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  parseHeadings() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  applyPropertyEdit() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  parseProperties() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  exportVault() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  importPrepare() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  importStart() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  importStatus() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  importCancel() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  importUnfinished() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  importDiscard() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listTasks() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  toggleTask() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  deleteTask() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  setTaskDue() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  expandTemplate() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  openDailyNote() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  graphReport() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  databaseTable() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  parseCanvas() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  parseTexLog(
    _rawLog: string,
    _exitCode: number | null,
    _context?: TexLogContext,
  ) {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  diagnosticsHostFacts() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  diagnosticsProbe() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  diagnosticsPreview() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  diagnosticsExportText() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  diagnosticsExport() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  diagnosticsOverall() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  planAttachmentImport() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  importAttachments() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  attachmentInfo() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  readAttachment() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  storeAttachmentBytes() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listVaultPdfs() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  saveNoteHtml() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  openAttachmentWithSystem() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listCanvases() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  readCanvas() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  saveCanvas() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listViews() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  readView() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  saveView() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  classifyAttachment() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  previewRenameCommit() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  refreshCatalog() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  watchCatalogChanges() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  watchSourceChanges() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  previewTagUpdate() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  applyTagUpdate() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  resumeTagUpdate() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  cancelTagUpdate() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  listTagUpdates() { return Promise.resolve({ ok: false as const, error: notImplementedFailure() }); },
  planTagRename() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  planTagRemoval() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  inspectLatexProject() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  parseBibliography() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  planCitation() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  planCitationRename() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  bibliographyDangling() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  bibliographyUnicode() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  planLatexInsertion() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  latexLabels() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  detectLatexLanguage() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  latexToggleComment() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  latexWrapSelection() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  latexMatchingBracket() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  latexCompletions() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  latexApplyCompletion() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  planLatexScaffold() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  escapeLatexText() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  planLatexExport() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listCompileJobs() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  cancelCompileJob() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  previewState() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },

  redactTexLog() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  serializeCanvas() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listTemplates() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  expandNoteEmbed() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  rename() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  previewNoteRename() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
};

const missingVaultScopeNoteService: NoteService = {
  listTrash: async () => ({ ok: false, error: noVaultScopeFailure() }),
  readTrash: async () => ({ ok: false, error: noVaultScopeFailure() }),
  deleteNote: async () => ({ ok: false, error: noVaultScopeFailure() }),
  restoreTrash: async () => ({ ok: false, error: noVaultScopeFailure() }),
  purgeTrash: async () => ({ ok: false, error: noVaultScopeFailure() }),
  listTrashOperations: async () => ({ ok: false, error: noVaultScopeFailure() }),
  resumeTrash: async () => ({ ok: false, error: noVaultScopeFailure() }),
  cancelTrash: async () => ({ ok: false, error: noVaultScopeFailure() }),
  createCopy() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  prepareConflict() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  listNotes() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  listFolders() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  createFolder() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  inspectFolder() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  removeEmptyFolder() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  recycleFolder() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  renameEmptyFolder() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  readNote() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  createNote() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  saveNote() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  recoverVault() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  writeCheckpoint() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listDrafts() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  readDraft() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  listHistory() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  readHistory() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  restoreHistory() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  historyRetentionSettings() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  setHistoryRetention() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  previewHistoryCleanup() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  applyHistoryCleanup() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  historyCleanupStatus() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  cancelHistoryCleanup() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  stopHistoryCleanup() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  exportDraft() {
    return Promise.resolve({ ok: false as const, error: notImplementedFailure() });
  },
  listTags() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  listBacklinks() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  listLinkIssues() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  readNoteMetadata() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  searchNotes() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  startSearch() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  searchJobStatus() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  cancelSearch() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  parseHeadings(source: string): Promise<ServiceOutcome<readonly ParsedHeading[]>> {
    // Parsing is pure: an unsaved buffer has no vault scope yet, but it also
    // has no stored path or revision to guess, so no scope is fabricated.
    return invokeOutcome<readonly ParsedHeading[]>('parse_headings', { source });
  },
  applyPropertyEdit(source: string, name: string, value: PropertyValue) {
    // Pure computation too: the host never saves here.
    return applyPropertyEditOnHost(source, name, value);
  },
  parseProperties(source: string): Promise<ServiceOutcome<readonly HostProperty[]>> {
    return invokeOutcome<{ properties: readonly HostProperty[] }>('parse_properties', {
      source,
    }).then((outcome) =>
      outcome.ok ? { ok: true as const, value: outcome.value.properties } : outcome,
    );
  },
  exportVault() {
    // Export needs a vault scope and a native save dialog; the unscoped service
    // cannot pick a target, so it refuses instead of guessing a path.
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  importPrepare() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  importStart() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  importStatus() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  importCancel() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  importUnfinished() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  importDiscard() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  listTasks() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  toggleTask(
    source: string,
    offset: number,
    expectedText: string,
    status: TaskStatus,
  ): Promise<ServiceOutcome<ToggledSource>> {
    // Toggling is pure and needs no vault scope: it rewrites a supplied buffer.
    return invokeOutcome<ToggledSource>('toggle_task', { source, offset, expectedText, status });
  },
  deleteTask(
    source: string,
    offset: number,
    expectedText: string,
  ): Promise<ServiceOutcome<TaskRewrite>> {
    return invokeOutcome<TaskRewrite>('delete_task', { source, offset, expectedText });
  },
  setTaskDue(
    source: string,
    offset: number,
    expectedText: string,
    date: string | null,
  ): Promise<ServiceOutcome<TaskRewrite>> {
    return invokeOutcome<TaskRewrite>('set_task_due', { source, offset, expectedText, date });
  },
  expandTemplate(
    body: string,
    variables: TemplateVariables,
  ): Promise<ServiceOutcome<TemplateExpansion>> {
    return invokeOutcome<TemplateExpansion>('expand_template', { body, variables });
  },
  listTemplates() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  openDailyNote() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  graphReport() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  databaseTable() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  parseCanvas(source: string) {
    // Parsing is pure and needs no vault scope.
    return invokeOutcome<CanvasDocument>('parse_canvas_source', { source });
  },
  parseTexLog(rawLog: string, exitCode: number | null, context?: TexLogContext) {
    return invokeOutcome<TexBuildReport>('parse_tex_log', {
      rawLog,
      exitCode,
      jobId: context?.jobId ?? null,
      sourceHash: context?.sourceHash ?? null,
    });
  },
  previewState(
    previous: PreviewArtifact | null,
    outcome: BuildOutcome,
    currentHash: string,
    hasMapping: boolean,
  ) {
    return invokeOutcome<PreviewState>('preview_state', { previous, outcome, currentHash, hasMapping });
  },

  redactTexLog(report: TexBuildReport, roots: readonly string[]) {
    return invokeOutcome<TexBuildReport>('redact_tex_log', { report, roots });
  },
  serializeCanvas(document: CanvasDocument) {
    return invokeOutcome<string>('serialize_canvas_document', { document });
  },
  planAttachmentImport() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  importAttachments() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  attachmentInfo() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  readAttachment() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  storeAttachmentBytes() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  listVaultPdfs() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  saveNoteHtml() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  openAttachmentWithSystem() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  listCanvases() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  readCanvas() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  saveCanvas() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  listViews() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  readView() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  saveView() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  classifyAttachment() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  previewRenameCommit() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  refreshCatalog() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  watchCatalogChanges() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  watchSourceChanges() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  previewTagUpdate() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  applyTagUpdate() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  resumeTagUpdate() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  cancelTagUpdate() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  listTagUpdates() { return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() }); },
  planTagRename() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  planTagRemoval() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  inspectLatexProject() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  parseBibliography() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  planCitation() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  planCitationRename() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  bibliographyDangling() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  bibliographyUnicode() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  expandNoteEmbed() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  diagnosticsHostFacts() {
    // Host facts need no vault scope: version, platform and locale are global.
    return invokeOutcome<DiagnosticsHostFacts>('diagnostics_host_facts', {});
  },
  diagnosticsProbe() {
    return invokeOutcome<DiagnosticsProbe>('diagnostics_probe', { vaultId: null });
  },
  diagnosticsPreview(request: DiagnosticsRequest) {
    return invokeOutcome<DiagnosticsPreview>('diagnostics_preview', { request });
  },
  diagnosticsExportText(preview: DiagnosticsPreview, request: DiagnosticsRequest) {
    return invokeOutcome<string>('diagnostics_export_text', { preview, request });
  },
  diagnosticsExport(
    preview: DiagnosticsPreview,
    request: DiagnosticsRequest,
    dialogTitle: string,
  ) {
    return invokeOutcome<string | null>('diagnostics_export', {
      preview,
      request,
      dialogTitle,
    });
  },
  diagnosticsOverall(preview: DiagnosticsPreview) {
    return invokeOutcome<DiagnosticStatus>('diagnostics_overall', { preview });
  },
  planLatexInsertion(
    parameters: InsertParameters,
    source: string,
  ): Promise<ServiceOutcome<InsertionPlan>> {
    return invokeOutcome<InsertionPlan>('plan_latex_insertion', { parameters, source });
  },
  latexLabels(source: string): Promise<ServiceOutcome<readonly string[]>> {
    return invokeOutcome<readonly string[]>('latex_labels', { source });
  },
  detectLatexLanguage(path: string, source: string): Promise<ServiceOutcome<LanguageMode>> {
    return invokeOutcome<LanguageMode>('detect_latex_language', { path, source });
  },
  latexToggleComment(
    source: string,
    selection: AssistRange,
    composing: boolean,
  ): Promise<ServiceOutcome<AssistResult>> {
    return invokeOutcome<AssistResult>('latex_toggle_comment', {
      source,
      selection,
      composing,
    });
  },
  latexWrapSelection(
    source: string,
    selection: AssistRange,
    command: string,
    composing: boolean,
  ): Promise<ServiceOutcome<AssistResult>> {
    return invokeOutcome<AssistResult>('latex_wrap_selection', {
      source,
      selection,
      command,
      composing,
    });
  },
  latexMatchingBracket(source: string, offset: number): Promise<ServiceOutcome<number | null>> {
    return invokeOutcome<number | null>('latex_matching_bracket', { source, offset });
  },
  latexCompletions(
    source: string,
    prefix: string,
  ): Promise<ServiceOutcome<readonly CompletionCandidate[]>> {
    return invokeOutcome<readonly CompletionCandidate[]>('latex_completions', {
      source,
      prefix,
    });
  },
  latexApplyCompletion(
    source: string,
    range: AssistRange,
    expected: string,
    insert: string,
  ): Promise<ServiceOutcome<AssistResult>> {
    return invokeOutcome<AssistResult>('latex_apply_completion', {
      source,
      range,
      expected,
      insert,
    });
  },
  planLatexScaffold(request: {
    templateId: LatexTemplateId;
    metadata: LatexMetadata;
    destination: string;
  }) {
    // Scaffolding needs no vault scope for the escaping, but a collision check does;
    // without a vault the plan simply reports no collisions.
    return invokeOutcome<ScaffoldPlan>('plan_latex_scaffold', {
      vaultId: null,
      templateId: request.templateId,
      metadata: request.metadata,
      destination: request.destination,
    });
  },
  escapeLatexText(text: string) {
    return invokeOutcome<string>('escape_latex_text', { text });
  },
  planLatexExport() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  listCompileJobs() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  cancelCompileJob() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  rename() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
  previewNoteRename() {
    return Promise.resolve({ ok: false as const, error: noVaultScopeFailure() });
  },
};

export function createTauriNoteService(
  vaultId: string | null | undefined,
): NoteService | null {
  if (typeof vaultId !== 'string' || vaultId.length === 0) {
    return null;
  }
  const internals = (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  if (internals === undefined) {
    return null;
  }
  const scope: string = vaultId;
  return {
    planAttachmentImport(
      inputs: readonly AttachmentInput[],
      destination: string,
      existing: readonly { path: string; sha256: string }[],
    ): Promise<ServiceOutcome<AttachmentPlan>> {
      return invokeOutcome<AttachmentPlan>('plan_attachment_import', {
        inputs,
        destination,
        existing: existing.map((entry) => [entry.path, entry.sha256]),
      });
    },
    importAttachments(
      request: AttachmentImportRequest,
    ): Promise<ServiceOutcome<AttachmentImportOutcome>> {
      return invokeOutcome<AttachmentImportOutcome>('import_attachments', {
        vaultId: scope,
        destination: request.destination,
        operationId: request.operationId,
        dialogTitle: request.dialogTitle ?? null,
      });
    },
    attachmentInfo(relativePath: string): Promise<ServiceOutcome<AttachmentInfo>> {
      return invokeOutcome<AttachmentInfo>('attachment_info', { vaultId: scope, relativePath });
    },
    async readAttachment(info: AttachmentInfo): Promise<ServiceOutcome<Uint8Array>> {
      const bytes = new Uint8Array(info.size);
      for (let offset = 0; offset < info.size; ) {
        const length = Math.min(ATTACHMENT_CHUNK_BYTES, info.size - offset);
        const chunk = await invokeOutcome<ArrayBuffer>('read_attachment', {
          vaultId: scope, relativePath: info.relativePath, offset, length,
        });
        if (!chunk.ok) return chunk;
        const part = new Uint8Array(chunk.value);
        if (part.byteLength === 0 || part.byteLength > length) return attachmentChanged();
        bytes.set(part, offset);
        offset += part.byteLength;
      }
      const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes as BufferSource));
      const hex = Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
      return hex === info.sha256 ? { ok: true as const, value: bytes } : attachmentChanged();
    },
    saveNoteHtml(request: { suggestedName: string; html: string; dialogTitle: string }): Promise<ServiceOutcome<{ cancelled: boolean }>> {
      return invokeOutcome<{ cancelled: boolean }>('save_note_html', { vaultId: scope, ...request });
    },
    listVaultPdfs(): Promise<ServiceOutcome<readonly string[]>> {
      return invokeOutcome<readonly string[]>('list_vault_pdfs', { vaultId: scope });
    },
    openAttachmentWithSystem(relativePath: string): Promise<ServiceOutcome<void>> {
      return invokeOutcome<void>('open_attachment_with_system', { vaultId: scope, relativePath });
    },
    storeAttachmentBytes(request: AttachmentBytesRequest): Promise<ServiceOutcome<AttachmentImportItem>> {
      return invoke<AttachmentImportItem>('store_attachment_bytes', attachmentBody(scope, request)).then(
        (value) => ({ ok: true as const, value }),
        (thrown: unknown) => ({ ok: false as const, error: serviceErrorFromThrown(thrown) }),
      );
    },
    classifyAttachment(extension: string): Promise<ServiceOutcome<[string, string]>> {
      return invokeOutcome<[string, string]>('classify_attachment_extension', { extension });
    },
    listCanvases(): Promise<ServiceOutcome<readonly string[]>> {
      return invokeOutcome<readonly string[]>('list_canvases', { vaultId: scope });
    },
    readCanvas(relativePath: string): Promise<ServiceOutcome<StoredCanvas>> {
      return invokeOutcome<StoredCanvas>('read_canvas', { vaultId: scope, relativePath });
    },
    saveCanvas(
      relativePath: string,
      document: CanvasDocument,
      expectedSha256: string | null,
      operationId: string,
    ): Promise<ServiceOutcome<CanvasSaveResult>> {
      return invokeOutcome<CanvasSaveResult>('save_canvas', {
        vaultId: scope,
        relativePath,
        document,
        expectedSha256,
        operationId,
      });
    },
    listViews(): Promise<ServiceOutcome<readonly string[]>> {
      return invokeOutcome<readonly string[]>('list_views', { vaultId: scope });
    },
    readView(relativePath: string): Promise<ServiceOutcome<StoredView>> {
      return invokeOutcome<StoredView>('read_view', { vaultId: scope, relativePath });
    },
    saveView(
      relativePath: string,
      source: string,
      expectedSha256: string | null,
      operationId: string,
    ): Promise<ServiceOutcome<CanvasSaveResult>> {
      return invokeOutcome<CanvasSaveResult>('save_view', {
        vaultId: scope,
        relativePath,
        source,
        expectedSha256,
        operationId,
      });
    },
    previewRenameCommit(plan: RenamePlan): Promise<ServiceOutcome<RenameCommit>> {
      return invokeOutcome<RenameCommit>('preview_rename_commit', { vaultId: scope, plan });
    },
    refreshCatalog() { return invokeOutcome('refresh_catalog', { vaultId: scope }); },
    watchCatalogChanges(onChange) { return watchChanges(scope, newOperationId(), null, event => onChange(event.error)); },
    watchSourceChanges(noteIds, onChange) { return watchChanges(scope, newOperationId(), noteIds, onChange); },
    previewTagUpdate(oldTag: string, newTag: string | null): Promise<ServiceOutcome<TagUpdatePreview>> {
      return invokeOutcome<{ plan: HostTagRenamePlan; writePlan: TagWritePlan | null; conflictPaths: Readonly<Record<string, string>> }>('preview_tag_update', { vaultId: scope, oldTag, newTag }).then(outcome => outcome.ok
        ? { ok: true, value: { ...outcome.value, plan: tagPlanFromHost(outcome.value.plan) } } : outcome);
    },
    applyTagUpdate(plan: TagWritePlan, operationId: string): Promise<ServiceOutcome<TagBatchResult>> {
      return invokeOutcome('apply_tag_update', { vaultId: scope, plan, operationId });
    },
    resumeTagUpdate(operationId: string): Promise<ServiceOutcome<TagBatchResult>> {
      return invokeOutcome('resume_tag_update', { vaultId: scope, operationId });
    },
    cancelTagUpdate(operationId: string): Promise<ServiceOutcome<TagBatchResult>> {
      return invokeOutcome('cancel_tag_update', { vaultId: scope, operationId });
    },
    listTagUpdates(): Promise<ServiceOutcome<readonly TagBatchResult[]>> {
      return invokeOutcome('list_tag_updates', { vaultId: scope });
    },
    planTagRename(
      notes: readonly TagNote[],
      oldTag: string,
      newTag: string,
      actualRevisions: readonly { noteId: string; revision: number }[],
      existingTags: readonly string[],
    ): Promise<ServiceOutcome<TagRenamePlan>> {
      return invokeOutcome<HostTagRenamePlan>('plan_tag_rename_command', {
        notes,
        oldTag,
        newTag,
        actualRevisions: actualRevisions.map((entry) => [entry.noteId, entry.revision]),
        existingTags,
      }).then(outcome => outcome.ok ? { ok: true, value: tagPlanFromHost(outcome.value) } : outcome);
    },
    planTagRemoval(
      notes: readonly TagNote[],
      tag: string,
      actualRevisions: readonly { noteId: string; revision: number }[],
    ): Promise<ServiceOutcome<TagRenamePlan>> {
      return invokeOutcome<HostTagRenamePlan>('plan_tag_removal_command', {
        notes,
        tag,
        actualRevisions: actualRevisions.map((entry) => [entry.noteId, entry.revision]),
      }).then(outcome => outcome.ok ? { ok: true, value: tagPlanFromHost(outcome.value) } : outcome);
    },
    inspectLatexProject(
      root: string,
      files: readonly InspectFile[],
    ): Promise<ServiceOutcome<ProjectInspection>> {
      return invokeOutcome<ProjectInspection>('inspect_latex_project', { root, files });
    },
    parseBibliography(source: string): Promise<ServiceOutcome<Bibliography>> {
      return invokeOutcome<Bibliography>('parse_bibliography', { source });
    },
    planCitation(
      bibliography: Bibliography,
      citeKeys: readonly string[],
      backend: BibBackend,
      preamble: string,
    ): Promise<ServiceOutcome<CitationPlan>> {
      return invokeOutcome<CitationPlan>('plan_citation_command', {
        bibliography,
        citeKeys,
        backend,
        preamble,
      });
    },
    planCitationRename(request: {
      bibliography: Bibliography;
      sources: readonly { path: string; text: string }[];
      oldKey: string;
      newKey: string;
      expectedRevisions: readonly { path: string; revision: number }[];
      actualRevisions: readonly { path: string; revision: number }[];
    }): Promise<ServiceOutcome<CitationRenamePlan>> {
      return invokeOutcome<CitationRenamePlan>('plan_citation_rename', {
        bibliography: request.bibliography,
        sources: request.sources.map((source) => [source.path, source.text]),
        oldKey: request.oldKey,
        newKey: request.newKey,
        expectedRevisions: request.expectedRevisions.map((entry) => [entry.path, entry.revision]),
        actualRevisions: request.actualRevisions.map((entry) => [entry.path, entry.revision]),
      });
    },
    bibliographyDangling(
      bibliography: Bibliography,
      sources: readonly string[],
    ): Promise<ServiceOutcome<readonly string[]>> {
      return invokeOutcome<readonly string[]>('bibliography_dangling', {
        bibliography,
        sources,
      });
    },
    bibliographyUnicode(
      bibliography: Bibliography,
      backend: BibBackend,
    ): Promise<ServiceOutcome<readonly string[]>> {
      return invokeOutcome<readonly string[]>('bibliography_unicode', {
        bibliography,
        backend,
      });
    },
    planLatexInsertion(
      parameters: InsertParameters,
      source: string,
    ): Promise<ServiceOutcome<InsertionPlan>> {
      // Pure: no vault scope is needed to compute a fragment.
      return invokeOutcome<InsertionPlan>('plan_latex_insertion', { parameters, source });
    },
    latexLabels(source: string): Promise<ServiceOutcome<readonly string[]>> {
      return invokeOutcome<readonly string[]>('latex_labels', { source });
    },
    listTrash: () => invokeOutcome<TrashEntry[]>('list_trash', { vaultId: scope }),
    readTrash: objectId => invokeOutcome<TrashRecord>('read_trash', { vaultId: scope, objectId }),
    deleteNote: request => invokeOutcome<TrashEntry>('delete_note', { vaultId: scope, ...request }),
    restoreTrash: request => invokeOutcome<TrashRestoreResult>('restore_trash', { vaultId: scope, ...request }),
    purgeTrash: request => invokeOutcome<void>('purge_trash', { vaultId: scope, ...request }),
    listTrashOperations: () => invokeOutcome<TrashOperation[]>('list_trash_operations', { vaultId: scope }),
    resumeTrash: operationId => invokeOutcome<TrashOperationResult>('resume_trash', { vaultId: scope, operationId }),
    cancelTrash: operationId => invokeOutcome<TrashCancellation>('cancel_trash', { vaultId: scope, operationId }),
    createCopy(request: CreateCopyRequest) {
      return invokeOutcome<NoteDocument>('create_copy', { vaultId: scope, ...request });
    },
    prepareConflict(noteId: string) {
      return invokeOutcome<NoteDocument>('prepare_conflict', { vaultId: scope, noteId });
    },
    listNotes(): Promise<ServiceOutcome<readonly NoteSummary[]>> {
      return invokeOutcome<readonly NoteSummary[]>('list_notes', { vaultId: scope });
    },
    listFolders() {
      return invokeOutcome<readonly string[]>('list_folders', { vaultId: scope });
    },
    createFolder(relativePath: string) {
      return invokeOutcome<string>('create_folder', { vaultId: scope, relativePath });
    },
    inspectFolder(relativePath: string) {
      return invokeOutcome<FolderContents>('inspect_folder', { vaultId: scope, relativePath });
    },
    removeEmptyFolder(relativePath: string) {
      return invokeOutcome<null>('remove_empty_folder', { vaultId: scope, relativePath });
    },
    recycleFolder(relativePath: string) {
      return invokeOutcome<null>('recycle_folder', { vaultId: scope, relativePath });
    },
    renameEmptyFolder(from: string, to: string) {
      return invokeOutcome<string>('rename_empty_folder', { vaultId: scope, from, to });
    },
    readNote(noteId: string): Promise<ServiceOutcome<NoteDocument>> {
      return invokeOutcome<NoteDocument>('read_note', { vaultId: scope, noteId });
    },
    createNote(request: CreateNoteRequest): Promise<ServiceOutcome<NoteDocument>> {
      return invokeOutcome<NoteDocument>('create_note', {
        vaultId: scope,
        relativePath: request.relativePath,
        source: request.source,
        operationId: request.operationId,
      });
    },
    saveNote(request: SaveNoteRequest): Promise<ServiceOutcome<SaveResult>> {
      return invokeOutcome<SaveResult>('save_note', {
        vaultId: scope,
        noteId: request.noteId,
        source: request.source,
        expectedRevision: request.expectedRevision,
        operationId: request.operationId,
      });
    },
    recoverVault(): Promise<ServiceOutcome<RecoverOutcome>> {
      return invokeOutcome<RecoverOutcome>('recover_vault', { vaultId: scope });
    },
    writeCheckpoint(request: CheckpointRequest): Promise<ServiceOutcome<CheckpointResult>> {
      return invokeOutcome('write_checkpoint', { ...request, vaultId: scope });
    },
    listDrafts(request: DraftListRequest): Promise<ServiceOutcome<DraftPage>> {
      return invokeOutcome('list_drafts', { ...request, vaultId: scope });
    },
    readDraft(draftId: string): Promise<ServiceOutcome<NoteDraft>> {
      return invokeOutcome('read_draft', { vaultId: scope, draftId });
    },
    listHistory(noteId: string): Promise<ServiceOutcome<HistoryListing>> {
      return invokeOutcome('list_history', { vaultId: scope, noteId });
    },
    readHistory(noteId: string, revisionId: string): Promise<ServiceOutcome<HistoryRevisionContent>> {
      return invokeOutcome<HistoryRevisionContent>('read_history', { vaultId: scope, noteId, revisionId });
    },
    restoreHistory(request: RestoreHistoryRequest): Promise<ServiceOutcome<SaveResult>> {
      return invokeOutcome('restore_history', { ...request, vaultId: scope });
    },
    historyRetentionSettings(): Promise<ServiceOutcome<HistoryRetentionSettings>> {
      return invokeOutcome('history_retention_settings', { vaultId: scope });
    },
    setHistoryRetention(
      retentionDays: number,
      expectedRevision: number,
    ): Promise<ServiceOutcome<HistoryRetentionSettings>> {
      return invokeOutcome('set_history_retention', {
        vaultId: scope,
        retentionDays,
        expectedRevision,
      });
    },
    previewHistoryCleanup(): Promise<ServiceOutcome<HistoryCleanupPreview>> {
      return invokeOutcome('preview_history_cleanup', { vaultId: scope });
    },
    applyHistoryCleanup(
      plan: HistoryRetentionPlan,
      operationId: string,
    ): Promise<ServiceOutcome<HistoryCleanupJob>> {
      return invokeOutcome('apply_history_cleanup', { vaultId: scope, plan, operationId });
    },
    historyCleanupStatus(jobId: string): Promise<ServiceOutcome<HistoryCleanupJob>> { return invokeOutcome('history_cleanup_status', { vaultId: scope, jobId }); },
    cancelHistoryCleanup(jobId: string): Promise<ServiceOutcome<HistoryCleanupJob>> { return invokeOutcome('cancel_history_cleanup', { vaultId: scope, jobId }); },
    stopHistoryCleanup(operationId: string): Promise<ServiceOutcome<HistoryCleanupJob>> { return invokeOutcome('stop_history_cleanup', { vaultId: scope, operationId }); },
    exportDraft(request: ExportDraftRequest): Promise<ServiceOutcome<ExportDraftResult>> {
      const args: Record<string, unknown> = { source: request.source };
      const suggested = suggestedFileName(request.relativePath);
      if (suggested !== null) {
        args.suggestedName = suggested;
      }
      return invoke<boolean>('export_draft', args).then(
        (saved: boolean) => ({
          ok: true as const,
          value: { outcome: saved ? ('saved' as const) : ('cancelled' as const) },
        }),
        (thrown: unknown) => ({ ok: false as const, error: serviceErrorFromThrown(thrown) }),
      );
    },
    listTags(): Promise<ServiceOutcome<TagReport>> {
      return invokeOutcome('list_tags', { vaultId: scope });
    },
    listBacklinks(noteId: string): Promise<ServiceOutcome<BacklinkReport>> {
      return invokeOutcome('list_backlinks', { vaultId: scope, noteId });
    },
    listLinkIssues(): Promise<ServiceOutcome<LinkReport>> {
      return invokeOutcome('list_link_issues', { vaultId: scope });
    },
    readNoteMetadata(noteId: string): Promise<ServiceOutcome<NoteMetadataReport>> {
      return invokeOutcome('read_note_metadata', { vaultId: scope, noteId });
    },
    searchNotes(request: SearchRequest): Promise<ServiceOutcome<SearchPage>> {
      return invokeOutcome('search_notes', { vaultId: scope, request });
    },
    startSearch(request: SearchRequest): Promise<ServiceOutcome<SearchJob>> { return invokeOutcome('start_search', { vaultId: scope, request }); },
    searchJobStatus(jobId: string): Promise<ServiceOutcome<SearchJob>> { return invokeOutcome('search_job_status', { vaultId: scope, jobId }); },
    cancelSearch(jobId: string): Promise<ServiceOutcome<void>> { return invokeOutcome('cancel_search', { vaultId: scope, jobId }); },
    parseHeadings(source: string): Promise<ServiceOutcome<readonly ParsedHeading[]>> {
      return invokeOutcome('parse_headings', { source });
    },
    applyPropertyEdit(source: string, name: string, value: PropertyValue) {
      return applyPropertyEditOnHost(source, name, value);
    },
    parseProperties(source: string): Promise<ServiceOutcome<readonly HostProperty[]>> {
      return invokeOutcome<{ properties: readonly HostProperty[] }>('parse_properties', {
        source,
      }).then((outcome) =>
        outcome.ok ? { ok: true as const, value: outcome.value.properties } : outcome,
      );
    },
    parseCanvas(source: string): Promise<ServiceOutcome<CanvasDocument>> {
      return invokeOutcome<CanvasDocument>('parse_canvas_source', { source });
    },
    parseTexLog(
      rawLog: string,
      exitCode: number | null,
      context?: TexLogContext,
    ): Promise<ServiceOutcome<TexBuildReport>> {
      return invokeOutcome<TexBuildReport>('parse_tex_log', {
        rawLog,
        exitCode,
        jobId: context?.jobId ?? null,
        sourceHash: context?.sourceHash ?? null,
      });
    },
  diagnosticsHostFacts(): Promise<ServiceOutcome<DiagnosticsHostFacts>> {
    return invokeOutcome<DiagnosticsHostFacts>('diagnostics_host_facts', {});
  },
  diagnosticsProbe(): Promise<ServiceOutcome<DiagnosticsProbe>> {
    return invokeOutcome<DiagnosticsProbe>('diagnostics_probe', { vaultId: scope });
  },
  diagnosticsPreview(request: DiagnosticsRequest): Promise<ServiceOutcome<DiagnosticsPreview>> {
    return invokeOutcome<DiagnosticsPreview>('diagnostics_preview', { request });
  },
  diagnosticsExportText(
    preview: DiagnosticsPreview,
    request: DiagnosticsRequest,
  ): Promise<ServiceOutcome<string>> {
    return invokeOutcome<string>('diagnostics_export_text', { preview, request });
  },
  diagnosticsExport(
    preview: DiagnosticsPreview,
    request: DiagnosticsRequest,
    dialogTitle: string,
  ): Promise<ServiceOutcome<string | null>> {
    return invokeOutcome<string | null>('diagnostics_export', {
      preview,
      request,
      dialogTitle,
    });
  },
  diagnosticsOverall(preview: DiagnosticsPreview): Promise<ServiceOutcome<DiagnosticStatus>> {
    return invokeOutcome<DiagnosticStatus>('diagnostics_overall', { preview });
  },
  detectLatexLanguage(path: string, source: string): Promise<ServiceOutcome<LanguageMode>> {
    return invokeOutcome<LanguageMode>('detect_latex_language', { path, source });
  },
  latexToggleComment(
    source: string,
    selection: AssistRange,
    composing: boolean,
  ): Promise<ServiceOutcome<AssistResult>> {
    return invokeOutcome<AssistResult>('latex_toggle_comment', {
      source,
      selection,
      composing,
    });
  },
  latexWrapSelection(
    source: string,
    selection: AssistRange,
    command: string,
    composing: boolean,
  ): Promise<ServiceOutcome<AssistResult>> {
    return invokeOutcome<AssistResult>('latex_wrap_selection', {
      source,
      selection,
      command,
      composing,
    });
  },
  latexMatchingBracket(source: string, offset: number): Promise<ServiceOutcome<number | null>> {
    return invokeOutcome<number | null>('latex_matching_bracket', { source, offset });
  },
  latexCompletions(
    source: string,
    prefix: string,
  ): Promise<ServiceOutcome<readonly CompletionCandidate[]>> {
    return invokeOutcome<readonly CompletionCandidate[]>('latex_completions', {
      source,
      prefix,
    });
  },
  latexApplyCompletion(
    source: string,
    range: AssistRange,
    expected: string,
    insert: string,
  ): Promise<ServiceOutcome<AssistResult>> {
    return invokeOutcome<AssistResult>('latex_apply_completion', {
      source,
      range,
      expected,
      insert,
    });
  },
  planLatexScaffold(request: {
    templateId: LatexTemplateId;
    metadata: LatexMetadata;
    destination: string;
  }): Promise<ServiceOutcome<ScaffoldPlan>> {
    return invokeOutcome<ScaffoldPlan>('plan_latex_scaffold', {
      vaultId: scope,
      templateId: request.templateId,
      metadata: request.metadata,
      destination: request.destination,
    });
  },
  escapeLatexText(text: string): Promise<ServiceOutcome<string>> {
    return invokeOutcome<string>('escape_latex_text', { text });
  },
  planLatexExport(request: {
    kind: LatexExportKind;
    files: readonly LatexProjectFile[];
    expectedManifestHash: string;
    actualManifestHash: string;
    sizeLimit?: number | null;
    sourceHash: string;
    pdfSourceHash?: string | null;
  }): Promise<ServiceOutcome<LatexExportPlan>> {
    return invokeOutcome<LatexExportPlan>('plan_latex_export', {
      kind: request.kind,
      files: request.files,
      expectedManifestHash: request.expectedManifestHash,
      actualManifestHash: request.actualManifestHash,
      sizeLimit: request.sizeLimit ?? null,
      sourceHash: request.sourceHash,
      pdfSourceHash: request.pdfSourceHash ?? null,
    });
  },
  listCompileJobs(): Promise<ServiceOutcome<readonly CompileJob[]>> {
    return invokeOutcome<readonly CompileJob[]>('list_compile_jobs', { vaultId: scope });
  },
  cancelCompileJob(jobId: string): Promise<ServiceOutcome<JobState>> {
    return invokeOutcome<JobState>('cancel_latex_compile', { vaultId: scope, jobId });
  },
    previewState(
      previous: PreviewArtifact | null,
      outcome: BuildOutcome,
      currentHash: string,
      hasMapping: boolean,
    ): Promise<ServiceOutcome<PreviewState>> {
      return invokeOutcome<PreviewState>('preview_state', { previous, outcome, currentHash, hasMapping });
    },

    redactTexLog(
      report: TexBuildReport,
      roots: readonly string[],
    ): Promise<ServiceOutcome<TexBuildReport>> {
      return invokeOutcome<TexBuildReport>('redact_tex_log', { report, roots });
    },
    serializeCanvas(document: CanvasDocument): Promise<ServiceOutcome<string>> {
      return invokeOutcome<string>('serialize_canvas_document', { document });
    },
    databaseTable(request: {
      columns?: readonly string[];
      filters?: readonly DatabaseFilter[];
      sort?: DatabaseSort | null;
      folder?: string | null;
      recursive?: boolean;
    }): Promise<ServiceOutcome<DatabaseOutcome>> {
      return invokeOutcome<DatabaseOutcome>('database_table', {
        vaultId: scope,
        columns: request.columns ?? [],
        filters: request.filters ?? [],
        sort: request.sort ?? null,
        folder: request.folder ?? null,
        recursive: request.recursive ?? true,
      });
    },
    graphReport(
      focus?: string | null,
      depth?: number,
      limit?: number,
    ): Promise<ServiceOutcome<GraphReport>> {
      return invokeOutcome<GraphReport>('graph_report', {
        vaultId: scope,
        focus: focus ?? null,
        depth: depth ?? null,
        limit: limit ?? null,
      });
    },
    openDailyNote(
      date: string,
      folderPattern?: string,
      templateId?: string,
    ): Promise<ServiceOutcome<DailyOutcome>> {
      return invokeOutcome<DailyOutcome>('open_daily_note', {
        vaultId: scope,
        date,
        folderPattern: folderPattern ?? null,
        templateId: templateId ?? null,
      });
    },
    expandTemplate(
      body: string,
      variables: TemplateVariables,
    ): Promise<ServiceOutcome<TemplateExpansion>> {
      return invokeOutcome<TemplateExpansion>('expand_template', { body, variables });
    },
    listTemplates(): Promise<ServiceOutcome<readonly TemplateRow[]>> {
      return invokeOutcome<readonly TemplateRow[]>('list_templates', { vaultId: scope });
    },
    listTasks(): Promise<ServiceOutcome<readonly TaskRow[]>> {
      return invokeOutcome<readonly TaskRow[]>('list_tasks', { vaultId: scope });
    },
    toggleTask(
      source: string,
      offset: number,
      expectedText: string,
      status: TaskStatus,
    ): Promise<ServiceOutcome<ToggledSource>> {
      return invokeOutcome<ToggledSource>('toggle_task', { source, offset, expectedText, status });
    },
    deleteTask(
      source: string,
      offset: number,
      expectedText: string,
    ): Promise<ServiceOutcome<TaskRewrite>> {
      return invokeOutcome<TaskRewrite>('delete_task', { source, offset, expectedText });
    },
    setTaskDue(
      source: string,
      offset: number,
      expectedText: string,
      date: string | null,
    ): Promise<ServiceOutcome<TaskRewrite>> {
      return invokeOutcome<TaskRewrite>('set_task_due', { source, offset, expectedText, date });
    },
    importPrepare(request: ImportPrepareRequest): Promise<ServiceOutcome<ImportPreview>> {
      return invokeOutcome<ImportPreview>('import_prepare', { vaultId: scope, ...request });
    },
    importStart(batchId: string): Promise<ServiceOutcome<ImportStatus>> {
      return invokeOutcome<ImportStatus>('import_start', { vaultId: scope, batchId });
    },
    importStatus(batchId: string): Promise<ServiceOutcome<ImportStatus>> {
      return invokeOutcome<ImportStatus>('import_status', { vaultId: scope, batchId });
    },
    importCancel(batchId: string): Promise<ServiceOutcome<void>> {
      return invokeOutcome<void>('import_cancel', { batchId });
    },
    importUnfinished(): Promise<ServiceOutcome<readonly ImportStatus[]>> {
      return invokeOutcome<readonly ImportStatus[]>('import_unfinished', { vaultId: scope });
    },
    importDiscard(batchId: string): Promise<ServiceOutcome<void>> {
      return invokeOutcome<void>('import_discard', { vaultId: scope, batchId });
    },
    exportVault(request: ExportRequest): Promise<ServiceOutcome<ExportOutcome>> {
      return invokeOutcome<ExportOutcome>('export_vault', {
        vaultId: scope,
        scope: request.scope,
        format: request.format ?? 'zip',
        vaultTarget: request.vaultTarget ?? null,
        target: request.target ?? null,
        suggestedName: request.suggestedName ?? null,
        dialogTitle: request.dialogTitle ?? null,
        includeAttachments: request.includeAttachments,
      });
    },
    rename(plan: RenamePlan, expectedRevision: number, operationId: string): Promise<ServiceOutcome<RenameResult>> {
      return invokeOutcome('rename_note', { vaultId: scope, plan, expectedRevision, operationId });
    },
    previewNoteRename(noteId: string, toPath: string): Promise<ServiceOutcome<RenamePlan>> {
      return invokeOutcome('preview_note_rename', { vaultId: scope, noteId, toPath });
    },
    expandNoteEmbed(request: ExpandEmbedRequest): Promise<ServiceOutcome<EmbedExpansion>> {
      return invokeOutcome('expand_note_embed', {
        vaultId: scope,
        noteId: request.noteId,
        target: request.target,
        anchor: request.anchor ?? null,
        chain: request.chain ?? [],
      });
    },
  };
}

export function resolveNoteService(
  vaultId: string | null | undefined,
): NoteService {
  if (typeof vaultId !== 'string' || vaultId.length === 0) {
    const internals = (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    return internals === undefined ? disconnectedNoteService : missingVaultScopeNoteService;
  }
  return createTauriNoteService(vaultId) ?? disconnectedNoteService;
}

export function resolveBackupService(vaultId: string | null | undefined): BackupService {
  function call<T>(command: string, args: Record<string, unknown> = {}): Promise<ServiceOutcome<T>> {
    if (!vaultId) return Promise.resolve({ ok: false, error: noVaultScopeFailure() });
    if ((globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ === undefined) return Promise.resolve({ ok: false, error: notImplementedFailure() });
    return invokeOutcome<T>(command, { ...args, vaultId });
  }
  return {
    context: () => call('backup_context'),
    chooseDestination: dialogTitle => call('choose_backup_destination', { dialogTitle }),
    setRetention: (selectionId, expectedRevision, retainedSnapshots) => call('set_backup_retention', { selectionId, expectedRevision, retainedSnapshots }),
    start: (selectionId, operationId, kind, planToken) => call('start_backup_task', { selectionId, operationId, kind, planToken: planToken ?? null }),
    preview: (selectionId, operationId, snapshotId, targetName, dialogTitle) => call('preview_backup_restore', { selectionId, operationId, snapshotId, targetName, dialogTitle }),
    previewOverwrite: (selectionId, operationId, snapshotId) => call('preview_backup_overwrite', { selectionId, operationId, snapshotId }),
    previewUpgrade: (selectionId, operationId, snapshotId) => call('preview_backup_upgrade', { selectionId, operationId, snapshotId: snapshotId ?? null }),
    status: jobId => call('backup_job_status', { jobId }),
    cancel: jobId => call('cancel_backup_task', { jobId }),
  };
}
