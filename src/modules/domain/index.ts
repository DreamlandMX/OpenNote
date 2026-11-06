export interface VaultFinding {
  code: string;
  target: string | null;
  messageKey: string;
}

export interface Vault {
  vaultId: string;
  schemaVersion: number;
  displayName: string;
  syncEnabled: boolean;
  writable: boolean;
  findings: VaultFinding[];
}

export interface NoteSummary {
  noteId: string;
  vaultId: string;
  relativePath: string;
  contentHash: string;
  localRevision: number;
  readOnly: boolean;
}

export interface NoteDocument {
  noteId: string;
  vaultId: string;
  relativePath: string;
  source: string;
  localRevision: number;
  contentHash: string;
  readOnly: boolean;
}

export interface SaveResult {
  noteId: string;
  localRevision: number;
  contentHash: string;
}

export interface HistoryRevision {
  revisionId: string;
  noteId: string;
  relativePath: string;
  localRevision: number;
  contentHash: string;
  savedAt: string;
}

export interface HistoryListing {
  items: readonly HistoryRevision[];
  unavailableRevisionIds: readonly string[];
}

export type SnapshotReferenceIssueReason = 'missing' | 'unsafe';
export interface SnapshotReferenceIssue {
  noteId: string;
  notePath: string;
  reference: string;
  reason: SnapshotReferenceIssueReason;
}
export interface SnapshotReferenceReport {
  complete: boolean;
  issues: readonly SnapshotReferenceIssue[];
}
export interface HistoryRevisionContent {
  document: NoteDocument;
  referenceReport: SnapshotReferenceReport;
}

export interface HistoryRetentionSettings {
  retentionDays: number;
  revision: number;
}

export interface HistoryRetentionCandidate {
  revision: HistoryRevision;
  fileHash: string;
  byteLength: number;
}

export type HistoryPreservationReason = 'pendingOperation' | 'unreadable';

export interface HistoryPreservedRevision {
  noteId: string;
  revisionId: string;
  reason: HistoryPreservationReason;
}

export interface HistoryRetentionPlan {
  planId: string;
  retentionDays: number;
  settingsRevision: number;
  generatedAt: string;
  cutoff: string;
  retainedCount: number;
  candidates: readonly HistoryRetentionCandidate[];
  preserved: readonly HistoryPreservedRevision[];
  candidateBytes: number;
}

export interface PendingHistoryCleanup {
  operationId: string;
  plan: HistoryRetentionPlan;
}

export interface HistoryCleanupPreview {
  current: HistoryRetentionPlan;
  pending: readonly PendingHistoryCleanup[];
}

export interface HistoryCleanupResult {
  operationId: string;
  status: 'completed' | 'cancelled';
  removed: readonly HistoryRetentionCandidate[];
  remaining?: readonly HistoryRetentionCandidate[];
}

export interface HistoryCleanupJob {
  jobId: string;
  operationId: string;
  status: 'running' | 'completed' | 'cancelled' | 'failed';
  progress: { removed: number; total: number };
  cancelRequested: boolean;
  result: HistoryCleanupResult | null;
  error: ServiceError | null;
}

export interface RecoverOutcome {
  applied: readonly string[];
  pending: readonly string[];
}

export type ErrorCode =
  | 'INVALID_INPUT'
  | 'UNSUPPORTED_SCHEMA'
  | 'OBJECT_NOT_FOUND'
  | 'VERSION_CONFLICT'
  | 'IDEMPOTENCY_MISMATCH'
  | 'IO_DENIED'
  | 'DISK_FULL'
  | 'FILE_LOCKED'
  | 'RESCAN_REQUIRED'
  | 'NOT_IMPLEMENTED'
  | 'HISTORY_MISSING'
  | 'PATH_COLLISION'
  | 'BACKUP_INCOMPLETE'
  | 'RESTORE_INTERRUPTED'
  | 'OPERATION_CANCELLED';

export interface TrashEntry {
  objectId: string; noteId: string; originalPath: string; contentHash: string;
  deletedRevision: number; deletedAt: string; retentionDays: number;
}
export interface TrashRecord { entry: TrashEntry; document: NoteDocument; }
export interface TrashRestoreResult { noteId: string; relativePath: string; localRevision: number; contentHash: string; }
export interface TrashOperation { operationId: string; kind: 'delete' | 'restore' | 'purge'; phase: 'prepared' | 'cancelling'; entry: TrashEntry; targetPath: string | null; }
export interface TrashOperationResult { kind: TrashOperation['kind']; entry: TrashEntry; restored: TrashRestoreResult | null; }
export type TrashCancellation = 'cancelled' | 'alreadyApplied';

export interface ServiceError {
  code: ErrorCode;
  messageKey: string;
  params: Record<string, string>;
  retryable: boolean;
  operationId: string | null;
  retryAfterSeconds: number | null;
}
