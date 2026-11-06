import { useEffect, useRef, useState } from 'react';
import { Minus, Plus, RotateCcw, X } from 'lucide-react';
import type { Translator } from '../presentation-i18n';

const STEP = 1.25;
const MIN = 0.1;
const MAX = 8;

interface Props {
  url: string;
  alt: string;
  translator: Translator;
  onClose(): void;
}

/** F19: an image at full size. Every control has a key; closing returns focus. */
export function ImageViewer({ url, alt, translator, onClose }: Props) {
  const [scale, setScale] = useState(1);
  const dialogRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const before = document.activeElement;
    dialogRef.current?.focus();
    return () => {
      if (before instanceof HTMLElement && before.isConnected) before.focus();
    };
  }, []);

  const zoom = (factor: number) => setScale((value) => Math.min(MAX, Math.max(MIN, value * factor)));

  return (
    <div
      className="image-viewer"
      role="dialog"
      aria-modal="true"
      aria-label={alt === '' ? translator('imageViewer.title') : alt}
      tabIndex={-1}
      ref={dialogRef}
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === 'Escape') onClose();
        else if (event.key === '+' || event.key === '=') zoom(STEP);
        else if (event.key === '-') zoom(1 / STEP);
        else if (event.key === '0') setScale(1);
        else return;
        event.preventDefault();
        event.stopPropagation();
      }}
    >
      <div className="image-viewer-bar">
        <button type="button" aria-label={translator('imageViewer.zoomOut')} title={translator('imageViewer.zoomOut')} onClick={() => zoom(1 / STEP)}>
          <Minus size={16} aria-hidden="true" />
        </button>
        <span aria-live="polite">{Math.round(scale * 100)}%</span>
        <button type="button" aria-label={translator('imageViewer.zoomIn')} title={translator('imageViewer.zoomIn')} onClick={() => zoom(STEP)}>
          <Plus size={16} aria-hidden="true" />
        </button>
        <button type="button" aria-label={translator('imageViewer.reset')} title={translator('imageViewer.reset')} onClick={() => setScale(1)}>
          <RotateCcw size={16} aria-hidden="true" />
        </button>
        <button type="button" aria-label={translator('imageViewer.close')} title={translator('imageViewer.close')} onClick={onClose}>
          <X size={16} aria-hidden="true" />
        </button>
      </div>
      <div className="image-viewer-stage">
        <img src={url} alt={alt} style={{ transform: `scale(${scale})` }} />
      </div>
    </div>
  );
}
