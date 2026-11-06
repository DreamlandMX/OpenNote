import { useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { ChevronRight } from 'lucide-react';

export interface ContextMenuItem {
  key: string;
  label: string;
  icon?: ReactNode;
  /** Shortcut text shown at the end of the row, e.g. `Ctrl+B`. */
  shortcut?: string;
  disabled?: boolean;
  run?(): void;
  /** Opens a submenu instead of running. */
  children?: readonly ContextMenuEntry[];
}

/** `'separator'` draws a divider between groups. */
export type ContextMenuEntry = ContextMenuItem | 'separator';

interface Props {
  x: number;
  y: number;
  label: string;
  entries: readonly ContextMenuEntry[];
  onClose(): void;
}

const GAP = 4;

/** Shifts a fixed box back inside the window; a submenu flips to the left. */
function placeBox(element: HTMLElement, left: number, top: number, flipLeft: number | null): void {
  const { width, height } = element.getBoundingClientRect();
  const maxLeft = window.innerWidth - width - GAP;
  const x = left > maxLeft ? (flipLeft === null ? maxLeft : flipLeft - width) : left;
  element.style.left = `${Math.max(GAP, x)}px`;
  element.style.top = `${Math.max(GAP, Math.min(top, window.innerHeight - height - GAP))}px`;
}

function enabledIndexes(entries: readonly ContextMenuEntry[]): number[] {
  return entries.flatMap((entry, index) => (entry !== 'separator' && !entry.disabled ? [index] : []));
}

function MenuList({ entries, label, left, top, flipLeft, autoFocus, onClose, onBack }: {
  entries: readonly ContextMenuEntry[];
  label: string;
  left: number;
  top: number;
  /** Left edge of the parent row; a submenu that does not fit opens before it. */
  flipLeft: number | null;
  autoFocus: boolean;
  onClose(): void;
  /** Closes this submenu and returns focus to its parent row. */
  onBack?(): void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [open, setOpen] = useState<{ index: number; focus: boolean } | null>(null);

  useLayoutEffect(() => {
    if (listRef.current) placeBox(listRef.current, left, top, flipLeft);
  }, [left, top, flipLeft, entries]);

  // Focus moves into the menu once, when it opens; later renders leave it be.
  useLayoutEffect(() => {
    if (autoFocus) rowRefs.current[enabledIndexes(entries)[0] ?? -1]?.focus();
  }, []);

  function move(from: number, step: 1 | -1): void {
    const enabled = enabledIndexes(entries);
    if (enabled.length === 0) return;
    const at = enabled.indexOf(from);
    const next = at === -1 ? (step === 1 ? 0 : enabled.length - 1) : (at + step + enabled.length) % enabled.length;
    rowRefs.current[enabled[next] as number]?.focus();
  }

  function onKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>, index: number, item: ContextMenuItem): void {
    const keys: Record<string, () => void> = {
      ArrowDown: () => move(index, 1),
      ArrowUp: () => move(index, -1),
      ArrowRight: () => { if (item.children) setOpen({ index, focus: true }); },
      ArrowLeft: () => onBack?.(),
      Escape: () => (onBack ?? onClose)(),
    };
    const handler = keys[event.key];
    if (handler) {
      event.preventDefault();
      event.stopPropagation();
      handler();
    }
  }

  function activate(index: number, item: ContextMenuItem): void {
    if (item.children) {
      setOpen({ index, focus: true });
      return;
    }
    onClose();
    item.run?.();
  }

  const openItem = open === null ? null : entries[open.index];
  const openRow = open === null ? null : rowRefs.current[open.index];
  const rowBox = openRow?.getBoundingClientRect();

  return <>
    <div ref={listRef} className="editor-menu" role="menu" aria-label={label} style={{ left, top }}>
      {entries.map((entry, index) => entry === 'separator'
        ? <div key={`sep-${index}`} className="editor-menu-separator" role="separator" />
        : <button
            key={entry.key}
            ref={(element) => { rowRefs.current[index] = element; }}
            type="button"
            role="menuitem"
            data-menu-item={entry.key}
            aria-haspopup={entry.children ? 'menu' : undefined}
            aria-expanded={entry.children ? open?.index === index : undefined}
            className={open?.index === index ? 'is-open' : undefined}
            disabled={entry.disabled}
            onMouseEnter={() => setOpen(entry.children && !entry.disabled ? { index, focus: false } : null)}
            onClick={() => activate(index, entry)}
            onKeyDown={(event) => onKeyDown(event, index, entry)}
          >
            <span className="editor-menu-icon" aria-hidden="true">{entry.icon}</span>
            <span className="editor-menu-label">{entry.label}</span>
            {entry.shortcut ? <kbd>{entry.shortcut}</kbd> : null}
            {entry.children ? <ChevronRight className="editor-menu-chevron" size={14} aria-hidden="true" /> : null}
          </button>)}
    </div>
    {openItem && openItem !== 'separator' && openItem.children && rowBox ? (
      <MenuList
        key={openItem.key}
        entries={openItem.children}
        label={openItem.label}
        left={rowBox.right + GAP}
        top={rowBox.top - GAP}
        flipLeft={rowBox.left - GAP}
        autoFocus={open?.focus ?? false}
        onClose={onClose}
        onBack={() => {
          const index = open?.index ?? -1;
          setOpen(null);
          rowRefs.current[index]?.focus();
        }}
      />
    ) : null}
  </>;
}

/**
 * The editor's own right-click menu, in place of the WebView's. Rows with
 * `children` open a submenu on hover or ArrowRight; Escape or a click outside
 * closes it, and running an item closes the whole menu first.
 */
export function EditorContextMenu({ x, y, label, entries, onClose }: Props) {
  return <>
    <div
      className="editor-menu-backdrop"
      onMouseDown={(event) => { event.preventDefault(); onClose(); }}
      onContextMenu={(event) => { event.preventDefault(); onClose(); }}
    />
    <MenuList entries={entries} label={label} left={x} top={y} flipLeft={null} autoFocus onClose={onClose} />
  </>;
}
