import { formatInstant, type RegionalPreferences } from './regional';
import { useEffect, useRef, useState } from 'react';
import type { NoteDocument } from '../domain';
import type { DraftPage, NoteDraft, NoteService } from '../editor/ports';
import { loadDraftComparison, sameDiskVersion, type DraftComparison } from '../editor/draft-recovery';
import { serviceErrorMessage, type SupportedLocaleId, type Translator } from '../presentation-i18n';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  regional: RegionalPreferences;
  onRestore(draft: NoteDraft, disk: NoteDocument): boolean;
  onClose(): void;
}

export function DraftRecoveryPanel({ service, translator: t, locale, regional, onRestore, onClose }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const session = useRef(0);
  const listSession = useRef(0);
  const alive = useRef(true);
  const [page, setPage] = useState<DraftPage>({ items: [], nextBeforeSequence: null });
  const [comparison, setComparison] = useState<DraftComparison | null>(null);
  const [busy, setBusy] = useState(false);
  const [listing, setListing] = useState(true);
  const [notice, setNotice] = useState('');

  async function list(beforeSequence?: number) {
    const token = ++listSession.current;
    setListing(true);
    const result = await service.listDrafts({ limit: 30, ...(beforeSequence === undefined ? {} : { beforeSequence }) });
    if (!alive.current || listSession.current !== token) return;
    setListing(false);
    if (result.ok) setPage(previous => ({ ...result.value, items: beforeSequence === undefined ? result.value.items : [...previous.items, ...result.value.items] }));
    else setNotice(serviceErrorMessage(locale, result.error.code));
  }

  useEffect(() => {
    alive.current = true;
    const opener = document.activeElement;
    dialog.current?.showModal();
    void list();
    return () => {
      alive.current = false;
      session.current++;
      listSession.current++;
      dialog.current?.close();
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, []);

  async function select(draftId: string) {
    const token = ++session.current;
    setBusy(true);
    setComparison(null);
    setNotice('');
    const result = await loadDraftComparison(service, draftId);
    if (!alive.current || session.current !== token) return;
    setBusy(false);
    if (result.ok) setComparison(result.value);
    else setNotice(serviceErrorMessage(locale, result.error.code));
  }

  async function restore() {
    if (!comparison || busy) return;
    const token = ++session.current;
    setBusy(true);
    const disk = await service.readNote(comparison.draft.noteId);
    if (!alive.current || session.current !== token) return;
    setBusy(false);
    if (!sameDiskVersion(comparison.disk, disk)) {
      setComparison({ ...comparison, disk });
      setNotice(t('editor.draft.changed'));
      return;
    }
    if (disk.ok && onRestore(comparison.draft, disk.value)) onClose();
    else setNotice(t('editor.draft.blocked'));
  }

  async function exportDraft() {
    if (!comparison || busy) return;
    const token = ++session.current;
    setBusy(true);
    const result = await service.exportDraft(comparison.draft);
    if (!alive.current || session.current !== token) return;
    setBusy(false);
    setNotice(result.ok ? t(result.value.outcome === 'saved' ? 'editor.export.saved' : 'editor.export.cancelled') : serviceErrorMessage(locale, result.error.code));
  }

  return <dialog ref={dialog} className="draft-dialog" aria-labelledby="draft-heading" onCancel={event => { event.preventDefault(); onClose(); }}>
    <header className="draft-heading"><div><h2 id="draft-heading">{t('editor.draft.open')}</h2><p>{t('editor.draft.body')}</p></div>
      <button type="button" className="secondary" onClick={onClose}>{t('editor.draft.later')}</button></header>
    <div className="draft-layout">
      <aside aria-label={t('editor.draft.open')}>
        {listing ? <p role="status">{t('workspace.loading')}</p> : null}
        {!listing && page.items.length === 0 ? <p>{t('editor.draft.empty')}</p> : null}
        <ul className="note-list">{page.items.map(draft => <li key={draft.draftId}>
          <button type="button" className="note-item" aria-pressed={comparison?.draft.draftId === draft.draftId} onClick={() => void select(draft.draftId)}>
            <span>{draft.relativePath}</span><time dateTime={draft.savedAt}>{formatInstant(draft.savedAt, regional)}</time>
          </button></li>)}</ul>
        {page.nextBeforeSequence !== null ? <button type="button" className="secondary" disabled={listing} onClick={() => void list(page.nextBeforeSequence!)}>{t('editor.draft.more')}</button> : null}
        {page.items.length === 0 && !listing ? <button type="button" className="secondary" onClick={() => void list()}>{t('workspace.error.retry')}</button> : null}
      </aside>
      <section className="draft-detail" aria-busy={busy}>
        {notice ? <p role="status" className="notice-panel">{notice}</p> : null}
        {busy ? <p role="status">{t('workspace.loading')}</p> : null}
        {comparison ? <>
          <div className="draft-compare">
            <section><h3>{t('editor.conflict.localHeading')}</h3><pre tabIndex={0}>{comparison.draft.source}</pre></section>
            <section><h3>{t('editor.conflict.diskHeading')}</h3>{comparison.disk.ok ? <pre tabIndex={0}>{comparison.disk.value.source}</pre> : <p role="alert">{serviceErrorMessage(locale, comparison.disk.error.code)}</p>}</section>
          </div>
          <div className="panel-actions">
            <button type="button" className="primary" disabled={busy || !comparison.disk.ok || comparison.disk.value.readOnly} onClick={() => void restore()}>{t('editor.draft.restore')}</button>
            <button type="button" className="secondary" disabled={busy} onClick={() => void exportDraft()}>{t('editor.save.failed.export')}</button>
          </div>
        </> : null}
      </section>
    </div>
  </dialog>;
}
