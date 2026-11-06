import { useEffect, useRef } from 'react';
import { X } from 'lucide-react';

/** How long a success message stays before it leaves on its own. */
export const TOAST_MS = 4000;

export interface ToastItem {
  key: string;
  text: string;
  onDismiss(): void;
}

/** Calls `clear` once `active` has stayed true for `ms`; a new render does not restart the wait. */
export function useAutoDismiss(active: boolean, clear: () => void, ms = TOAST_MS): void {
  const latest = useRef(clear);
  latest.current = clear;
  useEffect(() => {
    if (!active) return;
    const timer = window.setTimeout(() => latest.current(), ms);
    return () => window.clearTimeout(timer);
  }, [active, ms]);
}

/** Short success messages in the corner; failures stay as panels where they happened. */
export function ToastStack({ items, closeLabel }: { items: readonly ToastItem[]; closeLabel: string }) {
  return (
    <div className="toast-stack" role="status" aria-live="polite">
      {items.map(item => (
        <p className="toast" key={item.key}>
          <span>{item.text}</span>
          <button type="button" aria-label={closeLabel} title={closeLabel} onClick={item.onDismiss}>
            <X size={14} aria-hidden="true" />
          </button>
        </p>
      ))}
    </div>
  );
}
