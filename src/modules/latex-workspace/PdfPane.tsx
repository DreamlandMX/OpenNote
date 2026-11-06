import { useCallback, useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronLeft, ChevronRight, ChevronUp, Maximize2, Minus, Plus, Search } from 'lucide-react';
import type { Translator } from '../presentation-i18n';
import {
  clampPage,
  cycleHit,
  findHits,
  fitWidthZoom,
  stepZoom,
  type PdfFreshness,
  type SearchHit,
} from './pdf-view';
import type { OpenedPdf, PdfDocument } from './pdf-engine';
import './pdf-pane.css';

export interface PdfSource {
  /** Bytes of a PDF the host has already validated. */
  bytes: Uint8Array;
  /** Short name shown in the toolbar, e.g. `main.pdf`. */
  label: string;
  /** `outdated` when the source changed after this PDF was built. */
  freshness: PdfFreshness;
}

/** A spot on a page, in PDF points from the page's top left. */
export interface PdfMark {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Props {
  source: PdfSource | null;
  translator: Translator;
  /** Scrolls to a page, and to `mark` on it when given. Each nonce acts once. */
  jump?: { page: number; nonce: number; mark?: PdfMark } | null;
  onPageChange?(page: number): void;
  /** Ctrl/Cmd+click on a page, in PDF points from its top left. */
  onLocate?(page: number, x: number, y: number): void;
}

type Engine = typeof import('./pdf-engine');
let enginePromise: Promise<Engine> | null = null;
function loadEngine(): Promise<Engine> {
  enginePromise ??= import('./pdf-engine');
  return enginePromise;
}

type Zoom = { mode: 'fit' } | { mode: 'fixed'; value: number };

/**
 * PDF reader beside the LaTeX source: page navigation, zoom, fit-width, text
 * search and selectable text. Only pages near the viewport are rendered, so a
 * long thesis stays responsive.
 *
 * If a new PDF fails to open, the previous one stays on screen with an error,
 * instead of leaving the reader empty. An outdated PDF is labelled as such.
 */
export function PdfPane({ source, translator, jump = null, onPageChange, onLocate }: Props) {
  const [opened, setOpened] = useState<OpenedPdf | null>(null);
  const doc: PdfDocument | null = opened?.doc ?? null;
  const [shown, setShown] = useState<PdfSource | null>(null);
  const [error, setError] = useState<'open' | 'tooLarge' | 'locked' | null>(null);
  const [missingPage, setMissingPage] = useState<number | null>(null);
  const [noText, setNoText] = useState(false);
  const [mark, setMark] = useState<{ page: number; rect: PdfMark } | null>(null);
  const appliedJump = useRef<number | null>(null);
  const baseWidthRef = useRef(612);
  const [loading, setLoading] = useState(false);
  const [page, setPage] = useState(1);
  const [pageInput, setPageInput] = useState('1');
  const [zoom, setZoom] = useState<Zoom>({ mode: 'fit' });
  const [baseWidth, setBaseWidth] = useState(612);
  const [scale, setScale] = useState(1);
  const [query, setQuery] = useState('');
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [hitIndex, setHitIndex] = useState(-1);
  const [searching, setSearching] = useState(false);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const pageRefs = useRef(new Map<number, HTMLDivElement>());
  const textCache = useRef<string[] | null>(null);

  // Open a new document; keep the old one if the new one fails.
  useEffect(() => {
    if (source === null) return;
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const engine = await loadEngine();
        const next = await engine.openPdf(source.bytes);
        if (cancelled) {
          void next.close();
          return;
        }
        const first = await next.doc.getPage(1);
        const firstWidth = first.getViewport({ scale: 1 }).width;
        baseWidthRef.current = firstWidth;
        setBaseWidth(firstWidth);
        setOpened(next);
        setShown(source);
        setError(null);
        textCache.current = null;
        setNoText(false);
        setHits([]);
        setHitIndex(-1);
        setPage((current) => clampPage(current, next.doc.numPages));
      } catch (caught) {
        if (cancelled) return;
        const engine = await loadEngine().catch(() => null);
        setError(engine !== null && caught instanceof engine.PdfTooLargeError ? 'tooLarge'
          : engine !== null && caught instanceof engine.PasswordException ? 'locked' : 'open');
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [source]);

  // Release a document once it is replaced or the pane closes.
  useEffect(() => () => { void opened?.close(); }, [opened]);
  useEffect(() => { setPageInput(String(page)); onPageChange?.(page); }, [page, onPageChange]);

  // Resolve fit-width against the container, and follow resizes.
  useEffect(() => {
    const element = scrollRef.current;
    if (element === null) return;
    const update = (): void => {
      setScale(zoom.mode === 'fit' ? fitWidthZoom(element.clientWidth, baseWidth) : zoom.value);
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, [zoom, baseWidth, doc]);

  const goTo = useCallback((target: number): void => {
    if (doc === null) return;
    const next = clampPage(target, doc.numPages);
    setPage(next);
    pageRefs.current.get(next)?.scrollIntoView({ block: 'start' });
  }, [doc]);

  // A citation or source jump: a page the PDF no longer has is said out loud,
  // never quietly swapped for the last page.
  useEffect(() => {
    if (doc === null || jump === null || appliedJump.current === jump.nonce) return;
    appliedJump.current = jump.nonce;
    if (jump.page > doc.numPages || jump.page < 1) {
      setMissingPage(jump.page);
      return;
    }
    setMissingPage(null);
    setMark(jump.mark === undefined ? null : { page: jump.page, rect: jump.mark });
    setPage(jump.page);
    const scroller = scrollRef.current;
    if (scroller === null) return;
    // Pages above the target start as estimates and change height as they are
    // measured, which would slide the target out of view. Keep correcting until the
    // layout holds still, and stop at once if the reader starts scrolling.
    const targetY = jump.mark?.y;
    let frame = 0;
    let steady = 0;
    let lastTop = Number.NaN;
    const startedAt = performance.now();
    const stop = (): void => {
      cancelAnimationFrame(frame);
      scroller.removeEventListener('wheel', stop);
      scroller.removeEventListener('pointerdown', stop);
      scroller.removeEventListener('keydown', stop);
    };
    const settle = (): void => {
      const element = pageRefs.current.get(jump.page);
      if (element !== undefined) {
        const top = element.offsetTop;
        steady = top === lastTop ? steady + 1 : 0;
        lastTop = top;
        const zoomed = element.offsetHeight / pageHeightAtOne(element, baseWidthRef.current);
        const offset = targetY === undefined ? 0 : targetY * zoomed - scroller.clientHeight / 3;
        scroller.scrollTop = top - scroller.offsetTop + offset;
      }
      if (steady < 8 && performance.now() - startedAt < 2000) frame = requestAnimationFrame(settle);
      else stop();
    };
    scroller.addEventListener('wheel', stop, { passive: true });
    scroller.addEventListener('pointerdown', stop);
    scroller.addEventListener('keydown', stop);
    frame = requestAnimationFrame(settle);
    return stop;
  }, [doc, jump]);

  // Track the page in view while scrolling.
  useEffect(() => {
    const root = scrollRef.current;
    if (root === null || doc === null) return;
    const observer = new IntersectionObserver((entries) => {
      const visible = entries.filter((entry) => entry.isIntersecting)
        .sort((left, right) => right.intersectionRatio - left.intersectionRatio)[0];
      const number = Number((visible?.target as HTMLElement | undefined)?.dataset.page);
      if (number > 0) setPage(number);
    }, { root, threshold: [0.5] });
    pageRefs.current.forEach((element) => observer.observe(element));
    return () => observer.disconnect();
  }, [doc, scale]);

  const runSearch = useCallback(async (direction: 1 | -1): Promise<void> => {
    if (doc === null) return;
    let texts = textCache.current;
    if (texts === null) {
      setSearching(true);
      texts = [];
      for (let number = 1; number <= doc.numPages; number += 1) {
        const content = await (await doc.getPage(number)).getTextContent();
        texts.push(content.items.map((item) => ('str' in item ? item.str : '')).join(' '));
      }
      textCache.current = texts;
      setNoText(texts.every((text) => text.trim() === ''));
      setSearching(false);
    }
    const found = findHits(texts, query);
    const sameQuery = found.length === hits.length && found.every((hit, i) => hit.page === hits[i]?.page && hit.offset === hits[i]?.offset);
    const nextIndex = cycleHit(sameQuery ? hitIndex : -1, found.length, direction);
    setHits(found);
    setHitIndex(nextIndex);
    const target = found[nextIndex];
    if (target !== undefined) goTo(target.page);
  }, [doc, query, hits, hitIndex, goTo]);

  if (source === null && doc === null) {
    return <section className="pdf-pane pdf-pane-empty" aria-label={translator('pdf.label')}>
      <p>{translator('pdf.none')}</p>
    </section>;
  }

  const pageCount = doc?.numPages ?? 0;
  return (
    <section className="pdf-pane" aria-label={translator('pdf.label')} aria-busy={loading}>
      <div className="pdf-toolbar" role="toolbar" aria-label={translator('pdf.toolbar')}>
        <span className="pdf-name" title={shown?.label}>{shown?.label ?? ''}</span>
        {shown?.freshness === 'outdated' ? <span className="pdf-badge" role="status">{translator('pdf.outdated')}</span> : null}
        <span className="pdf-spacer" />
        <button type="button" className="icon" aria-label={translator('pdf.previous')} title={translator('pdf.previous')}
          disabled={doc === null || page <= 1} onClick={() => goTo(page - 1)}>
          <ChevronLeft size={16} aria-hidden="true" />
        </button>
        <label className="pdf-page">
          <span className="visually-hidden">{translator('pdf.page')}</span>
          <input
            type="text"
            inputMode="numeric"
            value={pageInput}
            disabled={doc === null}
            onChange={(event) => setPageInput(event.target.value)}
            onBlur={() => setPageInput(String(page))}
            onKeyDown={(event) => {
              if (event.key === 'Enter') goTo(Number(pageInput));
            }}
          />
          <span>{translator('pdf.of', { count: pageCount })}</span>
        </label>
        <button type="button" className="icon" aria-label={translator('pdf.next')} title={translator('pdf.next')}
          disabled={doc === null || page >= pageCount} onClick={() => goTo(page + 1)}>
          <ChevronRight size={16} aria-hidden="true" />
        </button>
        <span className="pdf-divider" />
        <button type="button" className="icon" aria-label={translator('pdf.zoomOut')} title={translator('pdf.zoomOut')}
          disabled={doc === null} onClick={() => setZoom({ mode: 'fixed', value: stepZoom(scale, -1) })}>
          <Minus size={16} aria-hidden="true" />
        </button>
        <span className="pdf-zoom" aria-live="polite">{Math.round(scale * 100)}%</span>
        <button type="button" className="icon" aria-label={translator('pdf.zoomIn')} title={translator('pdf.zoomIn')}
          disabled={doc === null} onClick={() => setZoom({ mode: 'fixed', value: stepZoom(scale, 1) })}>
          <Plus size={16} aria-hidden="true" />
        </button>
        <button type="button" className="icon" aria-label={translator('pdf.fitWidth')} title={translator('pdf.fitWidth')}
          aria-pressed={zoom.mode === 'fit'} disabled={doc === null} onClick={() => setZoom({ mode: 'fit' })}>
          <Maximize2 size={16} aria-hidden="true" />
        </button>
        <span className="pdf-divider" />
        <label className="pdf-search">
          <Search size={14} aria-hidden="true" />
          <span className="visually-hidden">{translator('pdf.search')}</span>
          <input
            type="search"
            value={query}
            placeholder={translator('pdf.search')}
            disabled={doc === null}
            onChange={(event) => { setQuery(event.target.value); setHits([]); setHitIndex(-1); }}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
                event.preventDefault();
                void runSearch(event.shiftKey ? -1 : 1);
              }
            }}
          />
        </label>
        <button type="button" className="icon" aria-label={translator('pdf.previousMatch')} title={translator('pdf.previousMatch')}
          disabled={doc === null || query.trim() === ''} onClick={() => void runSearch(-1)}>
          <ChevronUp size={16} aria-hidden="true" />
        </button>
        <button type="button" className="icon" aria-label={translator('pdf.nextMatch')} title={translator('pdf.nextMatch')}
          disabled={doc === null || query.trim() === ''} onClick={() => void runSearch(1)}>
          <ChevronDown size={16} aria-hidden="true" />
        </button>
        <span className="pdf-matches" role="status">
          {searching ? translator('pdf.searching')
            : query.trim() !== '' && hitIndex >= 0 ? translator('pdf.match', { index: hitIndex + 1, count: hits.length })
              : query.trim() !== '' && noText ? translator('pdf.noText')
              : query.trim() !== '' && textCache.current !== null && hits.length === 0 ? translator('pdf.noMatch') : ''}
        </span>
      </div>

      {error !== null ? (
        <p className="pdf-error" role="alert">
          {translator(error === 'tooLarge' ? 'pdf.error.tooLarge' : error === 'locked' ? 'pdf.error.locked' : 'pdf.error.open')}
          {doc !== null ? ` ${translator('pdf.error.keptPrevious')}` : ''}
        </p>
      ) : null}

      {missingPage !== null && doc !== null ? (
        <p className="pdf-error" role="alert">{translator('pdf.pageMissing', { page: missingPage, count: doc.numPages })}</p>
      ) : null}

      <div
        className="pdf-scroll"
        ref={scrollRef}
        tabIndex={0}
        aria-label={translator('pdf.document')}
        onKeyDown={(event) => {
          if (event.key !== 'PageUp' && event.key !== 'PageDown') return;
          event.preventDefault();
          goTo(page + (event.key === 'PageDown' ? 1 : -1));
        }}
      >
        {doc !== null ? Array.from({ length: doc.numPages }, (_, index) => (
          <PdfPageView
            key={index + 1}
            doc={doc}
            number={index + 1}
            scale={scale}
            fallbackWidth={baseWidth}
            label={translator('pdf.pageLabel', { page: index + 1 })}
            mark={mark?.page === index + 1 ? mark.rect : null}
            onLocate={onLocate}
            register={(element) => {
              if (element === null) pageRefs.current.delete(index + 1);
              else pageRefs.current.set(index + 1, element);
            }}
          />
        )) : loading ? <p className="pdf-loading">{translator('pdf.loading')}</p> : null}
      </div>
    </section>
  );
}

/** The page's height at 100%, from its current size and the document's base width. */
function pageHeightAtOne(element: HTMLElement, baseWidth: number): number {
  return (element.offsetHeight / element.offsetWidth) * baseWidth;
}

interface PageProps {
  doc: PdfDocument;
  number: number;
  scale: number;
  fallbackWidth: number;
  label: string;
  mark: PdfMark | null;
  onLocate?: Props['onLocate'];
  register(element: HTMLDivElement | null): void;
}

/** One page: sized placeholder, rendered (canvas + text layer) only when near the viewport. */
function PdfPageView({ doc, number, scale, fallbackWidth, label, mark, onLocate, register }: PageProps) {
  const holder = useRef<HTMLDivElement | null>(null);
  const content = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ width: fallbackWidth * scale, height: fallbackWidth * 1.294 * scale });
  const [near, setNear] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void doc.getPage(number).then((page) => {
      if (cancelled) return;
      const viewport = page.getViewport({ scale });
      setSize({ width: viewport.width, height: viewport.height });
    });
    return () => { cancelled = true; };
  }, [doc, number, scale]);

  useEffect(() => {
    const element = holder.current;
    if (element === null) return;
    const observer = new IntersectionObserver(([entry]) => setNear(entry?.isIntersecting ?? false), { rootMargin: '600px 0px' });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const element = content.current;
    if (!near || element === null) return;
    let cancelled = false;
    let renderTask: { cancel(): void } | null = null;
    let textLayer: { cancel(): void } | null = null;
    const draw = async (): Promise<void> => {
      const [engine, page] = await Promise.all([loadEngine(), doc.getPage(number)]);
      if (cancelled) return;
      const viewport = page.getViewport({ scale });
      const ratio = window.devicePixelRatio || 1;
      const canvas = document.createElement('canvas');
      canvas.width = Math.floor(viewport.width * ratio);
      canvas.height = Math.floor(viewport.height * ratio);
      canvas.style.width = `${viewport.width}px`;
      canvas.style.height = `${viewport.height}px`;
      const text = document.createElement('div');
      text.className = 'textLayer';
      const task = page.render({
        canvas,
        viewport,
        transform: ratio === 1 ? undefined : [ratio, 0, 0, ratio, 0, 0],
      });
      renderTask = task;
      try {
        await task.promise;
      } catch (caught) {
        if (caught instanceof engine.RenderingCancelledException) return;
        throw caught;
      }
      if (cancelled) return;
      element.replaceChildren(canvas, text);
      text.style.setProperty('--scale-factor', String(scale));
      const layer = new engine.TextLayer({ textContentSource: page.streamTextContent(), container: text, viewport });
      textLayer = layer;
      await layer.render().catch(() => undefined);
    };
    // A page that fails to draw stays a blank, correctly sized placeholder.
    draw().catch(() => undefined);
    return () => {
      cancelled = true;
      renderTask?.cancel();
      textLayer?.cancel();
    };
  }, [near, doc, number, scale]);

  return (
    <div
      className="pdf-page-view"
      data-page={number}
      role="group"
      aria-label={label}
      ref={(element) => { holder.current = element; register(element); }}
      style={{ width: size.width, height: size.height }}
      onClick={onLocate === undefined ? undefined : (event) => {
        if (!event.ctrlKey && !event.metaKey) return;
        event.preventDefault();
        const box = event.currentTarget.getBoundingClientRect();
        onLocate(number, (event.clientX - box.left) / scale, (event.clientY - box.top) / scale);
      }}
    >
      <div className="pdf-page-content" ref={content} />
      {mark !== null ? (
        <div
          className="pdf-sync-mark"
          aria-hidden="true"
          style={{ left: mark.x * scale, top: mark.y * scale, width: Math.max(mark.width * scale, 24), height: Math.max(mark.height * scale, 12) }}
        />
      ) : null}
    </div>
  );
}
