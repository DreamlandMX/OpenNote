import { useEffect, useRef, type KeyboardEvent } from 'react';
import { ChevronDown, ChevronRight, X } from 'lucide-react';
import type { Translator } from '../presentation-i18n';
import type { OutlineRow } from './outline';
import { visibleOutline } from './outline';

interface Props {
  rows: readonly OutlineRow[];
  pending?: boolean;
  failed?: boolean;
  activeId: string | null;
  collapsedIds: readonly string[];
  translator: Translator;
  /** Collapse state is a device preference, never a note change (F14 §4). */
  onToggle(id: string): void;
  onJump(row: OutlineRow): void;
  onClose(): void;
  /** Focus the outline when it opens from the toolbar, not on every rerender. */
  focusOnMount?: boolean;
}

/**
 * S10 outline. It is a navigation list, so rows are buttons rather than links:
 * a heading jump must go through the editor's selection, not a URL. Folded
 * descendants are skipped by the keyboard exactly as they are visually.
 */
export function OutlinePanel({
  rows,
  pending = false,
  failed = false,
  activeId,
  collapsedIds,
  translator,
  onToggle,
  onJump,
  onClose,
  focusOnMount = false,
}: Props) {
  const listRef = useRef<HTMLDivElement | null>(null);
  const openerRef = useRef<Element | null>(null);

  useEffect(() => {
    if (!focusOnMount) {
      return;
    }
    openerRef.current = document.activeElement;
    const first = listRef.current?.querySelector<HTMLElement>('[data-outline-row]');
    first?.focus();
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, [focusOnMount]);

  useEffect(() => {
    const container = listRef.current;
    if (activeId === null || container === null || container.clientHeight === 0) {
      return;
    }
    const active = container.querySelector<HTMLElement>(`[data-outline-id="${CSS.escape(activeId)}"]`);
    if (!active) return;
    // scrollIntoView would also move the editor's outer scroll ancestors.
    const top = container.getBoundingClientRect().top + container.clientTop;
    const bounds = active.getBoundingClientRect();
    if (bounds.top < top || bounds.height > container.clientHeight) {
      container.scrollTop += bounds.top - top;
    } else if (bounds.bottom > top + container.clientHeight) {
      container.scrollTop += bounds.bottom - top - container.clientHeight;
    }
  }, [activeId]);

  const visible = visibleOutline(rows);

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key === 'ArrowLeft') {
      const row = rowOf(event.target);
      if (row === null) {
        return;
      }
      event.preventDefault();
      if (row.hasChildren && !row.collapsed) {
        onToggle(row.id);
        return;
      }
      focusNeighbour(listRef.current, row.id, 'previousParent');
      return;
    }
    if (event.key === 'ArrowRight') {
      const row = rowOf(event.target);
      if (row === null) {
        return;
      }
      event.preventDefault();
      if (row.hasChildren && row.collapsed) {
        onToggle(row.id);
        return;
      }
      focusNeighbour(listRef.current, row.id, 'firstChild');
      return;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      const row = rowOf(event.target);
      if (row === null) {
        return;
      }
      event.preventDefault();
      focusNeighbour(listRef.current, row.id, event.key === 'ArrowDown' ? 'next' : 'previous');
    }
  }

  function rowOf(target: EventTarget | null): OutlineRow | null {
    if (!(target instanceof HTMLElement)) {
      return null;
    }
    const id = target.dataset.outlineId;
    return id === undefined ? null : rows.find((row) => row.id === id) ?? null;
  }

  return (
    <aside
      className="outline-panel"
      aria-busy={pending}
      aria-labelledby="outline-heading"
      onKeyDown={handleKeyDown}
    >
      <header className="outline-head">
        <h2 id="outline-heading">{translator('outline.title')}</h2>
        <button
          type="button"
          className="outline-close"
          aria-label={translator('outline.close')}
          title={translator('outline.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>
      {pending ? (
        <p className="outline-empty" role="status">{translator('workspace.loading')}</p>
      ) : failed ? null : rows.length === 0 ? (
        <p className="outline-empty" role="status">
          {translator('outline.empty')}
        </p>
      ) : (
        <div className="outline-list" ref={listRef} role="tree" aria-label={translator('outline.treeLabel')}>
          {rows.map((row) => {
            if (row.hidden) {
              return null;
            }
            const active = row.id === activeId;
            return (
              <div
                key={row.id}
                className="outline-row"
                data-outline-row=""
                data-outline-id={row.id}
                data-active={active ? 'true' : undefined}
                role="treeitem"
                aria-level={row.level}
                aria-expanded={row.hasChildren ? (row.collapsed ? 'false' : 'true') : undefined}
                aria-selected={active}
                tabIndex={active || (activeId === null && visible[0]?.id === row.id) ? 0 : -1}
                style={{ paddingInlineStart: `${12 + row.depth * 16}px` }}
                onClick={() => onJump(row)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    onJump(row);
                  }
                }}
              >
                {row.hasChildren ? (
                  <button
                    type="button"
                    className="outline-toggle"
                    aria-label={
                      row.collapsed
                        ? translator('outline.expand', { name: row.text })
                        : translator('outline.collapse', { name: row.text })
                    }
                    title={
                      row.collapsed
                        ? translator('outline.expand', { name: row.text })
                        : translator('outline.collapse', { name: row.text })
                    }
                    tabIndex={-1}
                    onClick={(event) => {
                      event.stopPropagation();
                      onToggle(row.id);
                    }}
                  >
                    {row.collapsed ? (
                      <ChevronRight size={14} aria-hidden="true" />
                    ) : (
                      <ChevronDown size={14} aria-hidden="true" />
                    )}
                  </button>
                ) : (
                  <span className="outline-toggle-spacer" aria-hidden="true" />
                )}
                <span className="outline-text">{row.text}</span>
              </div>
            );
          })}
        </div>
      )}
      <p className="outline-hint">{translator('outline.hint')}</p>
      {collapsedIds.length > 0 ? (
        <button
          type="button"
          className="outline-reset"
          onClick={() => collapsedIds.forEach((id) => onToggle(id))}
        >
          {translator('outline.expandAll')}
        </button>
      ) : null}
    </aside>
  );
}

function focusNeighbour(
  container: HTMLDivElement | null,
  currentId: string,
  direction: 'next' | 'previous' | 'previousParent' | 'firstChild',
): void {
  if (container === null) {
    return;
  }
  const nodes = [...container.querySelectorAll<HTMLElement>('[data-outline-row]')];
  const index = nodes.findIndex((node) => node.dataset.outlineId === currentId);
  if (index < 0) {
    return;
  }
  const currentLevel = Number(nodes[index]!.getAttribute('aria-level') ?? '1');
  let target: HTMLElement | undefined;
  if (direction === 'next') {
    target = nodes[index + 1];
  } else if (direction === 'previous') {
    target = nodes[index - 1];
  } else if (direction === 'firstChild') {
    const next = nodes[index + 1];
    if (next !== undefined && Number(next.getAttribute('aria-level') ?? '1') > currentLevel) {
      target = next;
    }
  } else {
    for (let cursor = index - 1; cursor >= 0; cursor -= 1) {
      if (Number(nodes[cursor]!.getAttribute('aria-level') ?? '1') < currentLevel) {
        target = nodes[cursor];
        break;
      }
    }
  }
  target?.focus();
}
