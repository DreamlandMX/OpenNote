/**
 * Tab containment for modal dialogs (F30-R04).
 *
 * Side panels are deliberately not trapped: they are non-modal asides, so Tab may
 * leave them. The command dialogs declare `aria-modal="true"`, which requires the
 * keyboard focus to stay inside until they close.
 */

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

/**
 * Where Tab should move inside a dialog.
 *
 * `null` means the browser default is already inside the dialog. `activeIndex`
 * is -1 when focus is on the container or outside it.
 */
export function trapTabIndex(
  count: number,
  activeIndex: number,
  shiftKey: boolean,
): number | null {
  if (count === 0) {
    return null;
  }
  if (activeIndex < 0) {
    return shiftKey ? count - 1 : 0;
  }
  if (shiftKey && activeIndex === 0) {
    return count - 1;
  }
  if (!shiftKey && activeIndex === count - 1) {
    return 0;
  }
  return null;
}

export function focusableWithin(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) =>
      !element.hasAttribute('hidden') &&
      element.getAttribute('aria-hidden') !== 'true' &&
      element.tabIndex >= 0 &&
      element.closest('[inert]') === null,
  );
}

/** Applies the trap for one Tab press; true when the default was replaced. */
export function handleTrappedTab(
  container: HTMLElement,
  active: Element | null,
  shiftKey: boolean,
): boolean {
  const focusable = focusableWithin(container);
  const index = active instanceof HTMLElement ? focusable.indexOf(active) : -1;
  const target = trapTabIndex(focusable.length, index, shiftKey);
  if (target === null) {
    return false;
  }
  focusable[target]?.focus();
  return true;
}
