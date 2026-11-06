/**
 * Lazy pdf.js loader. Imported dynamically by PdfPane so the ~1 MB library and
 * its worker load only when a PDF is opened.
 *
 * Everything is local: the worker comes from the bundle, and fonts, CMaps,
 * ICC profiles and wasm decoders are served from `/pdfjs/` (copied from
 * pdfjs-dist by the Vite plugin in vite.config.ts). No CDN, no network.
 */
import * as pdfjs from 'pdfjs-dist';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import { MAX_PDF_BYTES } from './pdf-view';

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

const resourceBase = `${import.meta.env.BASE_URL}pdfjs/`;

export type PdfDocument = pdfjs.PDFDocumentProxy;
export type PdfPage = pdfjs.PDFPageProxy;
export { TextLayer, RenderingCancelledException, PasswordException } from 'pdfjs-dist';

export class PdfTooLargeError extends Error {}

/**
 * Opens a PDF from bytes. The bytes are copied first: pdf.js transfers its
 * buffer to the worker, and the caller's copy must stay usable.
 */
export interface OpenedPdf {
  doc: PdfDocument;
  /** Releases the document and its worker resources. */
  close(): Promise<void>;
}

export async function openPdf(bytes: Uint8Array): Promise<OpenedPdf> {
  if (bytes.byteLength > MAX_PDF_BYTES) throw new PdfTooLargeError('pdf too large');
  const task = pdfjs.getDocument({
    data: bytes.slice(),
    cMapUrl: `${resourceBase}cmaps/`,
    cMapPacked: true,
    standardFontDataUrl: `${resourceBase}standard_fonts/`,
    wasmUrl: `${resourceBase}wasm/`,
    iccUrl: `${resourceBase}iccs/`,
    enableXfa: false,
    // Nothing is fetched from elsewhere; the document is fully in memory.
    disableAutoFetch: true,
  });
  try {
    const doc = await task.promise;
    return { doc, close: () => task.destroy() };
  } catch (error) {
    void task.destroy();
    throw error;
  }
}
