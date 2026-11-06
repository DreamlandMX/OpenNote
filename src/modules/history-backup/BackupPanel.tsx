import { formatInstant, formatNumber, type RegionalPreferences } from '../platform-shell/regional';
import { useEffect, useRef, useState } from 'react';
import type { ServiceError } from '../domain';
import type { Translator, SupportedLocaleId } from '../presentation-i18n';
import { serviceErrorMessage } from '../presentation-i18n';
import { newOperationId } from '../platform-shell/tauri-note-service';
import type { BackupService, BackupSelection, BackupJob, Snapshot, RestorePreview, SnapshotRestoreResult, OverwritePreview, Overwritten, RetentionPreview, CleanupPreview, SnapshotCleanupResult, UpgradePreview, UpgradePreviews, Upgraded } from './ports';
import { monitorBackupTask } from './backup-task';
import { startDailyBackupChecks } from './daily-checks';
import { ReferenceIssues } from './ReferenceIssues';

interface Props {
  service: BackupService; t: Translator; locale: SupportedLocaleId; regional: RegionalPreferences; visible: boolean; canStart: boolean; canSchedule: boolean; canOverwrite: boolean;
  run(action: () => Promise<void>, reattach?: boolean): Promise<boolean>;
  onClose(): void;
  canUpgrade: boolean;
  onUpgraded(value: Upgraded): 'accepted' | 'wait' | 'rejected';
  onOverwritten(value: Overwritten): 'accepted' | 'wait' | 'rejected';
}
function pathLabel(value: string): string {
  const prefix = '\\\\?\\';
  return value.startsWith(prefix + 'UNC\\') ? '\\\\' + value.slice(prefix.length + 4) : value.startsWith(prefix) ? value.slice(prefix.length) : value;
}
function byteLabel(value: number, locale: string): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']; let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
  return `${formatNumber(value, locale, { maximumFractionDigits: 1 })} ${units[unit]}`;
}
export function BackupPanel({ service, t, locale, regional, visible, canStart, canSchedule, canOverwrite, canUpgrade, run, onClose, onUpgraded, onOverwritten }: Props) {
  const alive = useRef(true);
  const contextEpoch = useRef(0);
  const actionInFlight = useRef(false);
  const automatic = useRef<{ canRun: boolean; poll(): Promise<boolean | 'deferred'>; nextCheck: number | null }>({ canRun: false, poll: async () => true, nextCheck: null });
  const heading = useRef<HTMLHeadingElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const cleanupDialog = useRef<HTMLDialogElement>(null);
  const cleanupCancel = useRef<HTMLButtonElement>(null);
  const upgradeDialog = useRef<HTMLDialogElement>(null);
  const upgradeCancel = useRef<HTMLButtonElement>(null);
  const abandonDialog = useRef<HTMLDialogElement>(null);
  const abandonCancel = useRef<HTMLButtonElement>(null);
  const overwriteDialog = useRef<HTMLDialogElement>(null);
  const overwriteCancel = useRef<HTMLButtonElement>(null);
  const opener = useRef<Element | null>(null);
  const [selection, setSelection] = useState<BackupSelection | null>(null);
  const [rememberedPath, setRememberedPath] = useState<string | null>(null);
  const [entries, setEntries] = useState<Snapshot[]>([]);
  const [listLoaded, setListLoaded] = useState(false);
  const [selected, setSelected] = useState<Snapshot | null>(null);
  const [preview, setPreview] = useState<RestorePreview | null>(null);
  const [overwriteView, setOverwriteView] = useState<OverwritePreview | null>(null);
  const [overwriteConfirm, setOverwriteConfirm] = useState<OverwritePreview | null>(null);
  const [pendingOverwrite, setPendingOverwrite] = useState<Overwritten | null>(null);
  const [overwriteAcknowledged, setOverwriteAcknowledged] = useState(false);
  const [overwritePhrase, setOverwritePhrase] = useState('');
  const [overwriteFailure, setOverwriteFailure] = useState<'operation' | 'reopen' | null>(null);
  const [confirming, setConfirming] = useState<RestorePreview | null>(null);
  const [restored, setRestored] = useState<SnapshotRestoreResult | null>(null);
  const [job, setJob] = useState<BackupJob | null>(null);
  const [working, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<ServiceError | null>(null);
  const [notice, setNotice] = useState<'complete' | 'cancelled' | 'dailyUnchanged' | 'upgradeDone' | 'upgradeCancelled' | 'upgradeAbandoned' | null>(null);
  const [dailyFailure, setDailyFailure] = useState(false);
  const [cleanupFailure, setCleanupFailure] = useState(false);
  const [nextDailyCheck, setNextDailyCheck] = useState<number | null>(null);
  const [name, setName] = useState('OpenNote-restored');
  const [composing, setComposing] = useState(false);
  const [retentionCount, setRetentionCount] = useState('7');
  const [retentionPlan, setRetentionPlan] = useState<RetentionPreview | null>(null);
  const [cleanupConfirm, setCleanupConfirm] = useState<CleanupPreview | null>(null);
  const [cleanupResult, setCleanupResult] = useState<SnapshotCleanupResult | null>(null);
  const [upgradeViews, setUpgradeViews] = useState<UpgradePreviews | null>(null);
  const [upgradeConfirm, setUpgradeConfirm] = useState<UpgradePreview | null>(null);
  const [upgradeAbandon, setUpgradeAbandon] = useState<UpgradePreview | null>(null);
  const [upgradeFailure, setUpgradeFailure] = useState<'operation' | 'reopen' | null>(null);
  const [pendingUpgrade, setPendingUpgrade] = useState<Upgraded | null>(null);
  const busy = working || loading;

  async function consume(initial: BackupJob): Promise<boolean> {
    const upgradeTask = initial.kind === 'upgrade' || initial.kind === 'upgradePreview';
    const overwriteTask = initial.kind === 'overwritePreview' || initial.kind === 'overwrite';
    if (initial.kind === 'upgrade' || initial.kind === 'upgradeAbandon') { setEntries([]); setListLoaded(false); setSelected(null); setPreview(null); setRestored(null); setRetentionPlan(null); setUpgradeViews(null); }
    if (initial.kind === 'overwrite') { setEntries([]); setListLoaded(false); setSelected(null); setPreview(null); setRestored(null); setRetentionPlan(null); setOverwriteView(null); }
    if (initial.kind === 'cleanup' || initial.kind === 'cancelCleanup') {
      setEntries([]); setListLoaded(false); setSelected(null); setPreview(null); setRestored(null); setRetentionPlan(null); setCleanupResult(null);
    }
    const errorBeforeCheck = error;
    const failureBeforeCheck = dailyFailure;
    const cleanupFailureBeforeCheck = cleanupFailure;
    const upgradeFailureBeforeCheck = upgradeFailure;
    const overwriteFailureBeforeCheck = overwriteFailure;
    const outcome = await monitorBackupTask(service, initial,
      value => { if (alive.current) { setJob(value); if (value.kind !== 'daily') setError(null); } },
      failure => { if (alive.current) { setError(failure); setCleanupFailure(initial.kind === 'cleanup' || initial.kind === 'cancelCleanup'); setDailyFailure(initial.kind === 'daily'); setUpgradeFailure(upgradeTask ? failure.messageKey === 'error.upgradeReopenRequired' ? 'reopen' : 'operation' : null); setOverwriteFailure(overwriteTask ? failure.messageKey === 'error.overwriteReopen' ? 'reopen' : 'operation' : null); } });
    if (!alive.current) return false;
    if (!outcome.ok) { setError(outcome.error); setDailyFailure(initial.kind === 'daily'); setCleanupFailure(initial.kind === 'cleanup' || initial.kind === 'cancelCleanup'); setUpgradeFailure(upgradeTask ? outcome.error.messageKey === 'error.upgradeReopenRequired' ? 'reopen' : 'operation' : null); setOverwriteFailure(overwriteTask ? outcome.error.messageKey === 'error.overwriteReopen' ? 'reopen' : 'operation' : null); return false; }
    const value = outcome.value;
    if (value.status === 'failed') { setError(value.error); setDailyFailure(value.kind === 'daily'); setCleanupFailure(value.kind === 'cleanup' || value.kind === 'cancelCleanup'); setUpgradeFailure(upgradeTask ? value.error?.messageKey === 'error.upgradeReopenRequired' ? 'reopen' : 'operation' : null); setOverwriteFailure(overwriteTask ? value.error?.messageKey === 'error.overwriteReopen' ? 'reopen' : 'operation' : null); return false; }
    const result = value.result;
    if (result?.kind === 'cleanup') { setCleanupResult(result.value); setNotice(result.value.status === 'cancelled' ? 'cancelled' : 'complete'); setError(null); return true; }
    if (value.status === 'cancelled') { setUpgradeFailure(null); setError(null); setNotice(upgradeTask ? 'upgradeCancelled' : 'cancelled'); return true; }
    if (result?.kind === 'upgradePreview') { setUpgradeViews(result.value); setUpgradeFailure(null); setError(null); return true; }
    if (result?.kind === 'upgraded') { setError(null); setNotice(null); setUpgradeFailure(null); setPendingUpgrade(result.value); return true; }
    if (result?.kind === 'upgradeAbandoned') {
      setError(null);
      setUpgradeFailure(null);
      setNotice('upgradeAbandoned');
      return true;
    }
    if (result?.kind === 'overwritePreview') { setOverwriteView(result.value); setOverwriteFailure(null); setError(null); return true; }
    if (result?.kind === 'overwritten') { setOverwriteFailure(null); setError(null); setPendingOverwrite(result.value); return true; }
    setOverwriteView(null);
    if (result?.kind === 'retention') { setRetentionPlan(result.value); setError(null); return true; }
    if (result?.kind === 'daily') {
      setNextDailyCheck(result.value.nextCheckAt);
      if (result.value.status === 'notDue') { setError(errorBeforeCheck); setDailyFailure(failureBeforeCheck); setCleanupFailure(cleanupFailureBeforeCheck); setUpgradeFailure(upgradeFailureBeforeCheck); setOverwriteFailure(overwriteFailureBeforeCheck); return true; }
      setError(null); setDailyFailure(false);
      setNotice(result.value.status === 'unchanged' ? 'dailyUnchanged' : 'complete');
      const snapshot = result.value.snapshot;
      setRetentionPlan(null);
      setEntries(items => [snapshot, ...items.filter(item => item.snapshotId !== snapshot.snapshotId)]);
      setSelected(current => current ?? snapshot);
      return true;
    }
    setError(null); setDailyFailure(false); setNotice('complete');
    if (result?.kind === 'snapshots') { setListLoaded(true); setEntries(result.value); setSelected(current => result.value.find(item => item.snapshotId === current?.snapshotId) ?? result.value[0] ?? null); }
    if (result?.kind === 'snapshot') { setRetentionPlan(null); setEntries(items => [result.value, ...items.filter(item => item.snapshotId !== result.value.snapshotId)]); setSelected(result.value); setPreview(null); setOverwriteView(null); setRestored(null); }
    if (result?.kind === 'preview') setPreview(result.value);
    if (result?.kind === 'restored') { setRestored(result.value); setPreview(null); }
    return true;
  }
  async function action(work: () => Promise<void>, reattach = false, quiet = false) {
    if (actionInFlight.current || (loading && !reattach)) return false;
    actionInFlight.current = true; setBusy(true);
    if (!quiet) { setError(null); setDailyFailure(false); setCleanupFailure(false); setUpgradeFailure(null); setOverwriteFailure(null); setNotice(null); setJob(null); }
    try { return await run(work, reattach); }
    finally { actionInFlight.current = false; if (alive.current) setBusy(false); }
  }
  async function refreshUpgradeViews() {
    if (!selection) return;
    await action(async () => {
      const result = await service.previewUpgrade(selection.selectionId, newOperationId());
      if (result.ok) await consume(result.value); else if (alive.current) setError(result.error);
    }, false, true);
  }
  async function dailyCheck(): Promise<boolean | 'deferred'> {
    if (!selection) return true;
    let success = true;
    let reachedHost = false;
    await action(async () => {
      reachedHost = true;
      const result = await service.start(selection.selectionId, newOperationId(), 'daily');
      if (result.ok) { success = await consume(result.value); }
      else { success = false; if (alive.current) { setError(result.error); setDailyFailure(true); setCleanupFailure(false); } }
    }, false, true);
    return reachedHost ? success : 'deferred';
  }
  automatic.current = { canRun: alive.current && canSchedule && canStart && !busy && !confirming && !cleanupConfirm && !upgradeConfirm && !upgradeAbandon && !overwriteConfirm && !pendingUpgrade && !pendingOverwrite && !overwriteView && !composing && !!selection && !actionInFlight.current, poll: dailyCheck, nextCheck: nextDailyCheck };
  useEffect(() => {
    const loop = startDailyBackupChecks(() => automatic.current.canRun, () => automatic.current.poll(), () => automatic.current.nextCheck);
    const wake = () => { if (document.visibilityState === 'visible') void loop.check(); };
    document.addEventListener('visibilitychange', wake); window.addEventListener('focus', wake);
    return () => { loop.stop(); document.removeEventListener('visibilitychange', wake); window.removeEventListener('focus', wake); };
  }, [service, selection?.selectionId]);
  async function context() {
    const epoch = ++contextEpoch.current; setLoading(true);
    const result = await service.context();
    if (!alive.current || epoch !== contextEpoch.current) return;
    setLoading(false);
    if (!result.ok) { setError(result.error); return; }
    setSelection(result.value.selection);
    setRetentionCount(String(result.value.selection?.retainedSnapshots ?? 7));
    setRetentionPlan(null);
    setCleanupResult(null);
    setRememberedPath(result.value.savedDestination);
    setError(result.value.configurationError);
    if (result.value.job) {
      setJob(result.value.job);
      if (result.value.job.status === 'running') await action(async () => { await consume(result.value.job!); }, true);
      else await consume(result.value.job);
    }
  }
  useEffect(() => { alive.current = true; void context(); return () => { alive.current = false; contextEpoch.current++; dialog.current?.close(); cleanupDialog.current?.close(); upgradeDialog.current?.close(); abandonDialog.current?.close(); overwriteDialog.current?.close(); }; }, [service]);
  useEffect(() => {
    if (!visible) return;
    const previous = document.activeElement;
    heading.current?.focus();
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, [visible]);
  useEffect(() => { if (confirming) { dialog.current?.showModal(); cancel.current?.focus(); } }, [confirming]);
  useEffect(() => { if (cleanupConfirm) { cleanupDialog.current?.showModal(); cleanupCancel.current?.focus(); } }, [cleanupConfirm]);
  useEffect(() => { if (overwriteConfirm) { setOverwriteAcknowledged(false); setOverwritePhrase(''); overwriteDialog.current?.showModal(); overwriteCancel.current?.focus(); } }, [overwriteConfirm]);
  useEffect(() => {
    if (!pendingUpgrade || busy) return;
    const outcome = onUpgraded(pendingUpgrade);
    if (outcome === 'wait') return;
    setPendingUpgrade(null);
    if (outcome === 'accepted') setNotice('upgradeDone');
    else { setNotice(null); setUpgradeFailure('reopen'); }
  }, [pendingUpgrade, busy, onUpgraded]);
  useEffect(() => {
    if (!pendingOverwrite || busy) return;
    const outcome = onOverwritten(pendingOverwrite);
    if (outcome === 'wait') return;
    setPendingOverwrite(null);
    if (outcome === 'rejected') { setNotice(null); setOverwriteFailure('reopen'); }
  }, [pendingOverwrite, busy, onOverwritten]);
  useEffect(() => { if (notice === 'upgradeAbandoned' && !busy) void refreshUpgradeViews(); }, [notice, busy]);
  useEffect(() => { setUpgradeViews(null); setUpgradeConfirm(null); upgradeDialog.current?.close(); setUpgradeAbandon(null); abandonDialog.current?.close(); setOverwriteView(null); setOverwriteConfirm(null); overwriteDialog.current?.close(); }, [selection?.selectionId]);
  useEffect(() => { if (upgradeConfirm) { upgradeDialog.current?.showModal(); upgradeCancel.current?.focus(); } }, [upgradeConfirm]);
  useEffect(() => { if (upgradeAbandon) { abandonDialog.current?.showModal(); abandonCancel.current?.focus(); } }, [upgradeAbandon]);
  function closeUpgradeConfirmation() {
    if (busy) return;
    upgradeDialog.current?.close(); setUpgradeConfirm(null);
    if (opener.current instanceof HTMLElement && opener.current.isConnected) opener.current.focus();
  }
  async function inspectUpgrade(snapshotId?: string) {
    if (!selection || composing || Number(retentionCount) !== selection.retainedSnapshots) return;
    await action(async () => {
      setUpgradeViews(null);
      const result = await service.previewUpgrade(selection.selectionId, newOperationId(), snapshotId);
      if (result.ok) await consume(result.value); else if (alive.current) { setError(result.error); setUpgradeFailure('operation'); }
    });
  }
  async function upgrade() {
    if (!selection || !upgradeConfirm || composing || Number(retentionCount) !== selection.retainedSnapshots) return;
    const token = upgradeConfirm.planToken; upgradeDialog.current?.close(); setUpgradeConfirm(null);
    await action(async () => {
      const result = await service.start(selection.selectionId, newOperationId(), 'upgrade', token);
      if (result.ok) await consume(result.value); else if (alive.current) { setError(result.error); setUpgradeFailure('operation'); }
    });
  }
  function closeAbandonConfirmation() {
    if (busy) return;
    abandonDialog.current?.close(); setUpgradeAbandon(null);
    if (opener.current instanceof HTMLElement && opener.current.isConnected) opener.current.focus();
  }
  async function abandonUpgrade() {
    if (!selection || !upgradeAbandon || composing || Number(retentionCount) !== selection.retainedSnapshots) return;
    const token = upgradeAbandon.planToken;
    abandonDialog.current?.close();
    setUpgradeAbandon(null);
    setUpgradeViews(null);
    await action(async () => {
      const result = await service.start(selection.selectionId, newOperationId(), 'upgradeAbandon', token);
      if (result.ok) await consume(result.value); else if (alive.current) { setError(result.error); setUpgradeFailure('operation'); }
    });
  }
  function closeCleanupConfirmation() {
    if (busy) return;
    cleanupDialog.current?.close(); setCleanupConfirm(null);
    if (opener.current instanceof HTMLElement && opener.current.isConnected) opener.current.focus();
  }
  function closeOverwriteConfirmation() {
    if (busy) return;
    overwriteDialog.current?.close(); setOverwriteConfirm(null);
    if (opener.current instanceof HTMLElement && opener.current.isConnected) opener.current.focus();
  }
  async function inspectOverwrite() {
    if (!selection || !selected || composing || !canOverwrite) return;
    setPreview(null); setRestored(null); setOverwriteView(null);
    await action(async () => {
      const result = await service.previewOverwrite(selection.selectionId, newOperationId(), selected.snapshotId);
      if (result.ok) await consume(result.value); else if (alive.current) { setError(result.error); setOverwriteFailure('operation'); }
    });
  }
  async function runOverwrite() {
    if (!selection || !overwriteConfirm || composing || !canOverwrite) return;
    if (!overwriteAcknowledged || overwritePhrase !== t('backup.overwritePhrase')) return;
    const token = overwriteConfirm.planToken;
    overwriteDialog.current?.close(); setOverwriteConfirm(null);
    await action(async () => {
      const result = await service.start(selection.selectionId, newOperationId(), 'overwrite', token);
      if (result.ok) await consume(result.value); else if (alive.current) { setError(result.error); setOverwriteFailure('operation'); }
    });
  }
  async function runCleanup(value: CleanupPreview, cancelPending = false) {
    if (!selection || composing) return;
    cleanupDialog.current?.close(); setCleanupConfirm(null);
    await action(async () => {
      setEntries([]); setListLoaded(false); setSelected(null); setPreview(null); setRestored(null); setRetentionPlan(null); setCleanupResult(null);
      const result = await service.start(selection.selectionId, newOperationId(), cancelPending ? 'cancelCleanup' : 'cleanup', value.planToken);
      if (result.ok) await consume(result.value); else if (alive.current) { setError(result.error); setCleanupFailure(true); }
    });
  }
  function closeConfirmation() {
    if (busy) return;
    dialog.current?.close(); setConfirming(null);
    if (opener.current instanceof HTMLElement && opener.current.isConnected) opener.current.focus();
  }
  async function choose() {
    await action(async () => {
      const result = await service.chooseDestination(t('backup.choose'));
      if (!alive.current) return;
      if (!result.ok) { setError(result.error); return; }
      if (result.value) { setCleanupResult(null); setRetentionCount(String(result.value.retainedSnapshots)); setRetentionPlan(null); setSelection(result.value); setRememberedPath(result.value.destination); setNextDailyCheck(null); setPreview(null); setOverwriteView(null); setSelected(null); setListLoaded(false); setEntries([]); setRestored(null); setJob(null); }
    });
  }
  async function saveRetention() {
    const count = Number(retentionCount);
    if (!selection || !Number.isInteger(count) || count < 1 || count > 4294967295) return;
    await action(async () => {
      const result = await service.setRetention(selection.selectionId, selection.preferencesRevision, count);
      if (!alive.current) return;
      if (result.ok) { setSelection(result.value); setRetentionCount(String(result.value.retainedSnapshots)); setRetentionPlan(null); setJob(null); }
      else setError(result.error);
    });
  }
  async function start(kind: 'list' | 'create' | 'retention') {
    if (!selection) return;
    setOverwriteView(null);
    await action(async () => {
      const result = await service.start(selection.selectionId, newOperationId(), kind);
      if (result.ok) await consume(result.value); else if (alive.current) setError(result.error);
    });
  }
  async function previewRestore() {
    if (!selection || !selected || composing) return;
    setPreview(null); setOverwriteView(null); setRestored(null);
    await action(async () => {
      const result = await service.preview(selection.selectionId, newOperationId(), selected.snapshotId, name, t('backup.chooseParent'));
      if (!result.ok) { if (alive.current) setError(result.error); }
      else if (result.value) await consume(result.value);
    });
  }
  async function restore() {
    if (!confirming || !selection || composing) return;
    const token = confirming.planToken;
    dialog.current?.close(); setConfirming(null);
    await action(async () => {
      const result = await service.start(selection.selectionId, newOperationId(), 'restore', token);
      if (result.ok) await consume(result.value); else if (alive.current) setError(result.error);
    });
  }
  async function cancelTask() {
    if (!job || job.status !== 'running') return;
    const result = await service.cancel(job.jobId);
    if (!alive.current) return;
    if (result.ok) setJob(result.value); else setError(result.error);
  }
  const disabled = busy || !canStart || composing;
  const running = job?.status === 'running';
  const counting = running && ['copying', 'restoring', 'cleaning'].includes(job.progress.phase) && job.progress.totalObjects > 0;
  const errorText = overwriteFailure === 'reopen' ? t('backup.overwriteReopen') : overwriteFailure === 'operation' ? t('backup.overwriteError') : upgradeFailure === 'reopen' ? t('backup.upgradeReopen') : upgradeFailure === 'operation' ? t('backup.upgradeError') : error ? cleanupFailure ? t('backup.cleanupError') : dailyFailure ? t('backup.dailyError') : ['IO_DENIED', 'VERSION_CONFLICT', 'INVALID_INPUT', 'HISTORY_MISSING'].includes(error.code) ? t('backup.error') : serviceErrorMessage(locale, error.code) : null;
  return <section className="backup-screen" aria-labelledby="backup-heading">
    <header className="draft-heading"><div><h2 id="backup-heading" ref={heading} tabIndex={-1}>{t('backup.title')}</h2><p>{t('backup.body')}</p></div><button type="button" className="secondary" disabled={busy} onClick={onClose}>{t('home.explain.close')}</button></header>
    {errorText ? <div role="status" className="notice-panel"><p>{errorText}</p>{!busy ? <button type="button" className="secondary" onClick={() => void context()}>{t('workspace.error.retry')}</button> : null}</div> : null}
    {notice ? <p role="status" className="notice-panel">{t(`backup.${notice}`)}</p> : null}
    {nextDailyCheck ? <p>{t('backup.dailyNext', { date: formatInstant(nextDailyCheck * 1000, regional) })}</p> : null}
    {restored ? <p role="status" className="notice-panel">{t('backup.restored', { path: pathLabel(restored.target) })}</p> : null}
    {busy ? <section className="notice-panel" aria-live="polite"><p>{running && ['verifying', 'finalizing', 'publishing'].includes(job.progress.phase) ? t('backup.verifying') : t('trash.working')}</p><progress {...(counting ? { max: job.progress.totalObjects, value: job.progress.completedObjects } : {})} aria-label={t('backup.title')} />{counting ? <p>{t('backup.progress', { current: job.progress.completedObjects, total: job.progress.totalObjects })}</p> : null}{running ? <button type="button" className="secondary" disabled={job.cancelRequested} onClick={() => void cancelTask()}>{t(job.cancelRequested ? 'backup.cancelling' : 'backup.cancelTask')}</button> : null}</section> : null}
    {!canStart && !busy ? <p role="status">{t('backup.unsaved')}</p> : null}
    <section className="backup-card"><h3>{t('backup.destination')}</h3><div className="backup-location"><input aria-label={t('backup.destination')} readOnly value={selection ? pathLabel(selection.destination) : rememberedPath ? pathLabel(rememberedPath) : ''} /><button type="button" className="secondary" disabled={disabled} onClick={() => void choose()}>{t('backup.choose')}</button></div><p>{t('backup.outside')}</p>{selection ? <p>{selection.availableBytes !== null && selection.checkedAt !== null ? t('backup.space', { bytes: byteLabel(selection.availableBytes, regional.region), date: formatInstant(selection.checkedAt, regional) }) : t('backup.spacePending')}</p> : null}<p className="notice-panel">{t('backup.media')}</p><p>{t('backup.session')}</p><div className="panel-actions"><button type="button" className="primary" disabled={disabled || !selection} onClick={() => void start('create')}>{t('backup.create')}</button><button type="button" className="secondary" disabled={disabled || !selection} onClick={() => void start('list')}>{t('backup.refresh')}</button></div></section>
    {selection ? <section className="backup-card">
      <label htmlFor="backup-retention-count">{t('backup.retentionCount')}</label>
      <input id="backup-retention-count" type="number" min="1" max="4294967295" step="1" value={retentionCount} disabled={disabled} onChange={event => { setRetentionCount(event.target.value); setRetentionPlan(null); }} onCompositionStart={() => setComposing(true)} onCompositionEnd={() => setComposing(false)} />
      <div className="panel-actions">
        <button type="button" className="secondary" disabled={disabled || !Number.isInteger(Number(retentionCount)) || Number(retentionCount) < 1 || Number(retentionCount) > 4294967295 || Number(retentionCount) === selection.retainedSnapshots} onClick={() => void saveRetention()}>{t('backup.saveRetention')}</button>
        <button type="button" className="secondary" disabled={disabled || Number(retentionCount) !== selection.retainedSnapshots} onClick={() => void start('retention')}>{t('backup.previewCleanup')}</button>
      </div>
      <p>{t('backup.cleanupPending')}</p>
      {retentionPlan ? [retentionPlan.current, ...retentionPlan.pending].map(value => <section className="notice-panel" key={value.planToken}>
        {value.operationId ? <p>{t('backup.cleanupPendingJob', { count: value.retainedSnapshots })}</p> : null}
        <p>{t('backup.cleanupCounts', { kept: value.retained.length, count: value.candidates.length, preserved: value.preserved.length })}</p>
        <p>{t('backup.files', { count: value.candidates.reduce((sum, snapshot) => sum + snapshot.objectCount, 0), bytes: byteLabel(value.candidateBytes, regional.region) })}</p>
        <ul>{value.candidates.map(snapshot => <li key={snapshot.snapshotId}>{formatInstant(snapshot.createdAt, regional)} · {byteLabel(snapshot.totalBytes, regional.region)} · <code>{snapshot.manifestHash}</code></li>)}</ul>
        <div className="panel-actions">
          <button type="button" className="secondary" disabled={disabled || Number(retentionCount) !== selection.retainedSnapshots || value.retainedSnapshots !== selection.retainedSnapshots || !value.candidates.length || (!value.operationId && !!retentionPlan.pending.length)} onClick={() => { opener.current = document.activeElement; setCleanupConfirm(value); }}>{t(value.operationId ? 'backup.cleanupResume' : 'backup.cleanupAction')}</button>
          {value.operationId ? <button type="button" className="secondary" disabled={disabled} onClick={() => void runCleanup(value, true)}>{t('backup.cleanupCancel')}</button> : null}
        </div>
      </section>) : null}
      {cleanupResult ? <p role="status">{t('backup.cleanupOutcome', { removed: cleanupResult.removed.length, remaining: cleanupResult.remaining.length, quarantined: cleanupResult.quarantined.length })}</p> : null}
    </section> : null}
    {selection && canUpgrade ? <section className="backup-card">
      <h3>{t('backup.upgradeTitle')}</h3><p>{t('backup.upgradeWarning')}</p>
      <div className="panel-actions">
        <button type="button" className="secondary" disabled={disabled || Number(retentionCount) !== selection.retainedSnapshots} onClick={() => void inspectUpgrade()}>{t('backup.upgradeFind')}</button>
        <button type="button" className="secondary" disabled={disabled || !selected || ![1, 2].includes(selected.schemaVersion) || Number(retentionCount) !== selection.retainedSnapshots || !!upgradeViews?.pending.length} onClick={() => void inspectUpgrade(selected?.snapshotId)}>{t('backup.upgradeInspect')}</button>
      </div>
      {upgradeViews && !upgradeViews.current && !upgradeViews.pending.length ? <p>{t('backup.upgradeNone')}</p> : null}
      {upgradeViews ? [...(upgradeViews.current ? [upgradeViews.current] : []), ...upgradeViews.pending].map(value => <section className="notice-panel" key={value.planToken}>
        {value.operationId ? <p>{t('backup.upgradePending', { operation: value.operationId })}</p> : null}
        <p className="backup-path">{pathLabel(value.source)}</p>
        <p>{t('backup.upgradeVersions', { from: value.fromSchema, to: value.toSchema, count: value.noteCount })}</p>
        <p>{formatInstant(value.snapshot.createdAt, regional)} · {byteLabel(value.snapshot.totalBytes, regional.region)}</p>
        <div className="panel-actions">
          <button type="button" className="primary" disabled={disabled || Number(retentionCount) !== selection.retainedSnapshots} onClick={() => { opener.current = document.activeElement; setUpgradeConfirm(value); }}>{t(value.operationId ? 'backup.upgradeResume' : 'backup.upgradeApply')}</button>
          {value.operationId ? <button type="button" className="secondary" disabled={disabled || Number(retentionCount) !== selection.retainedSnapshots} onClick={() => { opener.current = document.activeElement; setUpgradeAbandon(value); }}>{t('backup.upgradeAbandon')}</button> : null}
        </div>
      </section>) : null}
    </section> : null}
    {preview ? <ReferenceIssues report={preview.referenceReport} t={t} /> : null}
    {preview !== null && preview.snapshot.schemaVersion !== 3 ? <p className="notice-panel">{t('backup.legacyReadOnly')}</p> : null}
    <section className="backup-card">
      <h3>{t('backup.overwriteAction')}</h3>
      <p>{t('backup.overwriteWarning')}</p>
      {selected !== null && selected.schemaVersion !== 3 ? <p className="notice-panel">{t('backup.legacyReadOnly')}</p> : null}
      <div className="panel-actions"><button type="button" className="secondary" disabled={disabled || !selected || selected.schemaVersion !== 3 || !canOverwrite} onClick={() => void inspectOverwrite()}>{t('backup.overwriteAction')}</button></div>
      {overwriteView ? <section className="notice-panel">
        <h4>{t('backup.overwriteConfirm')}</h4>
        <p className="backup-path">{pathLabel(overwriteView.target)}</p>
        <p>{t('backup.notes', { count: overwriteView.noteCount })}</p>
        <p>{t('backup.files', { count: overwriteView.snapshot.objectCount, bytes: byteLabel(overwriteView.snapshot.totalBytes, regional.region) })}</p>
        <code className="backup-hash">{overwriteView.snapshot.manifestHash}</code>
        <p>{t('backup.overwriteWarning')}</p>
        <p className="backup-path">{pathLabel(overwriteView.previousRoot)}</p>
        <ReferenceIssues report={overwriteView.referenceReport} t={t} />
        <div className="panel-actions"><button type="button" className="primary" disabled={disabled || !canOverwrite || overwriteView.snapshot.schemaVersion !== 3} onClick={() => { opener.current = document.activeElement; setOverwriteConfirm(overwriteView); }}>{t('backup.overwriteAction')}</button></div>
      </section> : null}
    </section>
    <div className="backup-layout"><aside className="backup-card" aria-label={t('backup.refresh')}>
      {!entries.length ? <p>{t(listLoaded ? 'backup.empty' : 'backup.notLoaded')}</p> : null}<ul className="note-list">{entries.map(snapshot => <li key={snapshot.snapshotId}><button type="button" className="note-item" disabled={disabled} aria-pressed={selected?.snapshotId === snapshot.snapshotId} onClick={() => { setSelected(snapshot); setPreview(null); setRestored(null); }}><strong>{formatInstant(snapshot.createdAt, regional)}</strong><span>{t('backup.files', { count: snapshot.objectCount, bytes: byteLabel(snapshot.totalBytes, regional.region) })}</span></button></li>)}</ul>
    </aside><section className="backup-card">{selected ? <><h3>{t('backup.previewTitle')}</h3><p>{t('backup.verified')}</p><details><summary>SHA-256</summary><code className="backup-hash">{selected.manifestHash}</code></details><p>{t('backup.previewBody')}</p><label htmlFor="backup-folder-name">{t('backup.newName')}</label><input id="backup-folder-name" value={name} disabled={busy} onChange={event => { setName(event.target.value); setPreview(null); }} onCompositionStart={() => setComposing(true)} onCompositionEnd={() => setComposing(false)} /><div className="panel-actions"><button type="button" className="secondary" disabled={disabled || !name} onClick={() => void previewRestore()}>{t('backup.preview')}</button></div>
      {preview ? <section className="notice-panel"><h4>{t('backup.target')}</h4><p className="backup-path">{pathLabel(preview.target)}</p><p>{t('backup.notes', { count: preview.noteCount })}</p><p>{t('backup.files', { count: preview.snapshot.objectCount, bytes: byteLabel(preview.snapshot.totalBytes, regional.region) })}</p><button type="button" className="primary" disabled={disabled} onClick={() => { opener.current = document.activeElement; setConfirming(preview); }}>{t('backup.restore')}</button></section> : null}</> : <p>{t('backup.previewBody')}</p>}</section></div>
    {upgradeConfirm ? <dialog className="trash-confirm" ref={upgradeDialog} aria-labelledby="upgrade-confirm-heading" onCancel={event => { event.preventDefault(); closeUpgradeConfirmation(); }}>
      <h2 id="upgrade-confirm-heading">{t('backup.upgradeConfirm')}</h2>
      <p className="backup-path">{pathLabel(upgradeConfirm.source)}</p>
      <p>{t('backup.upgradeVersions', { from: upgradeConfirm.fromSchema, to: upgradeConfirm.toSchema, count: upgradeConfirm.noteCount })}</p>
      <p>{t('backup.destination')}: {selection ? pathLabel(selection.destination) : ''}</p>
      <p>{formatInstant(upgradeConfirm.snapshot.createdAt, regional)} · {byteLabel(upgradeConfirm.snapshot.totalBytes, regional.region)}</p>
      <code className="backup-hash">{upgradeConfirm.snapshot.manifestHash}</code>
      <p>{t('backup.upgradeWarning')}</p>
      <div className="panel-actions"><button type="button" className="secondary" ref={upgradeCancel} disabled={busy} onClick={closeUpgradeConfirmation}>{t('workspace.create.cancel')}</button><button type="button" className="primary" disabled={disabled} onClick={() => void upgrade()}>{t(upgradeConfirm.operationId ? 'backup.upgradeResume' : 'backup.upgradeApply')}</button></div>
    </dialog> : null}
    {upgradeAbandon ? <dialog className="trash-confirm" ref={abandonDialog} aria-labelledby="upgrade-abandon-heading" onCancel={event => { event.preventDefault(); closeAbandonConfirmation(); }}>
      <h2 id="upgrade-abandon-heading">{t('backup.upgradeAbandonConfirm')}</h2>
      <p className="backup-path">{pathLabel(upgradeAbandon.source)}</p>
      <p>{t('backup.upgradeAbandonWarning')}</p>
      <code className="backup-hash">{upgradeAbandon.snapshot.manifestHash}</code>
      <div className="panel-actions">
        <button type="button" className="secondary" ref={abandonCancel} disabled={busy} onClick={closeAbandonConfirmation}>{t('workspace.create.cancel')}</button>
        <button type="button" className="primary" disabled={disabled || Number(retentionCount) !== selection?.retainedSnapshots} onClick={() => void abandonUpgrade()}>{t('backup.upgradeAbandon')}</button>
      </div>
    </dialog> : null}
    {overwriteConfirm ? <dialog className="trash-confirm backup-overwrite-confirm" ref={overwriteDialog} aria-labelledby="overwrite-confirm-heading" onCancel={event => { event.preventDefault(); closeOverwriteConfirmation(); }}>
      <h2 id="overwrite-confirm-heading">{t('backup.overwriteConfirm')}</h2>
      <p className="backup-path">{pathLabel(overwriteConfirm.target)}</p>
      <p>{t('backup.notes', { count: overwriteConfirm.noteCount })}</p>
      <code className="backup-hash">{overwriteConfirm.snapshot.manifestHash}</code>
      <p>{t('backup.overwriteWarning')}</p>
      <p className="backup-path">{pathLabel(overwriteConfirm.previousRoot)}</p>
      <ReferenceIssues report={overwriteConfirm.referenceReport} t={t} />
      <label className="backup-acknowledge"><input type="checkbox" checked={overwriteAcknowledged} disabled={busy} onChange={event => setOverwriteAcknowledged(event.target.checked)} /> {t('backup.overwriteAcknowledge')}</label>
      <label htmlFor="backup-overwrite-phrase">{t('backup.overwritePhrase')}</label>
      <input id="backup-overwrite-phrase" value={overwritePhrase} disabled={busy} autoComplete="off" spellCheck={false} onChange={event => setOverwritePhrase(event.target.value)} onCompositionStart={() => setComposing(true)} onCompositionEnd={() => setComposing(false)} />
      <div className="panel-actions">
        <button type="button" className="secondary" ref={overwriteCancel} disabled={busy} onClick={closeOverwriteConfirmation}>{t('workspace.create.cancel')}</button>
        <button type="button" className="primary" disabled={disabled || !canOverwrite || !overwriteAcknowledged || overwritePhrase !== t('backup.overwritePhrase')} onClick={() => void runOverwrite()}>{t('backup.overwriteAction')}</button>
      </div>
    </dialog> : null}
    {cleanupConfirm ? <dialog className="trash-confirm" ref={cleanupDialog} aria-labelledby="cleanup-confirm-heading" onCancel={event => { event.preventDefault(); closeCleanupConfirmation(); }}>
      <h2 id="cleanup-confirm-heading">{t('backup.cleanupConfirm')}</h2>
      <p className="backup-path">{selection ? pathLabel(selection.destination) : ''}</p>
      <p>{t('backup.cleanupWarning')}</p>
      <p>{t('backup.cleanupCounts', { kept: cleanupConfirm.retained.length, count: cleanupConfirm.candidates.length, preserved: cleanupConfirm.preserved.length })}</p>
      <ul>{cleanupConfirm.candidates.map(snapshot => <li key={snapshot.snapshotId}>{formatInstant(snapshot.createdAt, regional)} · {byteLabel(snapshot.totalBytes, regional.region)}</li>)}</ul>
      <div className="panel-actions">
        <button type="button" className="secondary" ref={cleanupCancel} onClick={closeCleanupConfirmation}>{t('workspace.create.cancel')}</button>
        <button type="button" className="primary" disabled={disabled} onClick={() => void runCleanup(cleanupConfirm)}>{t(cleanupConfirm.operationId ? 'backup.cleanupResume' : 'backup.cleanupAction')}</button>
      </div>
    </dialog> : null}
    {confirming ? <dialog className="trash-confirm" ref={dialog} aria-labelledby="backup-confirm-heading" onCancel={event => { event.preventDefault(); closeConfirmation(); }}>
      <h2 id="backup-confirm-heading">{t('backup.confirm')}</h2>
      <p className="backup-path">{pathLabel(confirming.target)}</p>
      <p>{t('backup.previewBody')}</p>
      <p>{t('backup.notes', { count: confirming.noteCount })}</p>
      {confirming.snapshot.schemaVersion !== 3 ? <p className="notice-panel">{t('backup.legacyReadOnly')}</p> : null}
      <div className="panel-actions">
        <button type="button" className="secondary" ref={cancel} onClick={closeConfirmation}>{t('workspace.create.cancel')}</button>
        <button type="button" className="primary" disabled={busy} onClick={() => void restore()}>{t('backup.restore')}</button>
      </div>
    </dialog> : null}
  </section>;
}
