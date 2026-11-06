import { useEffect, useRef } from 'react';
import type { SaveStatus } from '../editor';

/**
 * Unsaved work held by a side panel (canvas, database view) rather than by the
 * note save controller. The workspace exit/switch gate asks these as well as
 * the open notes, so closing the window cannot drop a canvas the user drew.
 */
export interface PanelGuard {
  /** Stable while the panel is open, e.g. `canvas`. */
  readonly key: string;
  /** What the guard dialog lists: the file, or a name for an unsaved one. */
  label(): string;
  status(): Exclude<SaveStatus, 'clean'>;
  /**
   * Saves to the panel's file. Resolves `true` only when nothing is left
   * unsaved; a never-saved panel has no file and resolves `false`.
   */
  save(): Promise<boolean>;
  /** The text a rescue export writes, and the file name it suggests. */
  draft(): { relativePath: string; source: string };
  /** The draft is safely exported: stop blocking until the next edit. */
  rescued(): void;
}

/** Panels register while they have unsaved work and unregister when clean. */
export class PanelGuardRegistry {
  private readonly guards = new Map<string, PanelGuard>();

  set(key: string, guard: PanelGuard | null): void {
    if (guard === null) this.guards.delete(key);
    else this.guards.set(key, guard);
  }

  list(): readonly PanelGuard[] {
    return [...this.guards.values()];
  }
}

export type RegisterPanelGuard = (key: string, guard: PanelGuard | null) => void;

export interface PanelGuardState {
  /** Unsaved work exists that has not been rescued by an export. */
  active: boolean;
  label: string;
  status: Exclude<SaveStatus, 'clean'>;
  save(): Promise<boolean>;
  draft(): { relativePath: string; source: string };
  rescued(): void;
}

/**
 * Keeps a panel's entry in the exit gate while it has unsaved work. The guard
 * reads the latest state through a ref, so it never acts on a stale document.
 */
export function usePanelExitGuard(
  register: RegisterPanelGuard | undefined,
  key: string,
  state: PanelGuardState,
): void {
  const latest = useRef(state);
  latest.current = state;
  useEffect(() => {
    if (register === undefined) return;
    if (!state.active) {
      register(key, null);
      return;
    }
    register(key, {
      key,
      label: () => latest.current.label,
      status: () => latest.current.status,
      save: () => latest.current.save(),
      draft: () => latest.current.draft(),
      rescued: () => {
        register(key, null);
        latest.current.rescued();
      },
    });
  }, [register, key, state.active]);
  useEffect(() => () => register?.(key, null), [register, key]);
}

/** The guard dialog's row id for a panel, distinct from any note id. */
export function panelGuardId(guard: PanelGuard): string {
  return `panel:${guard.key}`;
}
