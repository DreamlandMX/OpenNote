import { useEffect, useRef, type RefObject } from 'react';

/**
 * Focus contract shared by panels that handle Escape themselves.
 *
 * The panel takes focus on open, so a keyboard user reaches the Escape handler
 * without tabbing through the page, and the trigger gets focus back on close when
 * it is still mounted (the panel may have been opened by a now-hidden button).
 */
export function useOverlayFocus<T extends HTMLElement>(): RefObject<T | null> {
  const panelRef = useRef<T | null>(null);
  const openerRef = useRef<Element | null>(null);

  useEffect(() => {
    openerRef.current = document.activeElement;
    panelRef.current?.focus();
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);

  return panelRef;
}
