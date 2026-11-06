import { useEffect, useId, useRef, type ReactNode } from 'react';
import { MoreHorizontal } from 'lucide-react';

export interface WorkspaceActionsProps {
  label: string;
  children: ReactNode;
  /** The trigger's face; the "…" icon when absent. */
  icon?: ReactNode;
  /** Which trigger edge the panel lines up with. */
  align?: 'start' | 'end';
  /** Extra class for the trigger button. */
  triggerClassName?: string;
}

/** Native popover keeps Escape, outside-click dismissal and focus restoration. */
export function WorkspaceActions({ label, children, icon, align = 'end', triggerClassName }: WorkspaceActionsProps) {
  const id = useId();
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = () => panel.current?.hidePopover();
    window.addEventListener('resize', close);
    return () => window.removeEventListener('resize', close);
  }, []);
  return <>
    <button type="button" className={['secondary workspace-actions-toggle', triggerClassName].filter(Boolean).join(' ')} popoverTarget={id} aria-label={label} title={label}
      onClick={event => {
        if (!panel.current) return;
        const bounds = event.currentTarget.getBoundingClientRect();
        const style = panel.current.style;
        const below = window.innerHeight - bounds.bottom;
        // Near the bottom of the window the panel opens upwards instead.
        if (below < 240 && bounds.top > below) {
          style.top = 'auto';
          style.bottom = `${Math.max(8, window.innerHeight - bounds.top + 4)}px`;
          style.maxHeight = `${Math.max(100, bounds.top - 12)}px`;
        } else {
          const top = Math.min(bounds.bottom + 4, window.innerHeight - 120);
          style.bottom = 'auto';
          style.top = `${Math.max(8, top)}px`;
          style.maxHeight = `${Math.max(100, window.innerHeight - top - 12)}px`;
        }
        if (align === 'start') {
          style.right = 'auto';
          style.left = `${Math.max(8, bounds.left)}px`;
        } else {
          style.left = 'auto';
          style.right = `${Math.max(8, window.innerWidth - bounds.right)}px`;
        }
      }}>
      {icon ?? <MoreHorizontal size={18} aria-hidden="true" />}
    </button>
    <div id={id} ref={panel} popover="auto" className="workspace-actions" aria-label={label}
      onClick={(event) => {
        const target = event.target;
        if (target instanceof Element && target.closest('button:not(:disabled)')) panel.current?.hidePopover();
      }}>
      {children}
    </div>
  </>;
}
