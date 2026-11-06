import { useEffect, useRef, useState } from 'react';
import { X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import type { AttachmentImportOutcome, NoteService } from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  attachmentReasonKey,
  attachmentStatusKey,
  insertableReferences,
  isOnDisk,
} from './attachment-view';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** True when the note the panel was opened for can still take an insertion. */
  canInsert: boolean;
  /** The note a reference is inserted into; links are written relative to it. */
  notePath: string;
  onOpenPdf(path: string): void;
  /** Inserts text at the captured position; false when the editor refused. */
  onInsert(text: string): boolean;
  onClose(): void;
}

function newOperationId(): string {
  return crypto.randomUUID();
}

/**
 * S17 attachment import (F19).
 *
 * Files are chosen with the native picker and copied into the vault by the host,
 * which verifies each file's hash before publishing it and never replaces an
 * existing file. Only files the host reports as saved or already present can be
 * inserted as references, so a note never points at bytes that are not on disk.
 * "Retry" reuses the same operation so files written by the first attempt are
 * recognised instead of copied again.
 */
export function AttachmentPanel({ service, translator, locale, canInsert, notePath, onOpenPdf, onInsert, onClose }: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [destination, setDestination] = useState('assets');
  const [outcome, setOutcome] = useState<AttachmentImportOutcome | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [busy, setBusy] = useState(false);
  const [inserted, setInserted] = useState<'none' | 'ok' | 'failed'>('none');
  const operationRef = useRef<string | null>(null);
  const [pdfs, setPdfs] = useState<readonly string[] | null>(null);

  useEffect(() => {
    let live = true;
    void service.listVaultPdfs().then((outcome) => {
      if (live && outcome.ok) setPdfs(outcome.value);
    });
    return () => { live = false; };
  }, [service]);

  async function run(retry: boolean): Promise<void> {
    if (busy) return;
    if (!retry || operationRef.current === null) operationRef.current = newOperationId();
    setBusy(true);
    setInserted('none');
    const result = await service.importAttachments({
      destination,
      operationId: operationRef.current,
      dialogTitle: translator('attachment.dialogTitle'),
    });
    setBusy(false);
    if (result.ok) {
      setOutcome(result.value);
      setError(null);
    } else {
      setError(result.error);
    }
  }

  const items = outcome?.items ?? [];
  const references = insertableReferences(items, notePath);
  const stored = items.filter((item) => item.status === 'stored').length;
  const present = items.filter((item) => item.status === 'alreadyPresent').length;
  const problems = items.length - stored - present;
  const retryable = items.some((item) => item.status === 'failed');

  return (
    <aside
      className="attachment-panel"
      aria-labelledby="attachment-heading"
      aria-busy={busy}
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="attachment-head">
        <h2 id="attachment-heading">{translator('attachment.title')}</h2>
        <button
          type="button"
          className="attachment-close"
          aria-label={translator('attachment.close')}
          title={translator('attachment.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <p className="attachment-body">{translator('attachment.body')}</p>

      <label className="attachment-field">
        <span>{translator('attachment.destination')}</span>
        <input
          type="text"
          value={destination}
          disabled={busy}
          onChange={(event) => {
            setDestination(event.target.value);
            // A new destination is a new import, not a retry of the old one.
            operationRef.current = null;
          }}
        />
      </label>

      <div className="attachment-controls">
        <button type="button" className="primary" disabled={busy} onClick={() => void run(false)}>
          {translator('attachment.choose')}
        </button>
        {retryable ? (
          <button type="button" className="secondary" disabled={busy} onClick={() => void run(true)}>
            {translator('attachment.retry')}
          </button>
        ) : null}
      </div>

      {busy ? (
        <p className="attachment-note" role="status">{translator('attachment.working')}</p>
      ) : null}

      {error !== null ? (
        <div className="attachment-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code, error.messageKey)}</p>
        </div>
      ) : null}

      {outcome !== null && outcome.cancelled ? (
        <p className="attachment-note" role="status">{translator('attachment.cancelled')}</p>
      ) : null}

      {items.length > 0 ? (
        <>
          <p className="attachment-verdict" role="status">
            {translator('attachment.summary', { stored, present, problems })}
          </p>
          <ul className="attachment-results">
            {items.map((item, index) => {
              const reason = attachmentReasonKey(item.errorKey);
              return (
                <li
                  key={`${item.name}-${index}`}
                  className={isOnDisk(item) ? 'attachment-row' : 'attachment-row attachment-danger'}
                >
                  <span className="attachment-path">{item.destination || item.name}</span>
                  <span className="attachment-meta">{translator(attachmentStatusKey(item.status))}</span>
                  {reason !== null && !isOnDisk(item) ? (
                    <span className="attachment-meta">{translator(reason)}</span>
                  ) : null}
                </li>
              );
            })}
          </ul>
          <p className="attachment-note">{translator('attachment.writeDisabled')}</p>
          <div className="attachment-controls">
            <button
              type="button"
              className="primary"
              disabled={busy || references.length === 0 || !canInsert || inserted === 'ok'}
              onClick={() => setInserted(onInsert(references) ? 'ok' : 'failed')}
            >
              {translator('attachment.insert')}
            </button>
          </div>
          {!canInsert && references.length > 0 ? (
            <p className="attachment-note">{translator('attachment.insertBlocked')}</p>
          ) : null}
          {inserted === 'ok' ? (
            <p className="attachment-note" role="status">{translator('attachment.inserted')}</p>
          ) : null}
          {inserted === 'failed' ? (
            <p className="attachment-warn" role="alert">{translator('attachment.insertFailed')}</p>
          ) : null}
        </>
      ) : null}

      <p className="attachment-note">{translator('attachment.hint')}</p>

      {pdfs !== null ? (
        <section aria-labelledby="attachment-pdfs">
          <h3 id="attachment-pdfs" className="attachment-subhead">{translator('pdfReader.vaultPdfs')}</h3>
          {pdfs.length === 0 ? <p className="attachment-note">{translator('pdfReader.noPdfs')}</p> : (
            <ul className="attachment-pdfs">
              {pdfs.map((path) => (
                <li key={path}>
                  <button type="button" className="secondary" title={path} onClick={() => onOpenPdf(path)}>{path}</button>
                </li>
              ))}
            </ul>
          )}
        </section>
      ) : null}
    </aside>
  );
}
