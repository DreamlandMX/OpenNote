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
  VaultFinding,
  RecoverOutcome,
  SaveResult,
  ServiceError,
  TrashEntry,
  TrashRecord,
  TrashRestoreResult,
  TrashOperation,
  TrashOperationResult,
  TrashCancellation,
} from '../domain';

export type ServiceOutcome<T> = { ok: true; value: T } | { ok: false; error: ServiceError };

export interface CreateNoteRequest {
  relativePath: string;
  source: string;
  operationId: string;
}
export interface CreateCopyRequest extends CreateNoteRequest { sourceNoteId: string; }

export interface SaveNoteRequest {
  noteId: string;
  source: string;
  expectedRevision: number;
  operationId: string;
}

export interface RestoreHistoryRequest {
  noteId: string;
  revisionId: string;
  expectedRevision: number;
  operationId: string;
}

export interface CheckpointRequest {
  noteId: string;
  relativePath: string;
  source: string;
  baseRevision: number;
  baseContentHash: string;
  operationId: string;
}

export interface CheckpointResult {
  draftId: string;
  noteId: string;
  sourceHash: string;
  savedAt: string;
  sequence: number;
}

export interface DraftSummary extends CheckpointResult {
  relativePath: string;
  baseRevision: number;
  baseContentHash: string;
}

export interface NoteDraft extends DraftSummary {
  source: string;
}

export type DraftListRequest = { noteId?: string; beforeSequence?: number; limit?: number };
export interface DraftPage {
  items: readonly DraftSummary[];
  nextBeforeSequence: number | null;
}

export interface ExportDraftRequest {
  noteId: string;
  relativePath: string;
  source: string;
}

export type ExportDraftResult = { outcome: 'saved' | 'cancelled' };
/** Non-note content of a folder; a move or delete that carries only notes refuses it. */
export interface FolderContents {
  otherFiles: readonly string[];
  otherCount: number;
  hasLink: boolean;
}
export interface DeleteNoteRequest { noteId: string; expectedRevision: number; expectedHash: string; operationId: string; }
export interface RestoreTrashRequest { objectId: string; targetPath: string; expectedRevision: number; operationId: string; }
export interface PurgeTrashRequest { objectId: string; expectedRevision: number; operationId: string; }

/** A note the index could not read; surfaced instead of silently skipped. */
export interface UnreadableNote {
  noteId: string;
  relativePath: string;
  code: ErrorCode;
  messageKey: string;
}

export interface TagEntry {
  tag: string;
  noteIds: readonly string[];
  notePaths: readonly string[];
}

export interface TagReport {
  tags: readonly TagEntry[];
  unreadable: readonly UnreadableNote[];
}

export interface Backlink {
  fromNoteId: string;
  fromPath: string;
  kind: 'link' | 'embed';
  label: string | null;
  anchor: string | null;
  offset: number;
}

export interface BacklinkReport {
  noteId: string;
  backlinks: readonly Backlink[];
  unreadable: readonly UnreadableNote[];
}

export type LinkStatus = 'resolved' | 'ambiguousTarget' | 'brokenLink' | 'anchorMissing';

export interface LinkIssue {
  fromNoteId: string;
  fromPath: string;
  target: string;
  /** Visible label from `[[target|label]]`, when the author wrote one. */
  label: string | null;
  anchor: string | null;
  status: LinkStatus;
  candidates: readonly string[];
  offset: number;
}

export interface LinkReport {
  broken: readonly LinkIssue[];
  missingAnchors: readonly LinkIssue[];
  unreadable: readonly UnreadableNote[];
}

export interface HeadingInfo {
  level: number;
  text: string;
  slug: string;
  offset: number;
  line: number;
}

export interface BlockAnchorInfo {
  id: string;
  offset: number;
  line: number;
}

export interface TagOccurrence {
  tag: string;
  offset: number;
  source: 'frontmatter' | 'body';
}

export interface RawLink {
  kind: 'link' | 'embed';
  target: string;
  label: string | null;
  anchor: string | null;
  offset: number;
  length: number;
}

export interface ResolvedLink {
  targetId: string | null;
  targetPath: string | null;
  anchor: string | null;
  status: LinkStatus;
  candidates: readonly string[];
}

export interface ParsedLink {
  link: RawLink;
  resolved: ResolvedLink;
}

export interface NoteMetadataReport {
  noteId: string;
  relativePath: string;
  title: string;
  headings: readonly HeadingInfo[];
  blockAnchors: readonly BlockAnchorInfo[];
  tags: readonly TagOccurrence[];
  links: readonly ParsedLink[];
  backlinks: readonly Backlink[];
  unreadable: readonly UnreadableNote[];
}

export type EmbedStatus =
  | 'ok'
  | 'brokenLink'
  | 'ambiguousTarget'
  | 'anchorMissing'
  | 'cycle'
  | 'depthExceeded';

export interface EmbedExpansion {
  status: EmbedStatus;
  targetId: string | null;
  targetPath: string | null;
  anchor: string | null;
  content: string | null;
  /** Note ids expanded on this path, root first, for cycle-safe nesting. */
  chain: readonly string[];
}

export interface ExpandEmbedRequest {
  noteId: string;
  target: string;
  anchor?: string;
  /** Already-expanded note ids; omit for a fresh expansion. */
  chain?: readonly string[];
}

/** F12 filters: OR within a class, AND across classes. */
export interface SearchFilters {
  folders: readonly string[];
  tags: readonly string[];
  types: readonly string[];
}

export interface SearchRequest {
  query: string;
  filters: SearchFilters;
  locale?: string;
  /** 1–200; the host default is 50. */
  limit?: number;
  cursor?: string | null;
}

export interface SearchHighlight {
  /** UTF-16 offset inside `snippet`. */
  start: number;
  end: number;
}

export interface SearchItem {
  noteId: string;
  relativePath: string;
  title: string;
  snippet: string;
  highlights: readonly SearchHighlight[];
  /** UTF-16 offset in the full source, for jumping to the first hit. */
  sourceOffset: number;
  rank: number;
}

export interface SearchIndexStatus {
  state: 'ready' | 'unavailable';
  version: number;
  indexedNotes: number;
  unreadableNotes: number;
  cachePersisted?: boolean;
}

export interface SearchPage {
  items: readonly SearchItem[];
  nextCursor: string | null;
  indexStatus: SearchIndexStatus;
}

export interface SearchJob {
  jobId: string;
  status: 'running' | 'completed' | 'cancelled' | 'failed';
  phase: 'queued' | 'loadingIndex' | 'reading' | 'indexing' | 'querying' | 'savingIndex';
  processed: number;
  total: number;
  unreadable: number;
  cancelRequested: boolean;
  page: SearchPage | null;
  error: ServiceError | null;
}

/** F09 typed property values; `unset` is distinct from "", 0 and false. */
export type PropertyValue =
  | { kind: 'text'; text: string }
  | { kind: 'number'; value: number }
  | { kind: 'boolean'; value: boolean }
  | { kind: 'date'; date: string }
  | { kind: 'dateTime'; at: string }
  | { kind: 'select'; text: string }
  | { kind: 'multiSelect'; values: readonly string[] }
  | { kind: 'link'; target: string }
  | { kind: 'unset' };

export type HostPropertyValue =
  | { text: string }
  | { number: number }
  | { boolean: boolean }
  | { date: string }
  | { dateTime: string }
  | { select: string }
  | { multiSelect: readonly string[] }
  | { link: string }
  | 'unset';

export interface HostProperty {
  name: string;
  value: HostPropertyValue;
  unknown: boolean;
}

/** Result of a targeted property write: the new source, not a saved note. */
export interface PropertyWrite {
  source: string;
  previous: PropertyValue;
  changed: boolean;
}

/** F25 export scope; the host reads notes through M02. */
export type ExportScope = 'vault' | 'folder' | 'note';

/** F25 §4: ZIP is the default; Markdown writes a plain folder plus a report. */
export type ExportFormat = 'zip' | 'markdown';

export interface ExportRequest {
  scope: ExportScope;
  format?: ExportFormat;
  /** Vault-relative note or folder path for `note`/`folder` scopes. */
  vaultTarget?: string;
  /** Pre-chosen destination; omitted means the native save dialog. */
  target?: string;
  suggestedName?: string;
  dialogTitle?: string;
  includeAttachments: boolean;
}

export interface ExportCompatibilityLoss {
  path: string;
  syntax: 'embed' | 'wikiLink' | 'blockReference';
  target: string;
}

export interface ExportOutcome {
  cancelled: boolean;
  path: string | null;
  notes: number;
  attachments: number;
  losses: readonly string[];
  compatibilityLosses: readonly ExportCompatibilityLoss[];
  verified: boolean;
  bytes: number;
}

export type ImportSourceType = 'folder' | 'zip' | 'notion';
export type ImportState = 'preview' | 'running' | 'interrupted' | 'cancelled' | 'complete' | 'partial';

/** Something an import could not carry over. `reason` is a stable code. */
export interface ImportLoss {
  path: string;
  reason: string;
}

/** F24: where everything will go, before anything is written. */
export interface ImportPreview {
  cancelled: boolean;
  batchId: string;
  notes: number;
  attachments: number;
  /** Up to 200 items as `[source, destination]`. */
  sample: readonly (readonly [string, string])[];
  losses: readonly ImportLoss[];
}

export interface ImportStatus {
  batchId: string;
  state: ImportState;
  sourceName: string;
  total: number;
  finished: number;
  notesDone: number;
  attachmentsDone: number;
  failed: readonly (readonly [string, string])[];
  losses: readonly ImportLoss[];
}

/** F19: one picked file after a real attachment import. */
export interface AttachmentImportItem {
  name: string;
  /** Vault-relative path; empty when the source could not be read. */
  destination: string;
  /** Markdown snippet for the file; empty unless it exists in the vault. */
  reference: string;
  status: 'stored' | 'alreadyPresent' | 'refused' | 'failed';
  errorKey: string | null;
}

export interface AttachmentImportOutcome {
  cancelled: boolean;
  plan: AttachmentPlan | null;
  items: readonly AttachmentImportItem[];
}

/** F19: what the host reports about one attachment before it is read. */
export interface AttachmentInfo {
  relativePath: string;
  size: number;
  sha256: string;
  /** Detected from the file's bytes; null when the app does not display this type. */
  mediaType: string | null;
}

/** F19: a pasted or dropped file, stored under a name the host's planner decides. */
export interface AttachmentBytesRequest {
  destination: string;
  name: string;
  bytes: Uint8Array;
  operationId: string;
}

export interface AttachmentImportRequest {
  /** Vault-relative folder; empty means the vault root. */
  destination: string;
  /** Reused for a retry so already-written files are recognised, not copied. */
  operationId: string;
  dialogTitle?: string;
}

export interface ImportPrepareRequest {
  sourceType: ImportSourceType;
  /** Vault-relative folder the import goes under; empty means the vault root. */
  destinationPrefix: string;
  dialogTitle: string;
}

/** F44 storage/service diagnostics with a redacted, user-reviewed report. */
export type DiagnosticScope =
  | 'storage'
  | 'network'
  | 'entitlement'
  | 'backup'
  | 'compile'
  | 'application';

export type DiagnosticStatus =
  | { state: 'ready' }
  | { state: 'degraded'; reasonKey: string }
  | { state: 'unknown'; reasonKey: string };

export type MeasuredCount =
  | { state: 'known'; value: number }
  | { state: 'unknown' };

export type CountKind =
  | 'notes'
  | 'attachments'
  | 'drafts'
  | 'trashEntries'
  | 'historyRevisions'
  | 'searchDocuments';

export type CapabilityKind =
  | 'search'
  | 'sync'
  | 'cloudBackup'
  | 'latexCompile'
  | 'telemetry';

export type DiagnosticFieldValue =
  | { type: 'text'; value: string }
  | { type: 'integer'; value: number }
  | { type: 'boolean'; value: boolean }
  | { type: 'unknown' };

export interface DiagnosticField {
  key: string;
  value: DiagnosticFieldValue;
}

export interface DiagnosticRedaction {
  kind: string;
  count: number;
}

export interface DiagnosticOmission {
  key: string;
  reasonKey: string;
}

export interface DiagnosticsPreview {
  correlationId: string;
  fields: readonly DiagnosticField[];
  statuses: readonly { scope: DiagnosticScope; status: DiagnosticStatus }[];
  redactions: readonly DiagnosticRedaction[];
  omitted: readonly DiagnosticOmission[];
  includesLog: boolean;
}

export interface DiagnosticsHostFacts {
  appVersion: string;
  buildChannel: string;
  platform: string;
  locale: string;
  /** Roots used only for redaction matching; never reported as fields. */
  roots: readonly string[];
}

/** F44: the real probe values collected from the storage layer. */
export interface DiagnosticsProbe {
  notes: number | null;
  trashEntries: number | null;
  drafts: number | null;
  compileRunning: number | null;
  compileQueued: number | null;
  storageWritable: boolean | null;
  /** Indexed document count, or null when the index is cold or unreadable. */
  searchDocuments: number | null;
  storageStatus: DiagnosticStatus;
  compileStatus: DiagnosticStatus;
  searchStatus: DiagnosticStatus;
}

export interface DiagnosticsRequest {
  facts: DiagnosticsHostFacts;
  correlationId: string;
  counts: readonly { kind: CountKind; value: MeasuredCount }[];
  capabilities: readonly { kind: CapabilityKind; status: DiagnosticStatus }[];
  statuses: readonly { scope: DiagnosticScope; status: DiagnosticStatus }[];
  lastErrorCode: string | null;
  timestampMillis: number;
  rawLog: string | null;
}

/** F19 attachment import planning. */
export type AttachmentKind = 'image' | 'media' | 'document' | 'binary' | 'unknown';
export type AttachmentPreview = 'inline' | 'systemOpen' | 'download';

export type AttachmentIssue =
  | { kind: 'emptyName' }
  | { kind: 'unsafePath'; path: string }
  | { kind: 'tooLarge'; size: number; limit: number }
  | { kind: 'empty'; path: string }
  | { kind: 'duplicateContent'; existing: string }
  | { kind: 'renamedForCollision'; from: string; to: string };

export interface PlannedAttachment {
  destination: string;
  kind: AttachmentKind;
  preview: AttachmentPreview;
  size: number;
  sha256: string;
  /** False when an identical file already exists. */
  storesBytes: boolean;
  /** True when a thumbnail would be generated; it is a cache, not a replacement. */
  cachesThumbnail: boolean;
  /** True when the file exceeds the cloud limit and would be marked unsynced. */
  exceedsCloudLimit: boolean;
  /** The markdown snippet that references the attachment. */
  reference: string;
  issues: readonly AttachmentIssue[];
}

export interface AttachmentPlan {
  attachments: readonly PlannedAttachment[];
  totalBytes: number;
  unsynced: number;
}

export interface AttachmentInput {
  name: string;
  sourcePath: string;
  size: number;
  sha256: string;
}

/** F03 rename commit planning. */
export interface CommitStep {
  noteId: string;
  relativePath: string;
  /** The source after the link rewrite. */
  source: string;
  expectedRevision: number;
  edits: number;
}

export type CommitIssue =
  | { kind: 'revisionChanged'; noteId: string; expected: number; actual: number }
  | { kind: 'targetMissing'; noteId: string }
  | { kind: 'destinationOccupied'; path: string }
  | { kind: 'nestedDestination'; path: string; destination: string };

export interface RenameCommit {
  steps: readonly CommitStep[];
  moveFrom: string;
  moveTo: string;
  /** True when the note id survives the move (F03-R02). */
  keepsIdentity: boolean;
  /** Non-empty means the commit must not run. */
  issues: readonly CommitIssue[];
  /** Issues the plan deliberately left alone. */
  untouched: readonly RenameWarning[];
}

/** F08 tag rename and merge planning. */
export interface TagNote {
  noteId: string;
  relativePath: string;
  source: string;
  revision: number;
}

export interface TagRewrite {
  noteId: string;
  relativePath: string;
  /** The source after the rewrite. */
  source: string;
  changed: number;
  expectedRevision: number;
}

export type TagPlanIssue =
  | { kind: 'sourceUnsupported'; noteId: string }
  | { kind: 'invalidTag'; reason: string }
  | { kind: 'revisionChanged'; noteId: string; expected: number; actual: number }
  | { kind: 'revisionUnknown'; noteId: string }
  | { kind: 'noChange' };

export interface TagRenamePlan {
  oldTag: string;
  newTag: string;
  /** True when the new tag already existed, so this is a merge. */
  isMerge: boolean;
  rewrites: readonly TagRewrite[];
  conflicted: readonly { noteId: string; issue: TagPlanIssue }[];
  totalChanges: number;
}

/** F45 LaTeX project inspection. */
export interface InspectFile {
  path: string;
  bytes: number | null;
  /** Text when readable; used to find the entry file and its references. */
  text: string | null;
}

export interface EntryCandidate {
  path: string;
  /** Higher means more likely. */
  score: number;
  reasonKey: string;
}

export interface ProjectDependency {
  /** `usepackage`, `documentclass`, `bibliography` or `bibresource`. */
  kind: string;
  name: string;
  known: boolean;
}

export type ProjectFinding =
  | { kind: 'entryAmbiguous'; candidates: readonly string[] }
  | { kind: 'noEntry' }
  | { kind: 'unsafeReference'; reference: string; from: string }
  | { kind: 'missingReference'; reference: string; from: string }
  | { kind: 'unknownDependency'; name: string }
  | { kind: 'shellEscape'; from: string }
  | { kind: 'configNotExecuted'; path: string }
  | { kind: 'unsafeRoot'; path: string }
  | { kind: 'movedFromRecord'; recorded: string; found: string };

export interface ProjectInspection {
  root: string;
  entryCandidates: readonly EntryCandidate[];
  /** The chosen entry, only when exactly one candidate is plausible. */
  entry: string | null;
  files: readonly string[];
  dependencies: readonly ProjectDependency[];
  findings: readonly ProjectFinding[];
  /** True when nothing blocks an attempt to build. */
  inspectable: boolean;
}

/** F49 BibTeX/Biber bibliography. */
export type BibBackend = 'bibtex' | 'biber';

export interface BibField {
  name: string;
  value: string;
}

export interface BibEntry {
  citeKey: string;
  entryType: string;
  fields: readonly BibField[];
  /** True when the type is not recognised; the entry is still complete. */
  unknownType: boolean;
  offset: number;
  raw: string;
}

export interface BibError {
  offset: number;
  /** The unparsable block, kept verbatim. */
  raw: string;
  messageKey: string;
}

export interface BibDuplicateKey {
  citeKey: string;
  /** Offsets of every entry claiming this key. */
  offsets: readonly number[];
}

export interface Bibliography {
  entries: readonly BibEntry[];
  errors: readonly BibError[];
  duplicates: readonly BibDuplicateKey[];
}

export type CitationIssue =
  | { kind: 'unknownKey'; citeKey: string }
  | { kind: 'duplicateKey'; citeKey: string; offsets: readonly number[] }
  | { kind: 'invalidKey'; citeKey: string }
  | { kind: 'noKeys' };

export interface CitationPlan {
  fragment: string;
  issues: readonly CitationIssue[];
  hasBibliographyPackage: boolean;
  backend: BibBackend;
}

export interface CitationRenamePlan {
  oldKey: string;
  newKey: string;
  citations: number;
  issues: readonly CitationIssue[];
}

/** F48 structured LaTeX insertion. */
export type InsertKind = 'equation' | 'image' | 'table' | 'section' | 'reference';

export type InsertParameters =
  | { kind: 'equation'; body: string; label: string | null }
  | {
      kind: 'image';
      path: string;
      width: number;
      caption: string | null;
      label: string | null;
    }
  | {
      kind: 'table';
      rows: readonly (readonly string[])[];
      advanced: string | null;
      caption: string | null;
      label: string | null;
    }
  | { kind: 'section'; title: string; level: number; label: string | null }
  | { kind: 'reference'; label: string };

export type InsertionIssue =
  | { kind: 'missing'; field: string }
  | { kind: 'invalidLabel'; label: string }
  | { kind: 'duplicateLabel'; label: string; existing: readonly string[] }
  | { kind: 'unsafeImagePath'; path: string }
  | { kind: 'invalidWidth'; value: number }
  | { kind: 'malformedTable'; detail: string }
  | { kind: 'invalidLevel'; level: number }
  | { kind: 'unknownLabel'; label: string };

export interface InsertionDependency {
  kind: string;
  name: string;
  /** The line that would be added, shown before anything changes. */
  line: string;
  /** True when the preamble already loads it. */
  present: boolean;
}

export interface InsertionPlan {
  fragment: string;
  dependencies: readonly InsertionDependency[];
  /** Empty only when the fragment may be inserted as-is. */
  issues: readonly InsertionIssue[];
  label: string | null;
}

/** F47 LaTeX source editing assistance. */
export type LanguageMode = 'tex' | 'bib' | 'plain';

export interface AssistRange {
  start: number;
  end: number;
}

export interface AssistEdit {
  range: AssistRange;
  text: string;
  /** `comment`, `uncomment`, `wrap` or `complete`. */
  kind: string;
}

export interface AssistResult {
  edits: readonly AssistEdit[];
  /** Selection to restore in the coordinate space of the new text. */
  selection: AssistRange;
  warnings: readonly string[];
}

export interface CompletionCandidate {
  label: string;
  insert: string;
  /** `lexicon` or `project`. */
  source: string;
}

/** F46 LaTeX project scaffolding (template wizard). */
export type LatexTemplateId = 'report' | 'paper' | 'assignment';
export type DocumentLanguage = 'zhCn' | 'en';
export type PaperSize = 'a4' | 'letter';

export interface LatexMetadata {
  title: string;
  /** Empty omits the author line entirely. */
  author: string;
  /** Calendar day `YYYY-MM-DD`. */
  date: string;
  language: DocumentLanguage;
  paper: PaperSize;
}

export type MetadataError =
  | { kind: 'titleEmpty' }
  | { kind: 'titleTooLong'; chars: number; limit: number }
  | { kind: 'authorTooLong'; chars: number; limit: number }
  | { kind: 'invalidDate'; value: string };

export type ScaffoldIssue =
  | { kind: 'collision'; path: string }
  | { kind: 'invalidMetadata'; error: MetadataError }
  | { kind: 'unsafeDestination'; destination: string }
  | { kind: 'unknownTemplate'; templateId: string };

export interface GeneratedFile {
  path: string;
  content: string;
}

export interface ScaffoldPlan {
  destination: string;
  entryFile: string;
  files: readonly GeneratedFile[];
  /** Empty only when the plan may be executed. */
  issues: readonly ScaffoldIssue[];
  requires: readonly string[];
}

/** F54 LaTeX project export planning. */
export type LatexExportKind = 'pdf' | 'sourceZip';

export interface LatexProjectFile {
  path: string;
  bytes: number | null;
  /** File text when available, used to follow \input and \bibliography. */
  text: string | null;
}

export interface LatexExportEntry {
  path: string;
  kind: string;
  bytes: number | null;
}

export type LatexExportIssue =
  | { kind: 'missingFile'; path: string; referencedBy: string }
  | { kind: 'pdfOutdated'; sourceHash: string; pdfSourceHash: string }
  | { kind: 'tooLarge'; path: string; bytes: number; limit: number }
  | { kind: 'noPdf' }
  | { kind: 'manifestMismatch'; expected: string; actual: string };

export interface LatexExportPlan {
  entries: readonly LatexExportEntry[];
  /** Empty only when the export may be called complete. */
  issues: readonly LatexExportIssue[];
  excluded: readonly string[];
  totalBytes: number;
  complete: boolean;
}

/** F51 compile queue. The queue holds state only; no compiler runs here. */
export type JobState =
  | 'preparing'
  | 'queued'
  | 'running'
  | 'succeeded'
  | 'failed'
  | 'cancelled'
  | 'timedOut'
  | 'interrupted';

export interface CompileJob {
  jobId: string;
  projectId: string;
  state: JobState;
  input: { manifestHash: string; sourceHash: string };
  operationId: string;
  artifactId: string | null;
  reasonKey: string | null;
}

export interface CompileRequestOutcome {
  jobId: string;
  state: JobState;
  /** False when an identical request or the same operation id was replayed. */
  created: boolean;
}

/** F52 LaTeX preview bookkeeping and SyncTeX mapping. */
export type MappingState = 'matching' | 'stale' | 'none';
export type PdfState = 'missing' | 'current' | 'outdated';

export interface PreviewArtifact {
  artifactId: string;
  sourceHash: string;
  hasSynctex: boolean;
}

export type BuildOutcome =
  | { published: PreviewArtifact }
  | { failed: { reason: string } }
  | { invalid: { reason: string } };

export interface PreviewState {
  artifactId: string | null;
  pdfState: PdfState;
  mappingState: MappingState;
  noticeKey: string | null;
}

export interface SourcePosition {
  file: string;
  line: number;
  column: number | null;
}

export interface SyncRecord {
  page: number;
  y: number;
  position: SourcePosition;
}

export interface SyncMapping {
  artifactId: string;
  sourceHash: string;
  records: readonly SyncRecord[];
}

export interface JumpTarget {
  position: SourcePosition;
  mappingState: MappingState;
  /** False when the caller must not highlight: the mapping is stale. */
  allowHighlight: boolean;
}

/** F53 LaTeX build diagnostics. */
export type TexSeverity = 'info' | 'warning' | 'error';

export type TexDiagnosticCode =
  | 'texError'
  | 'missingPackage'
  | 'missingFont'
  | 'undefinedControlSequence'
  | 'mathMode'
  | 'runawayArgument'
  | 'boxWarning'
  | 'unknownDiagnostic';

export interface TexDiagnostic {
  severity: TexSeverity;
  code: TexDiagnosticCode;
  /** The message exactly as the compiler wrote it, in its original language. */
  raw: string;
  messageKey: string;
  file: string | null;
  /** Only the line number the compiler itself reported. */
  line: number | null;
  offset: number;
}

export interface TexMissingResource {
  kind: string;
  name: string;
}

export type TexVerdict = 'failed' | 'succeeded' | 'unverified';

export interface TexBuildReport {
  diagnostics: readonly TexDiagnostic[];
  unparsed: readonly string[];
  missing: readonly TexMissingResource[];
  truncated: boolean;
  exitCode: number | null;
  succeeded: boolean;
  verdict: TexVerdict;
  /** The compile job the log belongs to, when the caller associated one (F53-R03). */
  jobId?: string | null;
  /** Source snapshot the log was produced from; locating requires an exact match. */
  sourceHash?: string | null;
}

/** Identity attached to a pasted log so a stale build cannot be located. */
export interface TexLogContext {
  jobId: string | null;
  sourceHash: string | null;
}

/** F21 canvas node; a `note` node holds a note id, never a copy of the body. */
export interface CanvasNode {
  /** Unknown fields stay at their original JSON level, matching Rust flatten. */
  readonly [key: string]: unknown;
  id: string;
  /** `note`, `text`, `group` or an unknown type from a newer file. */
  type: string;
  x: number;
  y: number;
  width: number;
  height: number;
  noteId?: string | null;
  text?: string | null;
  label?: string | null;
  color?: string | null;
}

export interface CanvasEdge {
  readonly [key: string]: unknown;
  id: string;
  from: string;
  to: string;
  label?: string | null;
}

export interface CanvasDocument {
  /** Unknown fields stay at their original JSON level, matching Rust flatten. */
  readonly [key: string]: unknown;
  formatVersion: string;
  nodes: readonly CanvasNode[];
  edges: readonly CanvasEdge[];
}

export type CanvasGeometryError =
  | { kind: 'nonFinite' }
  | { kind: 'tooSmall'; width: string; height: string }
  | { kind: 'danglingEdge'; edgeId: string; missing: string }
  | { kind: 'duplicateNodeId'; nodeId: string }
  | { kind: 'malformed'; detail: string };

/** F21-A02: a canvas file loaded from the vault. */
export interface StoredCanvas {
  relativePath: string;
  /** Hash the next save must still find on disk. */
  sha256: string;
  document: CanvasDocument;
}

export interface CanvasSaveResult {
  relativePath: string;
  sha256: string;
  /** False when the same bytes were already on disk. */
  written: boolean;
}

/** F22-A02: a saved database view file, stored verbatim. */
export interface StoredView {
  relativePath: string;
  sha256: string;
  source: string;
}

export interface CanvasParseResult {
  document: CanvasDocument;
  warnings: readonly CanvasGeometryError[];
}

/** F22: a cell is either a real value or explicitly unset (never a fake zero). */
export type CellValue =
  | { kind: 'unset' }
  | { kind: 'text'; value: string }
  | { kind: 'number'; value: number }
  | { kind: 'boolean'; value: boolean }
  | { kind: 'date'; value: string }
  | { kind: 'datetime'; value: string }
  | { kind: 'select'; value: string }
  | { kind: 'multiselect'; value: readonly string[] }
  | { kind: 'link'; value: string };

export interface DatabaseColumn {
  key: string;
  kind: string;
}

export interface DatabaseRow {
  noteId: string;
  title: string;
  relativePath: string;
  sourceRevision: number;
  sourceHash: string;
  readOnly: boolean;
  cells: readonly CellValue[];
}

export type DatabaseFilterOp = 'equals' | 'notEquals' | 'contains' | 'isSet' | 'isUnset';

export interface DatabaseFilter {
  key: string;
  op: DatabaseFilterOp;
  value: string;
}

export interface DatabaseSort {
  key: string;
  direction: 'ascending' | 'descending';
}

export interface DatabaseTable {
  columns: readonly DatabaseColumn[];
  rows: readonly DatabaseRow[];
  /** Rows the filters removed; shown so "hidden" is not read as "deleted". */
  hidden: number;
}

export interface DatabaseOutcome {
  table: DatabaseTable;
  unreadable: readonly { noteId: string; relativePath: string; code: string; messageKey: string }[];
}

/** F15 graph node; identity is the note id so a rename keeps the node. */
export interface GraphNode {
  id: string;
  title: string;
  relativePath: string;
  /** `note` for a real note, `missing` for an unresolved link target. */
  kind: 'note' | 'missing';
  anchor: boolean;
  depth: number;
}

export interface GraphEdge {
  from: string;
  to: string;
  kind: 'link' | 'embed';
}

export interface GraphReport {
  nodes: readonly GraphNode[];
  edges: readonly GraphEdge[];
  /** True when the node limit cut the result. */
  truncated: boolean;
  unreadable: readonly { noteId: string; relativePath: string; code: string; messageKey: string }[];
}

/** F17 daily note outcome: opened, created, or ambiguous. */
export interface DailyOutcome {
  status: 'opened' | 'created' | 'ambiguous' | 'templateMissing';
  noteId: string;
  relativePath: string;
  matchedBy: 'path' | 'property' | null;
  /** Notes that also claim the day; non-empty only when ambiguous. */
  candidates: readonly {
    noteId: string;
    relativePath: string;
    propertyDate: string | null;
  }[];
}

/** F18 whitelisted template variables; no expressions are evaluated. */
export interface TemplateVariables {
  title: string;
  /** Calendar day `YYYY-MM-DD`, chosen by the user or the target note. */
  date: string;
  time?: string | null;
}

export type ExpansionWarning =
  | { kind: 'unknownVariable'; name: string }
  | { kind: 'unterminated' };

export interface TemplateExpansion {
  fragment: string;
  warnings: readonly ExpansionWarning[];
  /** Byte offset of `{{cursor}}` in the fragment, or null. */
  cursorOffset: number | null;
}

export interface TemplateRow {
  noteId: string;
  relativePath: string;
  title: string;
  /** Raw file content including the `template: true` marker. */
  body: string;
}

/** F16 task status; the Markdown checkbox is the only authority. */
export type TaskStatus = 'open' | 'done';

export interface TaskRow {
  noteId: string;
  relativePath: string;
  sourceRevision: number;
  sourceHash: string;
  readOnly: boolean;
  /** Byte offset of the task line; the UI converts via the line index. */
  offset: number;
  line: number;
  status: TaskStatus;
  text: string;
  /** `YYYY-MM-DD` from `[due:: ...]`, or null. */
  due: string | null;
  depth: number;
}

/** F16: the result of rewriting one task line. */
export interface TaskRewrite {
  source: string;
  /** Offset of the line in the new source, or null when it was removed. */
  offset: number | null;
}

export interface ToggledSource {
  source: string;
  status: TaskStatus;
  offset: number;
}

/** F14: ATX headings outside code, with stable duplicate-suffixed slugs. */
export interface ParsedHeading {
  level: number;
  text: string;
  slug: string;
  /** Byte offset in the source; the UI converts via the line index. */
  offset: number;
  line: number;
}

export type RenameWarningReason = 'ambiguousTarget' | 'brokenLink';

export interface RenameWarning {
  noteId: string;
  relativePath: string;
  offset: number;
  target: string;
  reason: RenameWarningReason;
}

export interface RenameEdit {
  noteId: string;
  relativePath: string;
  offset: number;
  length: number;
  /** Link text as it appears today. */
  before: string;
  /** Rewritten link text; label and anchor preserved. */
  after: string;
}

/** Read-only rename preview: nothing is applied until the host executes it. */
export interface RenameResult {
  commitId: string;
  completed: boolean;
  items: readonly { noteId: string; relativePath: string; localRevision: number; contentHash: string }[];
}

export interface RenamePlan {
  planId: string;
  vaultId: string;
  expectedHashes: readonly { noteId: string; relativePath: string; expectedRevision: number; contentHash: string }[];
  noteId: string;
  fromPath: string;
  toPath: string;
  keepsIdentity: boolean;
  edits: readonly RenameEdit[];
  untouched: readonly RenameWarning[];
}

export interface TagWriteItem {
  noteId: string; relativePath: string; expectedRevision: number; contentHash: string; source: string;
}
export interface TagWritePlan {
  planId: string; vaultId: string; oldTag: string; newTag: string | null; isMerge: boolean; items: readonly TagWriteItem[];
}
export interface TagUpdatePreview { plan: TagRenamePlan; writePlan: TagWritePlan | null; conflictPaths: Readonly<Record<string, string>>; }
export type TagWriteRunner = (action: () => Promise<ServiceOutcome<TagBatchResult>>, noteIds: readonly string[]) => Promise<ServiceOutcome<TagBatchResult>>;

export interface TagBatchResult {
  operationId: string; oldTag: string; newTag: string | null; isMerge: boolean; completed: boolean; cancelled: boolean;
  items: readonly { noteId: string; relativePath: string; result: SaveResult | null; error: ServiceError | null }[];
}

export interface CatalogChange {
  noteId: string;
  kind: 'added' | 'updated' | 'moved' | 'unavailable' | 'readOnlyChanged';
  previousPath: string | null;
  relativePath: string | null;
}
export interface CatalogRefresh {
  notes: readonly NoteSummary[];
  changes: readonly CatalogChange[];
  unavailableNoteIds: readonly string[];
  findings: readonly VaultFinding[];
  fingerprint: string;
  deferred: boolean;
  persisted: boolean;
  writable: boolean;
}

export interface SourceWatchEvent { noteIds: readonly string[]; error: ServiceError | null; }
export type SourceWatchStop = () => Promise<void>;

export interface NoteService {
  refreshCatalog(): Promise<ServiceOutcome<CatalogRefresh>>;
  watchCatalogChanges(onChange: (error: ServiceError | null) => void): Promise<ServiceOutcome<SourceWatchStop>>;
  watchSourceChanges(noteIds: readonly string[], onChange: (event: SourceWatchEvent) => void): Promise<ServiceOutcome<SourceWatchStop>>;
  previewTagUpdate(oldTag: string, newTag: string | null): Promise<ServiceOutcome<TagUpdatePreview>>;
  applyTagUpdate(plan: TagWritePlan, operationId: string): Promise<ServiceOutcome<TagBatchResult>>;
  resumeTagUpdate(operationId: string): Promise<ServiceOutcome<TagBatchResult>>;
  cancelTagUpdate(operationId: string): Promise<ServiceOutcome<TagBatchResult>>;
  listTagUpdates(): Promise<ServiceOutcome<readonly TagBatchResult[]>>;
  /** F44: the host facts a report needs; paths are roots to redact, not fields. */
  diagnosticsHostFacts(): Promise<ServiceOutcome<DiagnosticsHostFacts>>;
  /**
   * F44: real probe values read from storage. Every field is nullable because a
   * probe that cannot read its value must stay unknown rather than become 0.
   */
  diagnosticsProbe(): Promise<ServiceOutcome<DiagnosticsProbe>>;
  /** F44: builds the allowlisted, redacted preview a user reviews. */
  diagnosticsPreview(request: DiagnosticsRequest): Promise<ServiceOutcome<DiagnosticsPreview>>;
  /** F44-R03: the export text, with the log redacted. */
  diagnosticsExportText(
    preview: DiagnosticsPreview,
    request: DiagnosticsRequest,
  ): Promise<ServiceOutcome<string>>;
  /**
   * F44 §6: saves the reviewed report through the native save dialog.
   *
   * The host regenerates the text from the preview and request, so a saved
   * report is redacted even if the caller passes a different string. A null
   * path means the user cancelled; a write failure leaves the preview intact
   * so another destination can be chosen (F44 §7).
   */
  diagnosticsExport(
    preview: DiagnosticsPreview,
    request: DiagnosticsRequest,
    dialogTitle: string,
  ): Promise<ServiceOutcome<string | null>>;
  /** F44: whether the report as a whole may be called healthy. */
  diagnosticsOverall(preview: DiagnosticsPreview): Promise<ServiceOutcome<DiagnosticStatus>>;
  /**
   * F19: plans importing attachments into a folder.
   *
   * An unknown type stays importable and is handed to the system rather than
   * executed, identical bytes are recognised so nothing is stored twice, and the
   * cloud limit marks a file unsynced without blocking local use.
   */
  planAttachmentImport(
    inputs: readonly AttachmentInput[],
    destination: string,
    existing: readonly { path: string; sha256: string }[],
  ): Promise<ServiceOutcome<AttachmentPlan>>;
  /**
   * F19: picks files and stores them in the vault. Bytes are verified before
   * publication and never overwrite an existing file; only `stored` and
   * `alreadyPresent` items exist on disk and may be referenced from a note.
   */
  importAttachments(
    request: AttachmentImportRequest,
  ): Promise<ServiceOutcome<AttachmentImportOutcome>>;
  /** F19: size, hash and detected type of one attachment. */
  attachmentInfo(relativePath: string): Promise<ServiceOutcome<AttachmentInfo>>;
  /** F19: the whole file, checked against the hash in `info`. */
  readAttachment(info: AttachmentInfo): Promise<ServiceOutcome<Uint8Array>>;
  /** Saves one exported document where the user picks; `cancelled` when they did not. */
  saveNoteHtml(request: { suggestedName: string; html: string; dialogTitle: string }): Promise<ServiceOutcome<{ cancelled: boolean }>>;
  /** F20: every PDF in the vault, sorted by path. */
  listVaultPdfs(): Promise<ServiceOutcome<readonly string[]>>;
  /** F20: hands a PDF to the system's default app, for one this app cannot read. */
  openAttachmentWithSystem(relativePath: string): Promise<ServiceOutcome<void>>;
  /** F19: stores a pasted or dropped file. Never replaces different bytes. */
  storeAttachmentBytes(request: AttachmentBytesRequest): Promise<ServiceOutcome<AttachmentImportItem>>;
  /** F19: how an extension is presented, for a picker's preview. */
  classifyAttachment(extension: string): Promise<ServiceOutcome<[string, string]>>;
  /**
   * F08: plans renaming or merging a tag across notes. A tag inside a code fence
   * is never rewritten, and a note that changed since it was read is reported
   * while the others still proceed.
   */
  planTagRename(
    notes: readonly TagNote[],
    oldTag: string,
    newTag: string,
    actualRevisions: readonly { noteId: string; revision: number }[],
    existingTags: readonly string[],
  ): Promise<ServiceOutcome<TagRenamePlan>>;
  /** F08: plans removing a tag; the note itself is never deleted. */
  planTagRemoval(
    notes: readonly TagNote[],
    tag: string,
    actualRevisions: readonly { noteId: string; revision: number }[],
  ): Promise<ServiceOutcome<TagRenamePlan>>;
  /**
   * F45: inspects a LaTeX project. Nothing in the directory is executed, and an
   * ambiguous entry set is reported rather than guessed.
   */
  inspectLatexProject(root: string, files: readonly InspectFile[]): Promise<ServiceOutcome<ProjectInspection>>;
  /** F49: parses a `.bib` file, keeping anything it cannot model. */
  parseBibliography(source: string): Promise<ServiceOutcome<Bibliography>>;
  /**
   * F49: plans a `\cite` command. A duplicate key is refused with every offset
   * rather than resolved silently.
   */
  planCitation(
    bibliography: Bibliography,
    citeKeys: readonly string[],
    backend: BibBackend,
    preamble: string,
  ): Promise<ServiceOutcome<CitationPlan>>;
  /** F49-R02: a multi-file rename plan with per-file revision checks. */
  planCitationRename(request: {
    bibliography: Bibliography;
    sources: readonly { path: string; text: string }[];
    oldKey: string;
    newKey: string;
    expectedRevisions: readonly { path: string; revision: number }[];
    actualRevisions: readonly { path: string; revision: number }[];
  }): Promise<ServiceOutcome<CitationRenamePlan>>;
  /** F49-A02: keys the sources use that the bibliography does not define. */
  bibliographyDangling(
    bibliography: Bibliography,
    sources: readonly string[],
  ): Promise<ServiceOutcome<readonly string[]>>;
  /** F49-R04: entries whose fields a BibTeX run cannot handle safely. */
  bibliographyUnicode(
    bibliography: Bibliography,
    backend: BibBackend,
  ): Promise<ServiceOutcome<readonly string[]>>;
  /**
   * F48: plans one structured insertion. The fragment and any needed dependency
   * come back together so they can be applied in one undo group.
   */
  planLatexInsertion(
    parameters: InsertParameters,
    source: string,
  ): Promise<ServiceOutcome<InsertionPlan>>;
  /** F48: labels already used in the buffer, for the label picker. */
  latexLabels(source: string): Promise<ServiceOutcome<readonly string[]>>;
  /** F47: which language a buffer should be edited as. */
  detectLatexLanguage(path: string, source: string): Promise<ServiceOutcome<LanguageMode>>;
  /**
   * F47: toggles `%` comments. A composition in progress is refused rather than
   * editing mid-IME.
   */
  latexToggleComment(
    source: string,
    selection: AssistRange,
    composing: boolean,
  ): Promise<ServiceOutcome<AssistResult>>;
  /** F47: wraps the selection in `\command{...}`. */
  latexWrapSelection(
    source: string,
    selection: AssistRange,
    command: string,
    composing: boolean,
  ): Promise<ServiceOutcome<AssistResult>>;
  /**
   * F47: the matching bracket, or null when the bracket is escaped or unbalanced.
   * An invented pair is worse than none.
   */
  latexMatchingBracket(source: string, offset: number): Promise<ServiceOutcome<number | null>>;
  /** F47: completion candidates from the lexicon plus the project's commands. */
  latexCompletions(
    source: string,
    prefix: string,
  ): Promise<ServiceOutcome<readonly CompletionCandidate[]>>;
  /** F47: applies one completion, refusing a range that no longer matches. */
  latexApplyCompletion(
    source: string,
    range: AssistRange,
    expected: string,
    insert: string,
  ): Promise<ServiceOutcome<AssistResult>>;
  /**
   * F46: plans a project scaffold without writing anything. The escaping and the
   * file list come from M13, so the preview is exactly what would be published.
   */
  planLatexScaffold(request: {
    templateId: LatexTemplateId;
    metadata: LatexMetadata;
    destination: string;
  }): Promise<ServiceOutcome<ScaffoldPlan>>;
  /** F46: escapes plain text for LaTeX with the same encoder the plan uses. */
  escapeLatexText(text: string): Promise<ServiceOutcome<string>>;
  /**
   * F54: plans a LaTeX project export without writing anything. The plan lists
   * its own issues, so a project is never called complete while a referenced
   * file is missing or the PDF predates the source.
   */
  planLatexExport(request: {
    kind: LatexExportKind;
    files: readonly LatexProjectFile[];
    expectedManifestHash: string;
    actualManifestHash: string;
    sizeLimit?: number | null;
    sourceHash: string;
    pdfSourceHash?: string | null;
  }): Promise<ServiceOutcome<LatexExportPlan>>;
  /** F51: every job in this vault's compile queue. */
  listCompileJobs(): Promise<ServiceOutcome<readonly CompileJob[]>>;
  /**
   * F51: cancels a job the host runs. Builds are requested and reported only
   * by the host (LaTeX workspace); there is no way to report a result here.
   */
  cancelCompileJob(jobId: string): Promise<ServiceOutcome<JobState>>;
  /** F52: the preview state after a build outcome. Pure. */
  previewState(
    previous: PreviewArtifact | null,
    outcome: BuildOutcome,
    currentHash: string,
    hasMapping: boolean,
  ): Promise<ServiceOutcome<PreviewState>>;
  /**
   * F53: parses a LaTeX build log. Pure and side-effect free: it never runs a
   * compiler or installs anything, so a missing package cannot become an
   * automatic system change.
   */
  parseTexLog(
    rawLog: string,
    exitCode: number | null,
    context?: TexLogContext,
  ): Promise<ServiceOutcome<TexBuildReport>>;
  /** F53-R04: the same report with machine-specific paths removed. */
  redactTexLog(
    report: TexBuildReport,
    roots: readonly string[],
  ): Promise<ServiceOutcome<TexBuildReport>>;
  /**
   * F21: parses and validates a `.canvas` document. Pure, so a preview and a
   * later write share one implementation; storing a canvas file needs M02 to
   * accept a non-Markdown path and is not implemented.
   */
  parseCanvas(source: string): Promise<ServiceOutcome<CanvasDocument>>;
  /** F21: validates and serialises a canvas document. */
  serializeCanvas(document: CanvasDocument): Promise<ServiceOutcome<string>>;
  /** F21-A02: `.canvas` files in the vault. */
  listCanvases(): Promise<ServiceOutcome<readonly string[]>>;
  readCanvas(relativePath: string): Promise<ServiceOutcome<StoredCanvas>>;
  /**
   * Saves a canvas. `expectedSha256` is the hash it was loaded with (null for a
   * new file). A file changed elsewhere fails with VERSION_CONFLICT and is left
   * untouched.
   */
  saveCanvas(
    relativePath: string,
    document: CanvasDocument,
    expectedSha256: string | null,
    operationId: string,
  ): Promise<ServiceOutcome<CanvasSaveResult>>;
  /** F22-A02: `.onview.json` files in the vault. */
  listViews(): Promise<ServiceOutcome<readonly string[]>>;
  readView(relativePath: string): Promise<ServiceOutcome<StoredView>>;
  /** Compare-and-swap save; see saveCanvas. */
  saveView(
    relativePath: string,
    source: string,
    expectedSha256: string | null,
    operationId: string,
  ): Promise<ServiceOutcome<CanvasSaveResult>>;
  /**
   * F22/F23: a table projected from note frontmatter. Read-only: the view
   * definition is not persisted yet, so the caller supplies the shape.
   */
  databaseTable(request: {
    columns?: readonly string[];
    filters?: readonly DatabaseFilter[];
    sort?: DatabaseSort | null;
    folder?: string | null;
    recursive?: boolean;
  }): Promise<ServiceOutcome<DatabaseOutcome>>;
  listTrash(): Promise<ServiceOutcome<TrashEntry[]>>;
  readTrash(objectId: string): Promise<ServiceOutcome<TrashRecord>>;
  deleteNote(request: DeleteNoteRequest): Promise<ServiceOutcome<TrashEntry>>;
  restoreTrash(request: RestoreTrashRequest): Promise<ServiceOutcome<TrashRestoreResult>>;
  purgeTrash(request: PurgeTrashRequest): Promise<ServiceOutcome<void>>;
  listTrashOperations(): Promise<ServiceOutcome<TrashOperation[]>>;
  resumeTrash(operationId: string): Promise<ServiceOutcome<TrashOperationResult>>;
  cancelTrash(operationId: string): Promise<ServiceOutcome<TrashCancellation>>;
  listNotes(): Promise<ServiceOutcome<readonly NoteSummary[]>>;
  /** Every visible folder, including empty ones, as vault-relative paths. */
  listFolders(): Promise<ServiceOutcome<readonly string[]>>;
  /** Creates one empty folder; returns its canonical path. */
  createFolder(relativePath: string): Promise<ServiceOutcome<string>>;
  /** What a folder holds besides notes. */
  inspectFolder(relativePath: string): Promise<ServiceOutcome<FolderContents>>;
  /** Removes a folder tree that holds no files. */
  removeEmptyFolder(relativePath: string): Promise<ServiceOutcome<null>>;
  /**
   * Sends a folder holding files other than notes to the system recycle bin and
   * drops the LaTeX projects inside it. Notes inside must be trashed first; a
   * folder already gone counts as done.
   */
  recycleFolder(relativePath: string): Promise<ServiceOutcome<null>>;
  /** Renames or moves a folder tree that holds no files; returns the new path. */
  renameEmptyFolder(from: string, to: string): Promise<ServiceOutcome<string>>;
  readNote(noteId: string): Promise<ServiceOutcome<NoteDocument>>;
  createNote(request: CreateNoteRequest): Promise<ServiceOutcome<NoteDocument>>;
  createCopy(request: CreateCopyRequest): Promise<ServiceOutcome<NoteDocument>>;
  prepareConflict(noteId: string): Promise<ServiceOutcome<NoteDocument>>;
  saveNote(request: SaveNoteRequest): Promise<ServiceOutcome<SaveResult>>;
  recoverVault(): Promise<ServiceOutcome<RecoverOutcome>>;
  writeCheckpoint(request: CheckpointRequest): Promise<ServiceOutcome<CheckpointResult>>;
  listDrafts(request: DraftListRequest): Promise<ServiceOutcome<DraftPage>>;
  readDraft(draftId: string): Promise<ServiceOutcome<NoteDraft>>;
  listHistory(noteId: string): Promise<ServiceOutcome<HistoryListing>>;
  readHistory(noteId: string, revisionId: string): Promise<ServiceOutcome<HistoryRevisionContent>>;
  restoreHistory(request: RestoreHistoryRequest): Promise<ServiceOutcome<SaveResult>>;
  historyRetentionSettings(): Promise<ServiceOutcome<HistoryRetentionSettings>>;
  setHistoryRetention(retentionDays: number, expectedRevision: number): Promise<ServiceOutcome<HistoryRetentionSettings>>;
  previewHistoryCleanup(): Promise<ServiceOutcome<HistoryCleanupPreview>>;
  applyHistoryCleanup(plan: HistoryRetentionPlan, operationId: string): Promise<ServiceOutcome<HistoryCleanupJob>>;
  historyCleanupStatus(jobId: string): Promise<ServiceOutcome<HistoryCleanupJob>>;
  cancelHistoryCleanup(jobId: string): Promise<ServiceOutcome<HistoryCleanupJob>>;
  stopHistoryCleanup(operationId: string): Promise<ServiceOutcome<HistoryCleanupJob>>;
  exportDraft(request: ExportDraftRequest): Promise<ServiceOutcome<ExportDraftResult>>;
  /** Derived metadata: every call rebuilds from current note sources. */
  listTags(): Promise<ServiceOutcome<TagReport>>;
  listBacklinks(noteId: string): Promise<ServiceOutcome<BacklinkReport>>;
  listLinkIssues(): Promise<ServiceOutcome<LinkReport>>;
  readNoteMetadata(noteId: string): Promise<ServiceOutcome<NoteMetadataReport>>;
  /** F12 full-text search over the rebuildable M05 index. */
  searchNotes(request: SearchRequest): Promise<ServiceOutcome<SearchPage>>;
  startSearch(request: SearchRequest): Promise<ServiceOutcome<SearchJob>>;
  searchJobStatus(jobId: string): Promise<ServiceOutcome<SearchJob>>;
  cancelSearch(jobId: string): Promise<ServiceOutcome<void>>;
  /** F14 outline for an unsaved buffer, parsed by the same M04 rules. */
  parseHeadings(source: string): Promise<ServiceOutcome<readonly ParsedHeading[]>>;
  /** F09: typed properties of a supplied buffer (live, unsaved source). */
  parseProperties(source: string): Promise<ServiceOutcome<readonly HostProperty[]>>;
  /**
   * F09: rewrites one frontmatter property and returns the new source. It does
   * not save; the caller applies it to the buffer so the normal save path keeps
   * revision checks in one place.
   */
  /** F25: staged → verified → published ZIP export. */
  exportVault(request: ExportRequest): Promise<ServiceOutcome<ExportOutcome>>;
  /**
   * F15: the relationship graph, whole vault or around one note. Identity is the
   * note id, so a rename never adds a node.
   */
  graphReport(
    focus?: string | null,
    depth?: number,
    limit?: number,
  ): Promise<ServiceOutcome<GraphReport>>;
  /**
   * F17: opens the daily note for `date`, creating it only when absent. The
   * same date twice returns the same note instead of a second file.
   */
  openDailyNote(
    date: string,
    folderPattern?: string,
    templateId?: string,
  ): Promise<ServiceOutcome<DailyOutcome>>;
  /** F18: expands a template body; a preview and the insert use the same call. */
  expandTemplate(
    body: string,
    variables: TemplateVariables,
  ): Promise<ServiceOutcome<TemplateExpansion>>;
  /** F18: notes marked `template: true`, which are ordinary notes. */
  listTemplates(): Promise<ServiceOutcome<readonly TemplateRow[]>>;
  /** F16: every task in the vault, derived from note sources. */
  listTasks(): Promise<ServiceOutcome<readonly TaskRow[]>>;
  /**
   * F16: rewrites one checkbox and returns the new source. It does not save, so
   * the editor buffer keeps the normal revision and conflict path.
   */
  toggleTask(
    source: string,
    offset: number,
    expectedText: string,
    status: TaskStatus,
  ): Promise<ServiceOutcome<ToggledSource>>;
  /**
   * F16-R04: deletes one task line. A separate call from toggling, because
   * completing a task keeps the record and deleting removes it.
   */
  deleteTask(
    source: string,
    offset: number,
    expectedText: string,
  ): Promise<ServiceOutcome<TaskRewrite>>;
  /** F16 §4: sets, replaces or clears the task's due date. */
  setTaskDue(
    source: string,
    offset: number,
    expectedText: string,
    date: string | null,
  ): Promise<ServiceOutcome<TaskRewrite>>;
  /** F24: imports a folder of Markdown; per-note operation ids make retries safe. */
  /** F24: picks a source and plans the import. Writes nothing to the vault. */
  importPrepare(request: ImportPrepareRequest): Promise<ServiceOutcome<ImportPreview>>;
  /** F24: runs a planned batch, or continues a stopped one. Returns at once. */
  importStart(batchId: string): Promise<ServiceOutcome<ImportStatus>>;
  importStatus(batchId: string): Promise<ServiceOutcome<ImportStatus>>;
  /** F24: stops after the item being written; what is imported stays. */
  importCancel(batchId: string): Promise<ServiceOutcome<void>>;
  /** F24: imports that stopped before finishing. */
  importUnfinished(): Promise<ServiceOutcome<readonly ImportStatus[]>>;
  importDiscard(batchId: string): Promise<ServiceOutcome<void>>;
  applyPropertyEdit(
    source: string,
    name: string,
    value: PropertyValue,
  ): Promise<ServiceOutcome<PropertyWrite>>;
  expandNoteEmbed(request: ExpandEmbedRequest): Promise<ServiceOutcome<EmbedExpansion>>;
  /** Plans a rename without applying it (F10-R03). */
  rename(plan: RenamePlan, expectedRevision: number, operationId: string): Promise<ServiceOutcome<RenameResult>>;
  previewNoteRename(noteId: string, toPath: string): Promise<ServiceOutcome<RenamePlan>>;
  /**
   * F03: the commit steps for a rename preview.
   *
   * Read-only: it returns what would be written and refuses when a note changed
   * since the caller read it or the destination is occupied. Execution uses the same sealed plan.
   */
  previewRenameCommit(
    plan: RenamePlan,
  ): Promise<ServiceOutcome<RenameCommit>>;
}
