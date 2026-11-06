import { useEffect, useRef, useState } from 'react';
import type { NoteDocument, TrashEntry } from '../domain';
import type { NoteService, ServiceOutcome, DeleteNoteRequest } from '../editor/ports';
import { sameDiskVersion } from '../editor/draft-recovery';
import { serviceErrorMessage, type SupportedLocaleId, type Translator } from '../presentation-i18n';
import { newOperationId } from '../platform-shell/tauri-note-service';

export type TrashMutationRunner = <T>(noteId: string, path: string, action: () => Promise<ServiceOutcome<T>>, allowLocalEdits?: boolean) => Promise<ServiceOutcome<T>>;
interface Props {
  local: NoteDocument; service: NoteService; t: Translator; locale: SupportedLocaleId;
  run: TrashMutationRunner;
  onDeleted(entry: TrashEntry): void;
  onConflict(local: NoteDocument): void;
  onClose(): void;
}

export function DeleteNoteDialog({ local, service, t, locale, run, onDeleted, onConflict, onClose }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const alive = useRef(true);
  const mutating = useRef(false);
  const request = useRef<DeleteNoteRequest | null>(null);
  const [busy, setBusy] = useState(true);
  const [ready, setReady] = useState(false);
  const [notice, setNotice] = useState('');

  async function check() {
    setBusy(true); setReady(false);
    const current = await service.readNote(local.noteId);
    if (!alive.current) return false;
    setBusy(false);
    if (!current.ok) {
      if (current.error.code === 'OBJECT_NOT_FOUND' || current.error.code === 'VERSION_CONFLICT') { onConflict(local); onClose(); }
      else setNotice(serviceErrorMessage(locale, current.error.code));
      return false;
    }
    if (!sameDiskVersion({ ok: true, value: local }, current)) { onConflict(local); onClose(); return false; }
    if (current.value.readOnly) { setNotice(t('editor.draft.blocked')); return false; }
    setReady(true); setNotice(''); return true;
  }

  useEffect(() => {
    alive.current = true;
    const opener = document.activeElement;
    dialog.current?.showModal(); cancel.current?.focus(); void check();
    return () => { alive.current = false; dialog.current?.close(); if (opener instanceof HTMLElement && opener.isConnected) opener.focus(); };
  }, []);

  async function confirm() {
    if (mutating.current || busy || !ready) return;
    mutating.current = true; setBusy(true);
    request.current ??= { noteId: local.noteId, expectedRevision: local.localRevision, expectedHash: local.contentHash, operationId: newOperationId() };
    const result = await run(local.noteId, local.relativePath, () => service.deleteNote(request.current!));
    mutating.current = false;
    if (!alive.current) return;
    setBusy(false);
    if (result.ok) { onDeleted(result.value); onClose(); }
    else if (result.error.code === 'VERSION_CONFLICT') { onConflict(local); onClose(); }
    else { setNotice(serviceErrorMessage(locale, result.error.code)); setReady(['IO_DENIED', 'DISK_FULL', 'FILE_LOCKED'].includes(result.error.code)); }
  }

  return <dialog ref={dialog} className="draft-dialog" aria-labelledby="delete-heading" onCancel={event => { event.preventDefault(); if (!mutating.current) onClose(); }}>
    <h2 id="delete-heading">{t('trash.delete')}</h2><p>{t('trash.deleteBody', { path: local.relativePath })}</p>
    <p>{t('trash.preserved')}</p><pre className="trash-source" tabIndex={0}>{local.source}</pre>
    {notice ? <p role="alert">{notice}</p> : null}{busy ? <p role="status">{t('trash.working')}</p> : null}
    <div className="panel-actions"><button ref={cancel} type="button" className="secondary" disabled={mutating.current} onClick={() => { if (!mutating.current) onClose(); }}>{t('workspace.create.cancel')}</button>
      <button type="button" className="secondary" disabled={busy} onClick={() => { if (request.current && ready) void confirm(); else void check(); }}>{t('workspace.error.retry')}</button>
      <button type="button" className="primary" disabled={busy || !ready} onClick={() => void confirm()}>{t('trash.delete')}</button></div>
  </dialog>;
}
