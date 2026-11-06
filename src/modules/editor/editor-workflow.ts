export type WorkflowSchedule = {
  schedule: (fn: () => void, delayMs: number) => () => void;
};

export interface EditorWorkflowCallbacks {
  onSaveDue(noteId: string): void;
  onCheckpointDue(noteId: string): void;
}

export interface EditorWorkflowOptions {
  idleSaveDelayMs: number;
  checkpointIntervalMs: number;
  schedule: WorkflowSchedule['schedule'];
}

// At composition end the browser may still be one frame away from flushing the
// composition's final committed input (CodeMirror flushes pending DOM changes
// in a microtask, or on requestAnimationFrame on Android). Any release faster
// than that could capture half-composed text, so the small delay stays ahead
// of the biggest normal flush window without stalling the explicit save.
export const EXPLICIT_SAVE_RELEASE_MS = 100;

export class EditorWorkflow {
  private readonly callbacks: EditorWorkflowCallbacks;
  private readonly idleSaveDelayMs: number;
  private readonly checkpointIntervalMs: number;
  private readonly schedule: WorkflowSchedule['schedule'];
  private activeNoteId: string | null = null;
  private composing = false;
  private pendingSaveAfterComposition = false;
  private pendingExplicitSaveAfterComposition = false;
  private explicitReleaseCancel: (() => void) | null = null;
  private idleCancel: (() => void) | null = null;
  private checkpointCancel: (() => void) | null = null;
  private readonly dirtyNotes = new Set<string>();

  constructor(callbacks: EditorWorkflowCallbacks, options: EditorWorkflowOptions) {
    this.callbacks = callbacks;
    this.idleSaveDelayMs = options.idleSaveDelayMs;
    this.checkpointIntervalMs = options.checkpointIntervalMs;
    this.schedule = options.schedule;
  }

  noteActivated(noteId: string): void {
    this.activeNoteId = noteId;
    this.composing = false;
    this.pendingSaveAfterComposition = false;
    this.pendingExplicitSaveAfterComposition = false;
    this.cancelExplicitRelease();
    this.cancelIdle();
    this.cancelCheckpoint();
  }

  // Tears the workflow down for its whole lifetime (the shell calls this when
  // the vault unmounts): every scheduled timer is cancelled and all state is
  // cleared, so no save/checkpoint callback can fire afterwards and no post-
  // dispose trigger can resurrect one (activeNoteId stays null).
  dispose(): void {
    this.activeNoteId = null;
    this.composing = false;
    this.pendingSaveAfterComposition = false;
    this.pendingExplicitSaveAfterComposition = false;
    this.dirtyNotes.clear();
    this.cancelExplicitRelease();
    this.cancelIdle();
    this.cancelCheckpoint();
  }

  updateSource(noteId: string): void {
    if (noteId !== this.activeNoteId) {
      return;
    }
    this.dirtyNotes.add(noteId);
    if (this.composing) {
      this.pendingSaveAfterComposition = true;
      return;
    }
    if (this.pendingExplicitSaveAfterComposition) {
      // This is the first committed input after the composition ended: the
      // explicit save may run now, on the final text.
      this.releaseExplicitSaveAfterComposition();
      return;
    }
    this.scheduleIdleSave();
    this.ensureCheckpointTimer();
  }

  compositionStarted(): void {
    this.composing = true;
    // A restarted composition must not let a pending explicit-save release
    // fire mid-composition; it re-arms on the next composition end.
    this.cancelExplicitRelease();
    this.cancelIdle();
  }

  compositionEnded(): void {
    this.composing = false;
    if (this.pendingExplicitSaveAfterComposition) {
      // Do not fire synchronously here: the composition's final input may not
      // have reached the editor yet. Release on the first committed update
      // after the end, with a small timed fallback when no change follows.
      this.cancelIdle();
      this.explicitReleaseCancel = this.schedule(
        () => this.releaseExplicitSaveAfterComposition(),
        EXPLICIT_SAVE_RELEASE_MS,
      );
      return;
    }
    const noteId = this.activeNoteId;
    const hasDirtyEdits = this.pendingSaveAfterComposition || (noteId !== null && this.dirtyNotes.has(noteId));
    this.pendingSaveAfterComposition = false;
    if (hasDirtyEdits) {
      this.scheduleIdleSave();
      this.ensureCheckpointTimer();
    }
  }

  bufferCleared(noteId: string): void {
    this.dirtyNotes.delete(noteId);
    if (noteId === this.activeNoteId) {
      this.cancelIdle();
      this.cancelCheckpoint();
    }
  }

  explicitSave(): void {
    if (this.composing) {
      this.pendingExplicitSaveAfterComposition = true;
      return;
    }
    if (this.pendingExplicitSaveAfterComposition) {
      // A release was already armed from a composition end; Ctrl+S now confirms
      // it immediately instead of waiting for the fallback.
      this.releaseExplicitSaveAfterComposition();
      return;
    }
    this.cancelIdle();
    this.fireExplicitSave();
  }

  markCommitted(noteId: string): void {
    this.dirtyNotes.delete(noteId);
    if (noteId === this.activeNoteId) {
      this.cancelCheckpoint();
    }
  }

  private fireExplicitSave(): void {
    if (this.activeNoteId === null) {
      return;
    }
    this.callbacks.onSaveDue(this.activeNoteId);
  }

  private releaseExplicitSaveAfterComposition(): void {
    if (!this.pendingExplicitSaveAfterComposition) {
      return;
    }
    this.pendingExplicitSaveAfterComposition = false;
    this.pendingSaveAfterComposition = false;
    this.cancelExplicitRelease();
    this.cancelIdle();
    this.fireExplicitSave();
  }

  private cancelExplicitRelease(): void {
    if (this.explicitReleaseCancel !== null) {
      this.explicitReleaseCancel();
      this.explicitReleaseCancel = null;
    }
  }

  private scheduleIdleSave(): void {
    this.cancelIdle();
    this.idleCancel = this.schedule(() => {
      this.idleCancel = null;
      this.fireExplicitSave();
    }, this.idleSaveDelayMs);
  }

  private ensureCheckpointTimer(): void {
    if (this.checkpointCancel !== null) {
      return;
    }
    this.checkpointCancel = this.schedule(
      () => this.fireCheckpoint(),
      this.checkpointIntervalMs,
    );
  }

  private fireCheckpoint(): void {
    this.checkpointCancel = null;
    const noteId = this.activeNoteId;
    if (noteId === null || !this.dirtyNotes.has(noteId)) {
      return;
    }
    if (this.composing) {
      this.checkpointCancel = this.schedule(() => this.fireCheckpoint(), 500);
      return;
    }
    try {
      this.callbacks.onCheckpointDue(noteId);
    } catch {
      this.dirtyNotes.add(noteId);
    }
    this.checkpointCancel = this.schedule(
      () => this.fireCheckpoint(),
      this.checkpointIntervalMs,
    );
  }

  private cancelIdle(): void {
    if (this.idleCancel !== null) {
      this.idleCancel();
      this.idleCancel = null;
    }
  }

  private cancelCheckpoint(): void {
    if (this.checkpointCancel !== null) {
      this.checkpointCancel();
      this.checkpointCancel = null;
    }
  }
}

export const IDLE_SAVE_DELAY_MS = 1000;
export const CHECKPOINT_INTERVAL_MS = 10000;
