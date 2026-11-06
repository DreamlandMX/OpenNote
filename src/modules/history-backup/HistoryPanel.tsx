import { formatInstant, formatNumber, type RegionalPreferences } from '../platform-shell/regional';
import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type {
  HistoryCleanupPreview,
  HistoryCleanupJob,
  HistoryListing,
  HistoryRetentionPlan,
  HistoryRetentionSettings,
  HistoryRevisionContent,
  NoteDocument,
} from '../domain';
import type { NoteService, ServiceOutcome } from '../editor';
import { sameDiskVersion } from '../editor/draft-recovery';
import { serviceErrorMessage, type SupportedLocaleId, type Translator } from '../presentation-i18n';
import { newOperationId } from '../platform-shell/tauri-note-service';
import { ReferenceIssues } from './ReferenceIssues';
import { monitorHistoryCleanup } from './history-cleanup-task';
import { revisionFocusTarget } from './history-navigation';

interface Props {
  writable: boolean;
  noteId: string;
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  regional: RegionalPreferences;
  onRestore(historical: NoteDocument, current: NoteDocument, revisionId: string): Promise<boolean>;
  onClose(): void;
}

interface CleanupTarget {
  plan: HistoryRetentionPlan;
  operationId: string;
  resuming: boolean;
}

const BYTE_UNITS = ['byte', 'kilobyte', 'megabyte', 'gigabyte', 'terabyte'] as const;

function formatBytes(bytes: number, locale: string): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return formatNumber(value, locale, {
    style: 'unit',
    unit: BYTE_UNITS[unit],
    maximumFractionDigits: unit === 0 ? 0 : 2,
  });
}

export function HistoryPanel({ writable, noteId, service, translator: t, locale, regional, onRestore, onClose }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const cleanupDialog = useRef<HTMLDialogElement>(null);
  const alive = useRef(true);
  const session = useRef(0);
  const committing = useRef(false);
  const [listing, setListing] = useState<HistoryListing>({ items: [], unavailableRevisionIds: [] });
  const [selected, setSelected] = useState<{
    revisionId: string;
    historical: HistoryRevisionContent;
    current: ServiceOutcome<NoteDocument>;
  } | null>(null);
  const [retention, setRetention] = useState<HistoryRetentionSettings | null>(null);
  const [retentionDays, setRetentionDays] = useState('');
  const [cleanupPreview, setCleanupPreview] = useState<HistoryCleanupPreview | null>(null);
  const [cleanupTarget, setCleanupTarget] = useState<CleanupTarget | null>(null);
  const [busy, setBusy] = useState(true);
  const [maintenanceBusy, setMaintenanceBusy] = useState(true);
  const [cleanupBusy, setCleanupBusy] = useState(false);
  const [cleanupJob, setCleanupJob] = useState<HistoryCleanupJob | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [notice, setNotice] = useState('');
  const [cleanupNotice, setCleanupNotice] = useState('');

  async function loadList(clearNotice = true) {
    const token = ++session.current;
    setBusy(true);
    const result = await service.listHistory(noteId);
    if (!alive.current || token !== session.current) return;
    setBusy(false);
    if (result.ok) {
      setListing(result.value);
      if (clearNotice) setNotice('');
    } else {
      setNotice(serviceErrorMessage(locale, result.error.code));
    }
  }

  async function loadMaintenance(announceEmpty: boolean) {
    if (!writable) { setMaintenanceBusy(false); return; }
    setMaintenanceBusy(true);
    const [settingsResult, previewResult] = await Promise.all([
      service.historyRetentionSettings(),
      service.previewHistoryCleanup(),
    ]);
    if (!alive.current) return;
    if (settingsResult.ok) {
      setRetention(settingsResult.value);
      setRetentionDays(String(settingsResult.value.retentionDays));
    } else {
      setNotice(serviceErrorMessage(locale, settingsResult.error.code));
    }
    if (previewResult.ok) {
      setCleanupPreview(previewResult.value);
      if (
        announceEmpty &&
        previewResult.value.current.candidates.length === 0 &&
        previewResult.value.pending.length === 0
      ) {
        setNotice(t('history.cleanupNone'));
      }
    } else {
      setNotice(serviceErrorMessage(locale, previewResult.error.code));
    }
    setMaintenanceBusy(false);
  }

  useEffect(() => {
    alive.current = true;
    const opener = document.activeElement;
    dialog.current?.showModal();
    void loadList(false);
    void loadMaintenance(false);
    return () => {
      alive.current = false;
      session.current++;
      cleanupDialog.current?.close();
      dialog.current?.close();
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, []);

  useEffect(() => {
    const confirmation = cleanupDialog.current;
    if (!confirmation) return;
    if (cleanupTarget && !confirmation.open) confirmation.showModal();
    if (!cleanupTarget && confirmation.open) confirmation.close();
  }, [cleanupTarget]);

  async function select(revisionId: string) {
    const token = ++session.current;
    setBusy(true);
    setSelected(null);
    setConfirming(false);
    setNotice('');
    const [historical, current] = await Promise.all([
      service.readHistory(noteId, revisionId),
      service.readNote(noteId),
    ]);
    if (!alive.current || token !== session.current) return;
    setBusy(false);
    if (historical.ok) {
      setSelected({ revisionId, historical: historical.value, current });
    } else {
      setNotice(serviceErrorMessage(locale, historical.error.code));
    }
  }

  async function restore() {
    if (!selected || !selected.current.ok || busy || committing.current) return;
    const token = ++session.current;
    setBusy(true);
    const current = await service.readNote(noteId);
    if (!alive.current || token !== session.current) return;
    if (!sameDiskVersion(selected.current, current)) {
      setBusy(false);
      setConfirming(false);
      setSelected({ ...selected, current });
      setNotice(
        current.ok ? t('editor.draft.changed') : serviceErrorMessage(locale, current.error.code),
      );
      return;
    }
    committing.current = true;
    const accepted = await onRestore(
      selected.historical.document,
      selected.current.value,
      selected.revisionId,
    );
    committing.current = false;
    if (!alive.current || token !== session.current) return;
    setBusy(false);
    if (accepted) {
      onClose();
    } else {
      setConfirming(false);
      setNotice(t('history.blocked'));
    }
  }

  async function exportVersion() {
    if (!selected || busy) return;
    const token = ++session.current;
    setBusy(true);
    const result = await service.exportDraft(selected.historical.document);
    if (!alive.current || token !== session.current) return;
    setBusy(false);
    setNotice(
      result.ok
        ? t(
            result.value.outcome === 'saved'
              ? 'editor.export.saved'
              : 'editor.export.cancelled',
          )
        : serviceErrorMessage(locale, result.error.code),
    );
  }

  async function saveRetention() {
    if (!writable || !retention || maintenanceBusy || cleanupBusy) return;
    const days = Number(retentionDays);
    if (!Number.isSafeInteger(days) || days < 1 || days > 0xffff_ffff) {
      setNotice(serviceErrorMessage(locale, 'INVALID_INPUT'));
      return;
    }
    setMaintenanceBusy(true);
    const result = await service.setHistoryRetention(days, retention.revision);
    if (!alive.current) return;
    if (result.ok) {
      setRetention(result.value);
      setRetentionDays(String(result.value.retentionDays));
      setCleanupPreview(null);
      setNotice(t('history.retentionSaved'));
    } else {
      setNotice(serviceErrorMessage(locale, result.error.code));
      if (result.error.code === 'VERSION_CONFLICT') {
        const latest = await service.historyRetentionSettings();
        if (alive.current && latest.ok) {
          setRetention(latest.value);
          setRetentionDays(String(latest.value.retentionDays));
        }
      }
    }
    setMaintenanceBusy(false);
  }

  function openCleanup(plan: HistoryRetentionPlan, operationId: string, resuming: boolean) {
    setCleanupNotice('');
    setCleanupTarget({ plan, operationId, resuming });
  }

  function closeCleanup() {
    if (cleanupBusy) return;
    setCleanupTarget(null);
    setCleanupNotice('');
  }

  async function applyCleanup() {
    if (!writable || !cleanupTarget || cleanupBusy) return;
    await runCleanup(() => service.applyHistoryCleanup(cleanupTarget.plan, cleanupTarget.operationId));
  }

  async function cancelActiveCleanup() {
    if (!cleanupJob || !cleanupBusy) return;
    const result = await service.cancelHistoryCleanup(cleanupJob.jobId);
    if (!alive.current) return;
    if (result.ok) setCleanupJob(result.value);
    else setCleanupNotice(serviceErrorMessage(locale, result.error.code));
  }

  async function runCleanup(start: () => Promise<ServiceOutcome<HistoryCleanupJob>>) {
    if (!writable || cleanupBusy) return;
    setCleanupBusy(true);
    setCleanupJob(null);
    setCleanupNotice('');
    let result = await start();
    if (result.ok) result = await monitorHistoryCleanup(service, result.value,
      job => { if (alive.current) setCleanupJob(job); },
      error => { if (alive.current) setCleanupNotice(serviceErrorMessage(locale, error.code)); });
    if (!alive.current) return;
    setCleanupBusy(false);
    if (result.ok && result.value.result) {
      const finished = result.value.result;
      setCleanupTarget(null);
      setCleanupPreview(null);
      setSelected(null);
      setConfirming(false);
      setNotice(finished.status === 'cancelled'
        ? t('history.cleanupCancelled', { count: finished.removed.length, remaining: finished.remaining?.length ?? 0 })
        : t('history.cleanupComplete', { count: finished.removed.length }));
      void loadList(false);
      void loadMaintenance(false);
    } else {
      const refreshed = await service.previewHistoryCleanup();
      if (!alive.current) return;
      if (refreshed.ok) setCleanupPreview(refreshed.value);
      setCleanupNotice(serviceErrorMessage(locale, result.ok ? result.value.error?.code ?? 'IO_DENIED' : result.error.code));
    }
  }

  function moveRevisionFocus(event: KeyboardEvent<HTMLUListElement>) {
    const buttons = Array.from(
      event.currentTarget.querySelectorAll<HTMLButtonElement>('button[data-revision-id]'),
    );
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const target = revisionFocusTarget(event.key, current, buttons.length);
    if (target === null) return;
    event.preventDefault();
    buttons[target]?.focus();
  }

  return (
    <>
      <dialog
        ref={dialog}
        className="draft-dialog"
        aria-labelledby="history-heading"
        onCancel={(event) => {
          event.preventDefault();
          if (!committing.current && !cleanupTarget && !cleanupBusy) onClose();
        }}
      >
        <header className="draft-heading">
          <div>
            <h2 id="history-heading">{t('history.title')}</h2>
            <p>{t('history.confirmBody')}</p>
          </div>
          <button
            type="button"
            className="secondary"
            disabled={committing.current || cleanupBusy}
            onClick={() => {
              if (!committing.current && !cleanupTarget && !cleanupBusy) onClose();
            }}
          >
            {t('home.explain.close')}
          </button>
        </header>
        <div className="draft-layout">
          <aside aria-label={t('history.title')}>
            {busy ? <p role="status">{t('workspace.loading')}</p> : null}
            {!busy && listing.items.length === 0 ? <p>{t('history.empty')}</p> : null}
            <ul className="note-list" onKeyDown={moveRevisionFocus}>
              {listing.items.map((revision, index) => (
                <li key={revision.revisionId}>
                  <button
                    type="button"
                    className="note-item"
                    data-revision-id={revision.revisionId}
                    tabIndex={
                      selected
                        ? selected.revisionId === revision.revisionId
                          ? 0
                          : -1
                        : index === 0
                          ? 0
                          : -1
                    }
                    disabled={busy}
                    aria-pressed={selected?.revisionId === revision.revisionId}
                    onClick={() => void select(revision.revisionId)}
                  >
                    <span>r{revision.localRevision}</span>
                    <time dateTime={revision.savedAt}>
                      {formatInstant(revision.savedAt, regional)}
                    </time>
                  </button>
                </li>
              ))}
            </ul>
            {listing.unavailableRevisionIds.length > 0 ? (
              <p role="alert">
                {t('history.unavailable', { count: listing.unavailableRevisionIds.length })}
              </p>
            ) : null}
            <button type="button" className="secondary" disabled={busy} onClick={() => void loadList()}>
              {t('workspace.error.retry')}
            </button>
          </aside>
          <section className="draft-detail" aria-busy={busy}>
            <section className="notice-panel history-retention" aria-labelledby="retention-heading">
              <h2 id="retention-heading">{t('history.retentionTitle')}</h2>
              {!writable ? <p role="status">{t('history.maintenanceReadOnly')}</p> : null}
              <label>
                <span>{t('history.retentionDays')}</span>
                <input
                  type="number"
                  min="1"
                  step="1"
                  inputMode="numeric"
                  value={retentionDays}
                  disabled={!writable || maintenanceBusy || cleanupBusy || !retention}
                  onChange={(event) => setRetentionDays(event.target.value)}
                />
              </label>
              <div className="panel-actions">
                <button
                  type="button"
                  className="secondary"
                  disabled={!writable || maintenanceBusy || cleanupBusy || !retention}
                  onClick={() => void saveRetention()}
                >
                  {t('history.saveRetention')}
                </button>
                <button
                  type="button"
                  className="secondary"
                  disabled={!writable || maintenanceBusy || cleanupBusy}
                  onClick={() => void loadMaintenance(true)}
                >
                  {t('history.previewCleanup')}
                </button>
              </div>
              {cleanupPreview ? (
                <div className="history-retention-summary">
                  <p>
                    {t('history.cleanupCutoff', {
                      date: formatInstant(cleanupPreview.current.cutoff, regional),
                    })}
                  </p>
                  <p>{t('history.cleanupKept', { count: cleanupPreview.current.retainedCount })}</p>
                  {cleanupPreview.current.candidates.length > 0 ? (
                    <p>
                      {t('history.cleanupBody', {
                        count: cleanupPreview.current.candidates.length,
                        bytes: formatBytes(cleanupPreview.current.candidateBytes, regional.region),
                      })}
                    </p>
                  ) : null}
                  {cleanupPreview.current.preserved.length > 0 ? (
                    <p>
                      {t('history.cleanupPreserved', {
                        count: cleanupPreview.current.preserved.length,
                      })}
                    </p>
                  ) : null}
                  {cleanupPreview.pending.length > 0 ? (
                    <>
                      <p>{t('history.cleanupPending', { count: cleanupPreview.pending.length })}</p>
                      <ul className="history-cleanup-pending">
                        {cleanupPreview.pending.map((pending) => (
                          <li key={pending.operationId}>
                            <span>
                              {formatInstant(pending.plan.generatedAt, regional)} ·{' '}
                              <code>{pending.operationId.slice(0, 8)}</code>
                            </span>
                            <button
                              type="button"
                              className="secondary"
                              disabled={!writable || maintenanceBusy || cleanupBusy}
                              onClick={() =>
                                openCleanup(pending.plan, pending.operationId, true)
                              }
                            >
                              {t('history.cleanupResume')}
                            </button>
                            <button type="button" className="secondary" disabled={!writable || maintenanceBusy || cleanupBusy}
                              onClick={() => {
                                setCleanupTarget({ plan: pending.plan, operationId: pending.operationId, resuming: true });
                                void runCleanup(() => service.stopHistoryCleanup(pending.operationId));
                              }}>
                              {t('history.cleanupStop')}
                            </button>
                          </li>
                        ))}
                      </ul>
                    </>
                  ) : null}
                  {cleanupPreview.current.candidates.length > 0 ? (
                    <div className="panel-actions">
                      <button
                        type="button"
                        className="danger"
                        disabled={!writable || maintenanceBusy || cleanupBusy}
                        onClick={() =>
                          openCleanup(cleanupPreview.current, newOperationId(), false)
                        }
                      >
                        {t('history.cleanupReview')}
                      </button>
                    </div>
                  ) : null}
                </div>
              ) : null}
            </section>
            {notice ? (
              <p role="status" className="notice-panel">
                {notice}
              </p>
            ) : null}
            {selected ? (
              <>
                <div className="draft-compare">
                  <section>
                    <h3>
                      {t('history.title')} · r{selected.historical.document.localRevision}
                    </h3>
                    <pre tabIndex={0}>{selected.historical.document.source}</pre>
                  </section>
                  <section>
                    <h3>
                      {t('editor.conflict.diskHeading')}
                      {selected.current.ok ? ` · r${selected.current.value.localRevision}` : ''}
                    </h3>
                    {selected.current.ok ? (
                      <pre tabIndex={0}>{selected.current.value.source}</pre>
                    ) : (
                      <p role="alert">
                        {serviceErrorMessage(locale, selected.current.error.code)}
                      </p>
                    )}
                  </section>
                </div>
                <ReferenceIssues report={selected.historical.referenceReport} t={t} />
                {confirming ? (
                  <div className="notice-panel">
                    <p>{t('history.confirmBody')}</p>
                    <div className="panel-actions">
                      <button
                        type="button"
                        className="primary"
                        disabled={
                          busy || !selected.current.ok || selected.current.value.readOnly
                        }
                        onClick={() => void restore()}
                      >
                        {t('history.confirm')}
                      </button>
                      <button
                        type="button"
                        className="secondary"
                        disabled={busy}
                        onClick={() => setConfirming(false)}
                      >
                        {t('workspace.create.cancel')}
                      </button>
                    </div>
                  </div>
                ) : (
                  <button
                    type="button"
                    className="primary"
                    disabled={busy || !selected.current.ok || selected.current.value.readOnly}
                    onClick={() => setConfirming(true)}
                  >
                    {t('history.restore')}
                  </button>
                )}
                <button
                  type="button"
                  className="secondary"
                  disabled={busy}
                  onClick={() => void exportVersion()}
                >
                  {t('editor.save.failed.export')}
                </button>
              </>
            ) : null}
          </section>
        </div>
      </dialog>
      <dialog
        ref={cleanupDialog}
        className="trash-confirm history-cleanup-confirm"
        aria-labelledby="history-cleanup-heading"
        onCancel={(event) => {
          event.preventDefault();
          if (cleanupBusy) void cancelActiveCleanup();
          else closeCleanup();
        }}
      >
        {cleanupTarget ? (
          <>
            <h2 id="history-cleanup-heading">{t('history.cleanupHeading')}</h2>
            <p>
              {t('history.cleanupBody', {
                count: cleanupTarget.plan.candidates.length,
                bytes: formatBytes(cleanupTarget.plan.candidateBytes, regional.region),
              })}
            </p>
            <p>
              {t('history.cleanupCutoff', {
                date: formatInstant(cleanupTarget.plan.cutoff, regional),
              })}
            </p>
            <p>{t('history.cleanupKept', { count: cleanupTarget.plan.retainedCount })}</p>
            {cleanupTarget.plan.preserved.length > 0 ? (
              <p>
                {t('history.cleanupPreserved', { count: cleanupTarget.plan.preserved.length })}
              </p>
            ) : null}
            <p role="alert" className="history-cleanup-warning">
              {t('history.cleanupWarning')}
            </p>
            {cleanupNotice ? <p role="alert">{cleanupNotice}</p> : null}
            {cleanupJob ? <p role="status" aria-live="polite">{t('history.cleanupProgress', { count: cleanupJob.progress.removed, total: cleanupJob.progress.total })}</p> : null}
            <div className="panel-actions">
              <button
                type="button"
                className="secondary"
                autoFocus
                disabled={cleanupBusy && (!cleanupJob || cleanupJob.cancelRequested)}
                onClick={() => { if (cleanupBusy) void cancelActiveCleanup(); else closeCleanup(); }}
              >
                {cleanupBusy ? t(cleanupJob?.cancelRequested ? 'history.cleanupStopping' : 'history.cleanupStop') : t('workspace.create.cancel')}
              </button>
              <button
                type="button"
                className="danger"
                disabled={!writable || cleanupBusy}
                onClick={() => void applyCleanup()}
              >
                {cleanupTarget.resuming
                  ? t('history.cleanupResume')
                  : t('history.cleanupConfirm')}
              </button>
            </div>
          </>
        ) : null}
      </dialog>
    </>
  );
}
