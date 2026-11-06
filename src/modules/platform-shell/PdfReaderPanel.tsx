import { useEffect, useMemo, useState } from 'react';
import { BookmarkPlus, ExternalLink, X } from 'lucide-react';
import { pdfCitation, type AttachmentInfo, type NoteService } from '../editor';
import { PdfPane, type PdfSource } from '../latex-workspace/PdfPane';
import type { ServiceError } from '../domain';
import { serviceErrorMessage, type SupportedLocaleId, type Translator } from '../presentation-i18n';
import { useOverlayFocus } from './use-overlay-focus';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** Vault-relative path of the PDF. */
  path: string;
  /** Page a citation asked for; null opens at the first page. */
  page: number | null;
  /** The note a citation goes into, when one can still take it. */
  citeInto: string | null;
  /** Inserts the citation at the captured position; false when the editor refused. */
  onCite(markdown: string): boolean;
  onClose(): void;
}

/** The text selected in the reader, if any: citation context, never written to the PDF. */
function selectedPdfText(): string {
  const selection = document.getSelection();
  const node = selection?.anchorNode;
  const element = node instanceof Element ? node : node?.parentElement;
  return element?.closest('.pdf-reader') ? (selection?.toString() ?? '') : '';
}

/**
 * F20: reads a PDF from the vault. The file is only read: a citation goes into the
 * note, and selecting text changes nothing in the PDF.
 */
export function PdfReaderPanel({ service, translator, locale, path, page, citeInto, onCite, onClose }: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [loaded, setLoaded] = useState<{ info: AttachmentInfo; bytes: Uint8Array } | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [notPdf, setNotPdf] = useState(false);
  const [current, setCurrent] = useState(page ?? 1);
  const [cited, setCited] = useState<'none' | 'ok' | 'failed'>('none');
  const name = path.split('/').pop() ?? path;

  useEffect(() => {
    let live = true;
    void (async () => {
      const info = await service.attachmentInfo(path);
      if (!live) return;
      if (!info.ok) { setError(info.error); return; }
      if (info.value.mediaType !== 'application/pdf') { setNotPdf(true); return; }
      const bytes = await service.readAttachment(info.value);
      if (!live) return;
      if (bytes.ok) setLoaded({ info: info.value, bytes: bytes.value });
      else setError(bytes.error);
    })();
    return () => { live = false; };
  }, [service, path]);

  const source = useMemo<PdfSource | null>(
    () => (loaded === null ? null : { bytes: loaded.bytes, label: name, freshness: 'current' }),
    [loaded, name],
  );
  const jump = useMemo(() => (page === null ? null : { page, nonce: 1 }), [page]);

  async function openWithSystem(): Promise<void> {
    const opened = await service.openAttachmentWithSystem(path);
    if (!opened.ok) setError(opened.error);
  }

  return (
    <section
      className="pdf-reader"
      aria-labelledby="pdf-reader-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="pdf-reader-head">
        <h2 id="pdf-reader-heading" title={path}>{name}</h2>
        <span className="pdf-reader-note" role="status">
          {cited === 'ok' ? translator('pdfReader.cited') : cited === 'failed' ? translator('attachment.insertFailed') : ''}
        </span>
        <div className="pdf-reader-actions">
          {citeInto !== null ? (
            <button
              type="button"
              className="primary"
              disabled={loaded === null}
              title={translator('pdfReader.citeHint')}
              onClick={() => setCited(onCite(pdfCitation(citeInto, path, current, selectedPdfText(), translator('pdfReader.citationLabel', { name, page: current }))) ? 'ok' : 'failed')}
            >
              <BookmarkPlus size={15} aria-hidden="true" /> {translator('pdfReader.cite', { page: current })}
            </button>
          ) : null}
          <button type="button" className="secondary" onClick={() => void openWithSystem()}>
            <ExternalLink size={15} aria-hidden="true" /> {translator('pdfReader.openSystem')}
          </button>
          <button type="button" className="pdf-reader-close" aria-label={translator('pdfReader.close')} title={translator('pdfReader.close')} onClick={onClose}>
            <X size={18} aria-hidden="true" />
          </button>
        </div>
      </header>
      {error !== null || notPdf ? (
        <p className="pdf-error" role="alert">
          {notPdf ? translator('pdfReader.notPdf') : serviceErrorMessage(locale, error!.code, error!.messageKey)}
        </p>
      ) : null}
      <div className="pdf-reader-body">
        <PdfPane source={source} translator={translator} jump={jump} onPageChange={setCurrent} />
      </div>
    </section>
  );
}
