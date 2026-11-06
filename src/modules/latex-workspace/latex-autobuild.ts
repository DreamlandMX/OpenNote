/** How long typing must pause before an automatic build starts. */
export const AUTO_BUILD_IDLE_MS = 1500;

export interface AutoBuildState {
  enabled: boolean;
  /** Counts edits; each keystroke that changes the text adds one. */
  changeSeq: number;
  /** The change count when the last build started, saved text included. */
  builtSeq: number;
  /** When the last edit happened, in ms. */
  lastEditAt: number;
  /** A save or compile is in flight, or the host still runs a job. */
  building: boolean;
}

export type AutoBuildStep =
  | { kind: 'idle' }
  /** Edits arrived during a build; build again once it ends. At most one waits. */
  | { kind: 'queued' }
  | { kind: 'wait'; ms: number }
  | { kind: 'build' };

/**
 * What the workspace should do next. A build is never interrupted: edits made
 * while one runs only mark the next one, and that next build starts after the
 * same pause once the running one ends.
 */
export function nextAutoBuild(state: AutoBuildState, now: number, idleMs = AUTO_BUILD_IDLE_MS): AutoBuildStep {
  if (!state.enabled || state.changeSeq === state.builtSeq) return { kind: 'idle' };
  if (state.building) return { kind: 'queued' };
  const left = idleMs - (now - state.lastEditAt);
  return left > 0 ? { kind: 'wait', ms: left } : { kind: 'build' };
}
