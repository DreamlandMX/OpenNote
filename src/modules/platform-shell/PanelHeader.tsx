import type { ReactNode } from 'react';
import { X, type LucideIcon } from 'lucide-react';

export interface PanelHeaderProps {
  /** Id of the `<h2>`, which the panel names itself by (`aria-labelledby`). */
  id: string;
  icon: LucideIcon;
  title: string;
  /** One sentence on what the panel is for, shown under the title. */
  purpose: string;
  /** Small status beside the title, such as "Unsaved" or "Ready". */
  status?: ReactNode;
  closeLabel: string;
  onClose(): void;
}

/** The shared head of a tool panel: what it is, in one line, and a way out. */
export function PanelHeader({ id, icon: Icon, title, purpose, status, closeLabel, onClose }: PanelHeaderProps) {
  return (
    <header className="panel-head">
      <div className="panel-head-row">
        <Icon className="panel-head-icon" size={18} aria-hidden="true" />
        <h2 id={id}>{title}</h2>
        {status ?? null}
        <button type="button" className="panel-close" aria-label={closeLabel} title={closeLabel} onClick={onClose}>
          <X size={18} aria-hidden="true" />
        </button>
      </div>
      <p className="panel-purpose">{purpose}</p>
    </header>
  );
}

/** Numbered how-to steps for an empty panel. */
export function PanelSteps({ steps }: { steps: readonly string[] }) {
  return (
    <ol className="panel-steps">
      {steps.map((step, index) => (
        <li key={index}>
          <span className="panel-step-number" aria-hidden="true">{index + 1}</span>
          <span>{step}</span>
        </li>
      ))}
    </ol>
  );
}
