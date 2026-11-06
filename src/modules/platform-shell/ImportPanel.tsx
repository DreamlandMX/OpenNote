import { useCallback, useEffect, useRef, useState } from 'react';
import { Upload, X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import type { ImportPreview, ImportSourceType, ImportStatus, NoteService } from '../editor';
import type { ServiceError } from '../domain';
import type { MessageKey, Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import { importErrorKey, importFailureLine, importLossLines, importReport } from './import-view';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  onImported(): void;
  onClose(): void;
}

const POLL_MS = 300;
const SOURCE_KEYS: Readonly<Record<ImportSourceType, MessageKey>> = {
  folder: 'import.source.folder',
  zip: 'import.source.zip',
  notion: 'import.source.notion',
};

/**
 * S37/S38 import. Choosing a source only reads it: the preview says where everything
 * will go and what will not make it, and nothing is written until it is confirmed.
 * The host keeps running if this panel closes, and an import stopped by a cancel or
 * a restart is offered again when the panel next opens. Partial is never labelled
 * complete (F24-R04).
 */
export function ImportPanel({ service, translator, locale, onImported, onClose }: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [sourceType, setSourceType] = useState<ImportSourceType>('folder');
  const [prefix, setPrefix] = useState('imported');
  const [preparing, setPreparing] = useState(false);
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [status, setStatus] = useState<ImportStatus | null>(null);
  const [unfinished, setUnfinished] = useState<readonly ImportStatus[]>([]);
  const [error, setError] = useState<ServiceError | null>(null);
  const [copied, setCopied] = useState(false);
  const refreshed = useRef(false);

  const loadUnfinished = useCallback(async () => {
    const found = await service.importUnfinished();
    if (found.ok) setUnfinished(found.value);
  }, [service]);

  useEffect(() => { void loadUnfinished(); }, [loadUnfinished]);

  const running = status?.state === 'running';

  // Follow a running batch, and refresh the note list as it finishes.
  useEffect(() => {
    if (!running || status === null) return;
    const batchId = status.batchId;
    const timer = window.setInterval(() => {
      void service.importStatus(batchId).then((next) => {
        if (next.ok) setStatus(next.value);
      });
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [running, status?.batchId, service]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (status === null || running || refreshed.current) return;
    refreshed.current = true;
    if (status.notesDone > 0 || status.attachmentsDone > 0) onImported();
  }, [status, running, onImported]);

  async function choose(): Promise<void> {
    setPreparing(true);
    setError(null);
    setStatus(null);
    refreshed.current = false;
    const outcome = await service.importPrepare({
      sourceType,
      destinationPrefix: prefix.trim(),
      dialogTitle: translator(sourceType === 'folder' ? 'import.pickTitle' : 'import.pickZip'),
    });
    setPreparing(false);
    if (!outcome.ok) setError(outcome.error);
    else if (!outcome.value.cancelled) setPreview(outcome.value);
  }

  async function start(batchId: string): Promise<void> {
    setError(null);
    refreshed.current = false;
    const outcome = await service.importStart(batchId);
    if (!outcome.ok) {
      setError(outcome.error);
      return;
    }
    setPreview(null);
    setStatus(outcome.value);
    setUnfinished((current) => current.filter((item) => item.batchId !== batchId));
  }

  async function discard(batchId: string): Promise<void> {
    await service.importDiscard(batchId);
    setUnfinished((current) => current.filter((item) => item.batchId !== batchId));
  }

  function close(): void {
    // A plan nobody confirmed, or a finished import, has nothing left to resume.
    if (preview !== null) void service.importDiscard(preview.batchId);
    if (status !== null && (status.state === 'complete' || status.state === 'partial')) void service.importDiscard(status.batchId);
    onClose();
  }

  const statusLosses = status === null ? [] : importLossLines(status.losses, translator);
  const previewLosses = preview === null ? [] : importLossLines(preview.losses, translator);

  return (
    <aside
      className="import-panel"
      aria-labelledby="import-heading"
      aria-busy={preparing || running}
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          close();
        }
      }}
    >
      <header className="import-head">
        <h2 id="import-heading">{translator('import.title')}</h2>
        <button type="button" className="import-close" aria-label={translator('import.close')} title={translator('import.close')} onClick={close}>
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      {unfinished.map((item) => (
        <div key={item.batchId} className="import-resume" role="status">
          <p>{translator('import.unfinished', { name: item.sourceName, done: item.finished, total: item.total })}</p>
          <div className="import-actions">
            <button type="button" className="primary" disabled={running} onClick={() => void start(item.batchId)}>{translator('import.resume')}</button>
            <button type="button" className="secondary" onClick={() => void discard(item.batchId)}>{translator('import.discard')}</button>
          </div>
        </div>
      ))}

      {preview === null && status === null ? (
        <>
          <fieldset className="import-source" disabled={preparing}>
            <legend>{translator('import.source')}</legend>
            {(['folder', 'zip', 'notion'] as const).map((option) => (
              <label key={option}>
                <input type="radio" name="import-source" value={option} checked={sourceType === option} onChange={() => setSourceType(option)} />
                <span>{translator(SOURCE_KEYS[option])}</span>
              </label>
            ))}
          </fieldset>
          <label className="import-prefix">
            <span>{translator('import.prefix')}</span>
            <input type="text" value={prefix} disabled={preparing} onChange={(event) => setPrefix(event.target.value)} />
          </label>
          <p className="import-hint">{translator('import.prefixHint')}</p>
          <div className="import-actions">
            <button type="button" className="primary" disabled={preparing} onClick={() => void choose()}>
              <Upload size={15} aria-hidden="true" />
              {translator(preparing ? 'import.reading' : 'import.choose')}
            </button>
          </div>
        </>
      ) : null}

      {preview !== null ? (
        <section aria-labelledby="import-preview-heading">
          <h3 id="import-preview-heading" className="import-subhead">{translator('import.previewTitle')}</h3>
          <p>{translator('import.previewCounts', { notes: preview.notes, attachments: preview.attachments })}</p>
          {preview.sample.length > 0 ? (
            <ul className="import-list import-mapping">
              {preview.sample.map(([from, to]) => <li key={from} title={from}>{to}</li>)}
            </ul>
          ) : null}
          {previewLosses.length > 0 ? (
            <>
              <p className="import-warning">{translator('import.previewLosses', { count: previewLosses.length })}</p>
              <ul className="import-list">{previewLosses.map((line) => <li key={line}>{line}</li>)}</ul>
            </>
          ) : null}
          <div className="import-actions">
            <button type="button" className="primary" disabled={preview.notes + preview.attachments === 0} onClick={() => void start(preview.batchId)}>{translator('import.start')}</button>
            <button type="button" className="secondary" onClick={() => { void service.importDiscard(preview.batchId); setPreview(null); }}>{translator('import.back')}</button>
          </div>
        </section>
      ) : null}

      {error !== null ? (
        <div className="import-error" role="alert">
          <p>{importErrorKey(error.messageKey) !== null ? translator(importErrorKey(error.messageKey) as MessageKey) : serviceErrorMessage(locale, error.code, error.messageKey)}</p>
        </div>
      ) : null}

      {status !== null ? (
        <div className="import-result" role="status">
          <p><strong>{translator(`import.state.${status.state}` as MessageKey)}</strong></p>
          <progress max={Math.max(status.total, 1)} value={status.finished} />
          <p>{translator('import.progress', { notes: status.notesDone, attachments: status.attachmentsDone, done: status.finished, total: status.total })}</p>
          {running ? (
            <div className="import-actions">
              <button type="button" className="secondary" onClick={() => void service.importCancel(status.batchId)}>{translator('import.cancel')}</button>
            </div>
          ) : null}
          {status.failed.length > 0 ? (
            <ul className="import-list">{status.failed.map(([path, key]) => <li key={path}>{importFailureLine(path, key, translator)}</li>)}</ul>
          ) : null}
          {statusLosses.length > 0 ? (
            <>
              <p className="import-warning">{translator('import.losses', { count: statusLosses.length })}</p>
              <ul className="import-list">{statusLosses.map((line) => <li key={line}>{line}</li>)}</ul>
            </>
          ) : null}
          {!running ? (
            <div className="import-actions">
              {status.state === 'cancelled' || status.state === 'interrupted' || status.failed.length > 0 ? (
                <button type="button" className="primary" onClick={() => void start(status.batchId)}>{translator('import.resume')}</button>
              ) : null}
              {statusLosses.length > 0 || status.failed.length > 0 ? (
                <button type="button" className="secondary" onClick={() => {
                  void navigator.clipboard.writeText(importReport(status, translator)).then(() => setCopied(true));
                }}>
                  {translator(copied ? 'import.reportCopied' : 'import.copyReport')}
                </button>
              ) : null}
              <button type="button" className="secondary" onClick={close}>{translator('import.close')}</button>
            </div>
          ) : null}
        </div>
      ) : null}
    </aside>
  );
}
