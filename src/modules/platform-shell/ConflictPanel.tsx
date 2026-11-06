import { useEffect, useMemo, useRef, useState } from 'react';
import type { NoteDocument } from '../domain';
import type { NoteService } from '../editor';
import { ConflictSession } from '../editor/conflict-resolution';
import { serviceErrorMessage, type SupportedLocaleId, type Translator } from '../presentation-i18n';
import { newOperationId } from './tauri-note-service';

interface Props {
  local: NoteDocument;
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  onResolved(disk: NoteDocument, local: NoteDocument): boolean;
  onClose(): void;
}

export function ConflictPanel({ local, service, translator: t, locale, onResolved, onClose }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const alive = useRef(true);
  const token = useRef(0);
  const session = useMemo(() => new ConflictSession(service, local, newOperationId), [service, local]);
  const [disk, setDisk] = useState<NoteDocument | null>(null);
  const [busy, setBusy] = useState(true);
  const [notice, setNotice] = useState('');
  const [confirmReload, setConfirmReload] = useState(false);
  const [path, setPath] = useState(local.relativePath.replace(/\.md$/i, '-copy.md'));

  async function load() {
    const id = ++token.current;
    setBusy(true);
    setConfirmReload(false);
    const result = await service.prepareConflict(local.noteId);
    if (!alive.current || id !== token.current) return;
    setBusy(false);
    if (result.ok) { setDisk(result.value); setNotice(''); }
    else setNotice(serviceErrorMessage(locale, result.error.code));
  }

  useEffect(() => {
    alive.current = true;
    const opener = document.activeElement;
    dialog.current?.showModal();
    void load();
    return () => {
      alive.current = false;
      token.current++;
      dialog.current?.close();
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, []);

  async function resolve(keepBoth: boolean) {
    if (!disk || busy) return;
    const id = ++token.current;
    setBusy(true);
    const result = await session.resolve(disk, keepBoth ? path : undefined);
    if (!alive.current || id !== token.current) return;
    setBusy(false);
    setConfirmReload(false);
    if (!result.ok) { setNotice(serviceErrorMessage(locale, result.error.code)); return; }
    setDisk(result.value.disk);
    if (result.value.kind === 'changed') {
      setNotice(t('editor.draft.changed') + (result.value.copy ? ` ${t('editor.conflict.copySaved', { path: result.value.copy.relativePath })}` : ''));
    } else if (onResolved(result.value.disk, local)) onClose();
    else setNotice(t('editor.draft.blocked'));
  }

  async function exportLocal() {
    if (busy) return;
    const id = ++token.current;
    setBusy(true);
    const result = await service.exportDraft(local);
    if (!alive.current || id !== token.current) return;
    setBusy(false);
    setNotice(result.ok ? t(result.value.outcome === 'saved' ? 'editor.export.saved' : 'editor.export.cancelled') : serviceErrorMessage(locale, result.error.code));
  }

  return <dialog ref={dialog} className="draft-dialog" aria-labelledby="conflict-heading" onCancel={event => { event.preventDefault(); onClose(); }}>
    <header className="draft-heading"><div><h2 id="conflict-heading">{t('editor.conflict.title')}</h2><p>{t('editor.conflict.body')}</p></div><button type="button" className="secondary" onClick={onClose}>{t('editor.conflict.close')}</button></header>
    {notice ? <p role="status" className="notice-panel">{notice}</p> : null}
    {busy ? <p role="status">{t('workspace.loading')}</p> : null}
    <div className="draft-compare" aria-busy={busy}>
      <section><h3>{t('editor.conflict.localHeading')}</h3><pre tabIndex={0}>{local.source}</pre></section>
      <section><h3>{t('editor.conflict.diskHeading')}{disk ? ` · r${disk.localRevision}` : ''}</h3>{disk ? <pre tabIndex={0}>{disk.source}</pre> : <p>{t('editor.conflict.diskLoadFailed')}</p>}</section>
    </div>
    <div className="create-form"><label htmlFor="conflict-copy-path">{t('workspace.create.pathLabel')}</label><input id="conflict-copy-path" value={path} disabled={busy || session.pendingCopyPath !== null} onChange={event => setPath(event.target.value)} /></div>
    <div className="panel-actions">
      <button type="button" className="primary" disabled={busy || !disk || disk.readOnly || !path.trim()} onClick={() => void resolve(true)}>{t('editor.conflict.keepBoth')}</button>
      <button type="button" className="secondary" disabled={busy || !disk} onClick={() => setConfirmReload(true)}>{t('editor.conflict.takeDisk')}</button>
      <button type="button" className="secondary" disabled={busy} onClick={() => void exportLocal()}>{t('editor.save.failed.export')}</button>
      <button type="button" className="secondary" disabled={busy} onClick={() => void load()}>{t('workspace.error.retry')}</button>
    </div>
    {confirmReload ? <div className="notice-panel"><p>{t('editor.conflict.reloadBackup')}</p><div className="panel-actions"><button type="button" className="primary" disabled={busy} onClick={() => void resolve(false)}>{t('editor.conflict.takeDisk')}</button><button type="button" className="secondary" disabled={busy} onClick={() => setConfirmReload(false)}>{t('workspace.create.cancel')}</button></div></div> : null}
  </dialog>;
}
