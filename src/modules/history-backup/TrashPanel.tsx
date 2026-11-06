import { formatInstant, type RegionalPreferences } from '../platform-shell/regional';
import { useEffect, useRef, useState } from 'react';
import type { TrashEntry, TrashRecord, TrashOperation } from '../domain';
import type { NoteService, RestoreTrashRequest, PurgeTrashRequest } from '../editor/ports';
import { NoteEditor } from '../editor';
import { serviceErrorMessage, type SupportedLocaleId, type Translator } from '../presentation-i18n';
import { newOperationId } from '../platform-shell/tauri-note-service';
import type { TrashMutationRunner } from './DeleteNoteDialog';

interface Props {
  service: NoteService; t: Translator; locale: SupportedLocaleId; regional: RegionalPreferences; initialObjectId: string | null;
  run: TrashMutationRunner;
  canMutate(noteId: string, cancelling?: boolean): boolean;
  onChanged(noteId: string): void;
  onClose(): void;
}
const noChange = () => undefined;

export function TrashPanel({ service, t, locale, regional, initialObjectId, run, canMutate, onChanged, onClose }: Props) {
  const heading = useRef<HTMLHeadingElement>(null);
  const confirmDialog = useRef<HTMLDialogElement>(null);
  const cancelButton = useRef<HTMLButtonElement>(null);
  const confirmationOpener = useRef<Element | null>(null);
  const alive = useRef(true);
  const token = useRef(0);
  const mutating = useRef(false);
  const restoreRequest = useRef<RestoreTrashRequest | null>(null);
  const purgeRequest = useRef<PurgeTrashRequest | null>(null);
  const [entries, setEntries] = useState<TrashEntry[]>([]);
  const [operations, setOperations] = useState<TrashOperation[]>([]);
  const [operationsReady, setOperationsReady] = useState(false);
  const [selected, setSelected] = useState<TrashRecord | null>(null);
  const [path, setPath] = useState('');
  const [composing, setComposing] = useState(false);
  const [busy, setBusy] = useState(true);
  const [notice, setNotice] = useState('');
  const [confirmation, setConfirmation] = useState<
    { kind: 'restore'; record: TrashRecord; target: string } |
    { kind: 'purge'; record: TrashRecord; target: string } |
    { kind: 'resume'; operation: TrashOperation } |
    { kind: 'cancel'; operation: TrashOperation } | null>(null);

  async function refresh() {
    const id = ++token.current; setBusy(true);
    const [items, pending] = await Promise.all([service.listTrash(), service.listTrashOperations()]);
    if (!alive.current || id !== token.current) return;
    setBusy(false);
    if (items.ok) setEntries(items.value); else setNotice(serviceErrorMessage(locale, items.error.code));
    setOperationsReady(pending.ok);
    if (pending.ok) setOperations(pending.value); else setNotice(serviceErrorMessage(locale, pending.error.code));
    if (selected && items.ok && !items.value.some(entry => entry.objectId === selected.entry.objectId)) setSelected(null);
    return items.ok ? items.value : [];
  }

  async function select(objectId: string) {
    const id = ++token.current; setBusy(true); setSelected(null); setNotice('');
    const result = await service.readTrash(objectId);
    if (!alive.current || id !== token.current) return;
    setBusy(false);
    if (result.ok) { setSelected(result.value); setPath(result.value.entry.originalPath); }
    else setNotice(`${t('trash.unavailable')} ${serviceErrorMessage(locale, result.error.code)}`);
  }

  useEffect(() => {
    alive.current = true;
    const opener = document.activeElement;
    heading.current?.focus();
    void refresh().then(items => { if (alive.current && initialObjectId && items?.some(entry => entry.objectId === initialObjectId)) void select(initialObjectId); });
    return () => { alive.current = false; token.current++; confirmDialog.current?.close(); if (opener instanceof HTMLElement && opener.isConnected) opener.focus(); };
  }, []);

  useEffect(() => {
    if (!confirmation) return;
    confirmDialog.current?.showModal(); cancelButton.current?.focus();
  }, [confirmation]);

  function ask(value: NonNullable<typeof confirmation>) {
    confirmationOpener.current = document.activeElement;
    setConfirmation(value);
  }
  function closeConfirmation() {
    if (mutating.current) return;
    confirmDialog.current?.close(); setConfirmation(null);
    const opener = confirmationOpener.current;
    if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
  }

  async function confirm() {
    if (!confirmation || mutating.current || busy || composing) return;
    const entry = 'record' in confirmation ? confirmation.record.entry : confirmation.operation.entry;
    if (!canMutate(entry.noteId, confirmation.kind === 'cancel')) { setNotice(t('editor.draft.blocked')); closeConfirmation(); return; }
    mutating.current = true; setBusy(true);
    let message: string;
    if (confirmation.kind === 'restore') {
      const previous = restoreRequest.current;
      if (!previous || previous.objectId !== entry.objectId || previous.targetPath !== confirmation.target || previous.expectedRevision !== entry.deletedRevision) restoreRequest.current = { objectId: entry.objectId, targetPath: confirmation.target, expectedRevision: entry.deletedRevision, operationId: newOperationId() };
      const result = await run(entry.noteId, confirmation.target, () => service.restoreTrash(restoreRequest.current!));
      message = result.ok ? t('trash.completed') : serviceErrorMessage(locale, result.error.code);
      if (result.ok) restoreRequest.current = null;
    } else if (confirmation.kind === 'purge') {
      if (!purgeRequest.current || purgeRequest.current.objectId !== entry.objectId) purgeRequest.current = { objectId: entry.objectId, expectedRevision: entry.deletedRevision, operationId: newOperationId() };
      const result = await run(entry.noteId, entry.originalPath, () => service.purgeTrash(purgeRequest.current!));
      message = result.ok ? t('trash.completed') : serviceErrorMessage(locale, result.error.code);
      if (result.ok) purgeRequest.current = null;
    } else if (confirmation.kind === 'resume') {
      const result = await run(entry.noteId, entry.originalPath, () => service.resumeTrash(confirmation.operation.operationId));
      message = result.ok ? t('trash.completed') : serviceErrorMessage(locale, result.error.code);
    } else {
      const result = await run(entry.noteId, entry.originalPath, () => service.cancelTrash(confirmation.operation.operationId), true);
      message = result.ok ? t(result.value === 'cancelled' ? 'trash.cancelled' : 'trash.alreadyApplied') : serviceErrorMessage(locale, result.error.code);
      if (result.ok) { restoreRequest.current = null; purgeRequest.current = null; }
    }
    mutating.current = false;
    if (!alive.current) return;
    setBusy(false); closeConfirmation(); setNotice(message);
    onChanged(entry.noteId);
    await refresh();
  }

  async function exportSource() {
    if (!selected || busy) return;
    const id = ++token.current; setBusy(true);
    const result = await service.exportDraft(selected.document);
    if (!alive.current || id !== token.current) return;
    setBusy(false); setNotice(result.ok ? t(result.value.outcome === 'saved' ? 'editor.export.saved' : 'editor.export.cancelled') : serviceErrorMessage(locale, result.error.code));
  }

  const hasPending = selected && operations.some(op => op.entry.noteId === selected.entry.noteId);
  const locked = selected && (!operationsReady || !canMutate(selected.entry.noteId));
  const confirmEntry = confirmation && ('record' in confirmation ? confirmation.record.entry : confirmation.operation.entry);
  const destructive = confirmation?.kind === 'purge' || (confirmation?.kind === 'resume' && confirmation.operation.kind === 'purge');
  return <section className="trash-screen" aria-labelledby="trash-heading">
    <header className="draft-heading"><h2 id="trash-heading" ref={heading} tabIndex={-1}>{t('trash.title')}</h2><button type="button" className="secondary" disabled={mutating.current} onClick={() => { if (!mutating.current) onClose(); }}>{t('home.explain.close')}</button></header>
    {notice ? <p role="status" className="notice-panel">{notice}</p> : null}{busy ? <p role="status">{t('trash.working')}</p> : null}
    {operations.length ? <section className="notice-panel"><h3>{t('trash.pending')}</h3>{operations.map(operation => <div className="trash-pending" key={operation.operationId}><span>{t(`trash.${operation.kind}`)} · {operation.entry.originalPath}</span><div className="panel-actions"><button type="button" disabled={busy || !canMutate(operation.entry.noteId) || operation.phase === 'cancelling'} onClick={() => ask({ kind: 'resume', operation })}>{t('trash.resume')}</button><button type="button" className="secondary" disabled={busy || !canMutate(operation.entry.noteId, true)} onClick={() => ask({ kind: 'cancel', operation })}>{t('trash.cancelOperation')}</button></div></div>)}</section> : null}
    <div className="trash-layout"><aside aria-label={t('trash.title')}>
      {!entries.length && !busy ? <p>{t('trash.empty')}</p> : null}<ul className="note-list">{entries.map(entry => <li key={entry.objectId}><button type="button" className="note-item" disabled={busy} aria-pressed={selected?.entry.objectId === entry.objectId} onClick={() => void select(entry.objectId)}><strong>{entry.originalPath}</strong><time dateTime={entry.deletedAt}>{formatInstant(entry.deletedAt, regional)}</time></button></li>)}</ul>
      <button type="button" className="secondary" disabled={busy} onClick={() => void refresh()}>{t('workspace.error.retry')}</button>
    </aside><section className="trash-detail" aria-busy={busy}>
      {selected ? <><h3>{selected.entry.originalPath}</h3><p>{t('trash.retention', { days: selected.entry.retentionDays })}</p><p>{t('trash.preserved')}</p>
        <NoteEditor noteId={selected.entry.noteId} pathName={selected.entry.originalPath} source={selected.document.source} docVersion={selected.entry.deletedRevision} mode="read" readOnly locale={locale} translator={t} onDocChanged={noChange} onExplicitSave={noChange} onCompositionChange={noChange} />
        {locked ? <p role="status">{t('editor.draft.blocked')}</p> : null}
        <label htmlFor="trash-restore-path">{t('workspace.create.pathLabel')}</label><input id="trash-restore-path" value={path} disabled={busy} onChange={event => setPath(event.target.value)} onCompositionStart={() => setComposing(true)} onCompositionEnd={() => setComposing(false)} />
        <div className="panel-actions"><button type="button" className="primary" disabled={busy || composing || !!locked || !!hasPending || !path.trim()} onClick={() => ask({ kind: 'restore', record: selected, target: path })}>{t('trash.restore')}</button><button type="button" className="danger" disabled={busy || composing || !!locked || !!hasPending} onClick={() => ask({ kind: 'purge', record: selected, target: path })}>{t('trash.purge')}</button><button type="button" className="secondary" disabled={busy} onClick={() => void exportSource()}>{t('trash.export')}</button></div>
      </> : <p>{t('trash.select')}</p>}
    </section></div>
    {confirmation ? <dialog ref={confirmDialog} className="trash-confirm" aria-labelledby="trash-confirm-heading" onCancel={event => { event.preventDefault(); closeConfirmation(); }}><h2 id="trash-confirm-heading">{destructive ? t('trash.purge') : confirmation.kind === 'restore' ? t('trash.restore') : t(confirmation.kind === 'cancel' ? 'trash.cancelOperation' : 'trash.resume')}</h2><strong>{confirmEntry?.originalPath}</strong>{confirmation.kind === 'restore' ? <p>{confirmation.target}</p> : null}<p>{destructive ? t('trash.purgeBody') : t('trash.preserved')}</p><div className="panel-actions"><button ref={cancelButton} type="button" className="secondary" disabled={busy} onClick={closeConfirmation}>{t('workspace.create.cancel')}</button><button type="button" className={destructive ? 'danger' : 'primary'} disabled={busy} onClick={() => void confirm()}>{destructive ? t('trash.purge') : confirmation.kind === 'restore' ? t('trash.restore') : t(confirmation.kind === 'cancel' ? 'trash.cancelOperation' : 'trash.resume')}</button></div></dialog> : null}
  </section>;
}
