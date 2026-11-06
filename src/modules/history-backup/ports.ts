import type { ServiceError, SnapshotReferenceReport, Vault } from '../domain';
import type { ServiceOutcome } from '../editor/ports';

export type {
  SnapshotReferenceIssue,
  SnapshotReferenceIssueReason,
  SnapshotReferenceReport,
} from '../domain';

export interface Snapshot {
  snapshotId: string; vaultId: string; schemaVersion: number; manifestHash: string;
  createdAt: string; objectCount: number; totalBytes: number; completionState: 'complete';
}
export interface BackupSelection { selectionId: string; destination: string; availableBytes: number | null; checkedAt: string | null; retainedSnapshots: number; preferencesRevision: number; }
export interface SnapshotRetentionPlan { planId: string; retainedSnapshots: number; retained: Snapshot[]; candidates: Snapshot[]; preserved: { snapshotId: string; reason: 'unverified' | 'sourceUnknown' | 'protectedDependency' | 'cleanupPending' | 'orderUnknown' }[]; candidateBytes: number; }
export interface CleanupPreview extends SnapshotRetentionPlan { planToken: string; operationId: string | null; }
export interface RetentionPreview { current: CleanupPreview; pending: CleanupPreview[]; }
export interface SnapshotCleanupResult { operationId: string; status: 'completed' | 'cancelled'; removed: Snapshot[]; remaining: Snapshot[]; quarantined: Snapshot[]; }
export interface RestorePreview { planToken: string; snapshot: Snapshot; target: string; noteCount: number; referenceReport: SnapshotReferenceReport; }
export interface SnapshotRestoreResult { operationId: string; snapshotId: string; manifestHash: string; target: string; noteCount: number; completedAt: string; }
export interface OverwritePreview {
  source: string; planToken: string; operationId: string; snapshot: Snapshot; safetySnapshot: Snapshot;
  target: string; previousRoot: string; noteCount: number; referenceReport: SnapshotReferenceReport;
}
export interface SnapshotOverwriteResult {
  operationId: string; snapshotId: string; manifestHash: string; target: string; previousRoot: string;
  safetySnapshotId: string; noteCount: number; completedAt: string;
}
export interface Overwritten { result: SnapshotOverwriteResult; vault: Vault | null; }
export interface UpgradePreview { source: string; planToken: string; operationId: string | null; snapshot: Snapshot; fromSchema: number; toSchema: number; noteCount: number; }
export interface UpgradePreviews { current: UpgradePreview | null; pending: UpgradePreview[]; }
export interface SchemaUpgradeResult { operationId: string; manifestHash: string; fromSchema: number; toSchema: number; noteCount: number; }
export interface Upgraded { migration: SchemaUpgradeResult; vault: Vault | null; }
export interface SnapshotUpgradeAbandonment { operationId: string; snapshotId: string; }
export type DailyBackupResult =
  | { status: 'notDue'; nextCheckAt: number }
  | { status: 'created' | 'unchanged'; nextCheckAt: number; snapshot: Snapshot };
export type BackupResult =
  | { kind: 'snapshots'; value: Snapshot[] }
  | { kind: 'snapshot'; value: Snapshot }
  | { kind: 'preview'; value: RestorePreview }
  | { kind: 'restored'; value: SnapshotRestoreResult }
  | { kind: 'overwritePreview'; value: OverwritePreview }
  | { kind: 'overwritten'; value: Overwritten }
  | { kind: 'daily'; value: DailyBackupResult }
  | { kind: 'retention'; value: RetentionPreview }
  | { kind: 'cleanup'; value: SnapshotCleanupResult }
  | { kind: 'upgradePreview'; value: UpgradePreviews }
  | { kind: 'upgraded'; value: Upgraded }
  | { kind: 'upgradeAbandoned'; value: SnapshotUpgradeAbandonment };
export interface BackupJob {
  jobId: string; kind: 'list' | 'create' | 'preview' | 'restore' | 'overwritePreview' | 'overwrite' | 'daily' | 'retention' | 'cleanup' | 'cancelCleanup' | 'upgradePreview' | 'upgrade' | 'upgradeAbandon';
  status: 'running' | 'succeeded' | 'cancelled' | 'failed';
  progress: { phase: string; completedObjects: number; totalObjects: number };
  cancelRequested: boolean; result: BackupResult | null; error: ServiceError | null;
}
export interface BackupContext { selection: BackupSelection | null; job: BackupJob | null; savedDestination: string | null; configurationError: ServiceError | null; }
export interface BackupService {
  context(): Promise<ServiceOutcome<BackupContext>>;
  chooseDestination(dialogTitle: string): Promise<ServiceOutcome<BackupSelection | null>>;
  setRetention(selectionId: string, expectedRevision: number, retainedSnapshots: number): Promise<ServiceOutcome<BackupSelection>>;
  start(selectionId: string, operationId: string, kind: 'list' | 'create' | 'restore' | 'overwrite' | 'daily' | 'retention' | 'cleanup' | 'cancelCleanup' | 'upgrade' | 'upgradeAbandon', planToken?: string): Promise<ServiceOutcome<BackupJob>>;
  previewUpgrade(selectionId: string, operationId: string, snapshotId?: string): Promise<ServiceOutcome<BackupJob>>;
  preview(selectionId: string, operationId: string, snapshotId: string, targetName: string, dialogTitle: string): Promise<ServiceOutcome<BackupJob | null>>;
  previewOverwrite(selectionId: string, operationId: string, snapshotId: string): Promise<ServiceOutcome<BackupJob>>;
  status(jobId: string): Promise<ServiceOutcome<BackupJob>>;
  cancel(jobId: string): Promise<ServiceOutcome<BackupJob>>;
}
