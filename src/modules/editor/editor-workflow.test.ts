import { describe, expect, it } from 'vitest';
import {
  CHECKPOINT_INTERVAL_MS,
  EditorWorkflow,
  EXPLICIT_SAVE_RELEASE_MS,
  IDLE_SAVE_DELAY_MS,
} from './editor-workflow';
import { SaveController, type SaveControllerIo } from './save-controller';
import type { NoteDocument, SaveResult } from '../domain';
import type { SaveNoteRequest, ServiceOutcome } from './ports';

interface ScheduledEntry {
  fn: () => void;
  delayMs: number;
  cancelled: boolean;
}

function fakeScheduler() {
  const entries: ScheduledEntry[] = [];
  function schedule(fn: () => void, delayMs: number): () => void {
    const entry: ScheduledEntry = { fn, delayMs, cancelled: false };
    entries.push(entry);
    return () => {
      entry.cancelled = true;
    };
  }
  return { schedule, entries };
}

// Minimal advancing fake-time scheduler used for the composition-end /
// final-input / scheduled-save ordering regression.
class FakeClock {
  private now = 0;
  private nextId = 0;
  private readonly timers = new Map<number, { fireAt: number; fn: () => void }>();

  schedule = (fn: () => void, delayMs: number): (() => void) => {
    this.nextId += 1;
    const id = this.nextId;
    this.timers.set(id, { fireAt: this.now + delayMs, fn });
    return () => {
      this.timers.delete(id);
    };
  };

  advance(ms: number): void {
    const target = this.now + ms;
    for (;;) {
      let nextId: number | null = null;
      let nextAt = Number.POSITIVE_INFINITY;
      for (const [id, timerEntry] of this.timers) {
        if (
          nextId === null ||
          timerEntry.fireAt < nextAt ||
          (timerEntry.fireAt === nextAt && id < nextId)
        ) {
          nextId = id;
          nextAt = timerEntry.fireAt;
        }
      }
      if (nextId === null || nextAt > target) {
        break;
      }
      this.now = nextAt;
      const timerEntry = this.timers.get(nextId);
      this.timers.delete(nextId);
      timerEntry?.fn();
    }
    this.now = target;
  }

  pending(): { fireAt: number }[] {
    return [...this.timers.values()].map((timerEntry) => ({ fireAt: timerEntry.fireAt }));
  }
}

describe('EditorWorkflow composition boundary', () => {
  it('reschedules the dirty idle save from before composition even when nothing was typed during composition', () => {
    const { schedule, entries } = fakeScheduler();
    const saves: string[] = [];
    const checkpoints: string[] = [];
    const workflow = new EditorWorkflow(
      {
        onSaveDue: (noteId) => saves.push(noteId),
        onCheckpointDue: (noteId) => checkpoints.push(noteId),
      },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule },
    );

    workflow.noteActivated('n1');
    workflow.updateSource('n1');
    expect(entries.filter((entry) => !entry.cancelled && entry.delayMs === IDLE_SAVE_DELAY_MS)).toHaveLength(1);

    workflow.compositionStarted();
    expect(entries.filter((entry) => !entry.cancelled && entry.delayMs === IDLE_SAVE_DELAY_MS)).toHaveLength(0);

    workflow.compositionEnded();
    expect(entries.filter((entry) => !entry.cancelled && entry.delayMs === IDLE_SAVE_DELAY_MS)).toHaveLength(1);
    expect(entries.filter((entry) => !entry.cancelled && entry.delayMs === CHECKPOINT_INTERVAL_MS)).toHaveLength(1);

    const idleEntry = entries.find((entry) => !entry.cancelled && entry.delayMs === IDLE_SAVE_DELAY_MS);
    idleEntry?.fn();
    expect(saves).toEqual(['n1']);
  });

  it('fires the explicit save immediately when it arrives after composition end (Ctrl+S)', () => {
    const { schedule, entries } = fakeScheduler();
    const saves: string[] = [];
    const workflow = new EditorWorkflow(
      { onSaveDue: (noteId) => saves.push(noteId), onCheckpointDue: () => undefined },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule },
    );
    workflow.noteActivated('n1');

    workflow.explicitSave();
    expect(saves).toEqual(['n1']);
    expect(entries.filter((entry) => !entry.cancelled)).toHaveLength(0);
  });

  it('defers an explicit save typed during composition and never fires it synchronously at composition end', () => {
    const { schedule, entries } = fakeScheduler();
    const saves: string[] = [];
    const workflow = new EditorWorkflow(
      { onSaveDue: (noteId) => saves.push(noteId), onCheckpointDue: () => undefined },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule },
    );
    workflow.noteActivated('n1');
    workflow.compositionStarted();
    workflow.updateSource('n1');
    workflow.explicitSave();
    expect(saves).toEqual([]);

    // No synchronous explicit save at composition end: the composition's
    // final committed input has not reached the editor state yet.
    workflow.compositionEnded();
    expect(saves).toEqual([]);
    expect(entries.filter((entry) => !entry.cancelled && entry.delayMs === IDLE_SAVE_DELAY_MS)).toHaveLength(0);

    // The first committed input after composition end releases it promptly.
    workflow.updateSource('n1');
    expect(saves).toEqual(['n1']);
  });

  it('defers an explicit save typed during composition and falls back to the small scheduled release', () => {
    const { schedule, entries } = fakeScheduler();
    const saves: string[] = [];
    const workflow = new EditorWorkflow(
      { onSaveDue: (noteId) => saves.push(noteId), onCheckpointDue: () => undefined },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule },
    );
    workflow.noteActivated('n1');
    workflow.compositionStarted();
    workflow.explicitSave();
    workflow.compositionEnded();
    expect(saves).toEqual([]);

    const releaseEntry = entries.find(
      (entry) => !entry.cancelled && entry.delayMs === EXPLICIT_SAVE_RELEASE_MS,
    );
    expect(releaseEntry).toBeDefined();
    releaseEntry?.fn();
    expect(saves).toEqual(['n1']);

    // The consumed release never fires a second save.
    releaseEntry?.fn();
    expect(saves).toEqual(['n1']);
  });

  it('keeps a pending explicit save deferred across a restarted composition instead of firing mid-composition', () => {
    const { schedule, entries } = fakeScheduler();
    const saves: string[] = [];
    const workflow = new EditorWorkflow(
      { onSaveDue: (noteId) => saves.push(noteId), onCheckpointDue: () => undefined },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule },
    );
    workflow.noteActivated('n1');
    workflow.compositionStarted();
    workflow.explicitSave();
    workflow.compositionEnded();
    const firstRelease = entries.find(
      (entry) => !entry.cancelled && entry.delayMs === EXPLICIT_SAVE_RELEASE_MS,
    );
    expect(firstRelease).toBeDefined();

    workflow.compositionStarted();
    expect(firstRelease?.cancelled).toBe(true);
    workflow.compositionEnded();
    const secondRelease = entries.filter(
      (entry) => entry.delayMs === EXPLICIT_SAVE_RELEASE_MS,
    ).at(-1);
    expect(secondRelease?.cancelled).toBe(false);
    secondRelease?.fn();
    expect(saves).toEqual(['n1']);
  });
});

describe('EditorWorkflow dispose', () => {
  it('cancels idle, checkpoint and explicit-release timers so no callback fires after dispose (fake clock)', () => {
    const clock = new FakeClock();
    const saves: string[] = [];
    const checkpoints: string[] = [];
    const workflow = new EditorWorkflow(
      {
        onSaveDue: (noteId) => saves.push(noteId),
        onCheckpointDue: (noteId) => checkpoints.push(noteId),
      },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule: clock.schedule },
    );

    workflow.noteActivated('n1');
    workflow.updateSource('n1');
    workflow.compositionStarted();
    workflow.updateSource('n1');
    workflow.explicitSave();
    workflow.compositionEnded();
    expect(clock.pending().length).toBeGreaterThan(0);

    workflow.dispose();
    expect(clock.pending()).toHaveLength(0);

    clock.advance(CHECKPOINT_INTERVAL_MS * 5);
    expect(saves).toEqual([]);
    expect(checkpoints).toEqual([]);

    workflow.explicitSave();
    workflow.compositionEnded();
    expect(saves).toEqual([]);
    expect(checkpoints).toEqual([]);
  });
});

describe('EditorWorkflow checkpoints', () => {
  it('re-fires every interval while dirty and keeps the dirty buffer after a callback failure', () => {
    const { schedule, entries } = fakeScheduler();
    const checkpoints: string[] = [];
    const workflow = new EditorWorkflow(
      {
        onSaveDue: () => undefined,
        onCheckpointDue: (noteId) => {
          checkpoints.push(noteId);
          throw new Error('checkpoint write failed');
        },
      },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule },
    );
    workflow.noteActivated('n1');
    workflow.updateSource('n1');

    const checkpointEntry = entries.find((entry) => entry.delayMs === CHECKPOINT_INTERVAL_MS);
    checkpointEntry?.fn();
    expect(checkpoints).toEqual(['n1']);

    const nextEntry = entries.filter((entry) => entry.delayMs === CHECKPOINT_INTERVAL_MS).at(-1);
    nextEntry?.fn();
    expect(checkpoints).toEqual(['n1', 'n1']);

    const thirdEntry = entries.filter((entry) => entry.delayMs === CHECKPOINT_INTERVAL_MS).at(-1);
    thirdEntry?.fn();
    expect(checkpoints).toEqual(['n1', 'n1', 'n1']);
  });

  it('does not checkpoint an unmodified note', () => {
    const { schedule, entries } = fakeScheduler();
    const checkpoints: string[] = [];
    const workflow = new EditorWorkflow(
      { onSaveDue: () => undefined, onCheckpointDue: (noteId) => checkpoints.push(noteId) },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule },
    );
    workflow.noteActivated('n1');
    const entry = entries.find((entry2) => entry2.delayMs === CHECKPOINT_INTERVAL_MS);
    expect(entry).toBeUndefined();
    expect(checkpoints).toEqual([]);
  });

  it('delays checkpoints while composing; committing stops the cycle', () => {
    const { schedule, entries } = fakeScheduler();
    const checkpoints: string[] = [];
    const workflow = new EditorWorkflow(
      { onSaveDue: () => undefined, onCheckpointDue: (noteId) => checkpoints.push(noteId) },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule },
    );
    workflow.noteActivated('n1');
    workflow.updateSource('n1');
    const first = entries.filter((entry) => entry.delayMs === CHECKPOINT_INTERVAL_MS).at(-1);
    expect(first?.delayMs).toBe(CHECKPOINT_INTERVAL_MS);

    workflow.compositionStarted();
    first?.fn();
    expect(checkpoints).toEqual([]);
    const deferredEntry = entries.filter((entry) => entry.delayMs === 500).at(-1);
    expect(deferredEntry).toBeDefined();

    workflow.compositionEnded();
    deferredEntry?.fn();
    expect(checkpoints).toEqual(['n1']);

    workflow.markCommitted('n1');
    const next = entries.filter((entry) => entry.delayMs === CHECKPOINT_INTERVAL_MS).at(-1);
    next?.fn();
    expect(checkpoints).toEqual(['n1']);
  });
});

describe('composition end to final input to scheduled save (fake time)', () => {
  interface PendingCall {
    request: SaveNoteRequest;
    settle: (outcome: ServiceOutcome<SaveResult>) => void;
  }

  function awaitMicrotasks(times = 8): Promise<void> {
    let running: Promise<void> = Promise.resolve();
    for (let index = 0; index < times; index += 1) {
      running = running.then(() => Promise.resolve());
    }
    return running.then(() => undefined);
  }

  it('fires the scheduled save only after the final committed input; final text only, one in-flight, latest revision', async () => {
    const clock = new FakeClock();
    const pendingCalls: PendingCall[] = [];
    let uuidIndex = 0;
    const io: SaveControllerIo = {
      saveNote(request: SaveNoteRequest): Promise<ServiceOutcome<SaveResult>> {
        return new Promise<ServiceOutcome<SaveResult>>((resolve) => {
          pendingCalls.push({
            request,
            settle: (outcome) => {
              resolve(outcome);
            },
          });
        });
      },
    };
    const controller = new SaveController(io, {
      uuid: () => {
        uuidIndex += 1;
        return `op-${uuidIndex}`;
      },
    });
    const saves: string[] = [];
    const workflow = new EditorWorkflow(
      {
        onSaveDue: (noteId) => {
          saves.push(noteId);
          void controller.requestSave(noteId);
        },
        onCheckpointDue: () => undefined,
      },
      { idleSaveDelayMs: IDLE_SAVE_DELAY_MS, checkpointIntervalMs: CHECKPOINT_INTERVAL_MS, schedule: clock.schedule },
    );

    const document: NoteDocument = {
      noteId: 'n1',
      vaultId: 'v1',
      relativePath: 'notes/ime.md',
      source: 'committed base',
      localRevision: 1,
      contentHash: 'hash-1',
      readOnly: false,
    };
    workflow.noteActivated('n1');
    controller.registerDocument(document);

    // Half-composed text exists in the buffer during the composition; no save
    // may be triggered while it can still change under a pending DOM flush.
    workflow.compositionStarted();
    controller.compositionStarted('n1');
    controller.setBufferText('n1', 'ime-part');
    workflow.updateSource('n1');
    const refused = controller.requestSave('n1');
    await awaitMicrotasks();
    expect(pendingCalls).toHaveLength(0);
    expect(controller.snapshot('n1')?.status).toBe('dirty');

    // Composition end launches nothing: no save and no checkpoint reaction.
    workflow.compositionEnded();
    controller.compositionEnded('n1');
    await refused;
    await awaitMicrotasks(10);
    expect(pendingCalls).toHaveLength(0);
    expect(saves).toEqual([]);
    // The owed save is routed back into the 1000ms idle workflow.
    expect(clock.pending().filter((entry) => entry.fireAt === IDLE_SAVE_DELAY_MS)).toHaveLength(1);

    // The compositionend flush delivers the final committed text at t=600ms:
    // the idle save is responsibly re-armed from the latest input, so nothing
    // fires between composition end and the final input update.
    clock.advance(600);
    controller.setBufferText('n1', 'ime-part converted to final text');
    workflow.updateSource('n1');
    await awaitMicrotasks();
    expect(pendingCalls).toHaveLength(0);
    expect(clock.pending().filter((entry) => entry.fireAt === IDLE_SAVE_DELAY_MS + 600)).toHaveLength(1);

    // The scheduled save runs and captures the FINAL text only, exactly once.
    clock.advance(IDLE_SAVE_DELAY_MS + 600);
    expect(saves).toEqual(['n1']);
    await awaitMicrotasks();
    expect(pendingCalls).toHaveLength(1);
    expect(pendingCalls[0]?.request.source).toBe('ime-part converted to final text');
    expect(pendingCalls[0]?.request.expectedRevision).toBe(1);
    expect(pendingCalls[0]?.request.operationId).toBe('op-1');
    expect(controller.snapshot('n1')?.inFlight).toBe(true);
    expect(controller.snapshot('n1')?.dirty).toBe(true);
    expect(controller.snapshot('n1')?.revision).toBe(1);

    pendingCalls[0]?.settle({ ok: true, value: { noteId: 'n1', localRevision: 2, contentHash: 'hash-2' } });
    await awaitMicrotasks(10);
    expect(controller.snapshot('n1')?.status).toBe('clean');
    expect(controller.snapshot('n1')?.revision).toBe(2);
    expect(controller.snapshot('n1')?.dirty).toBe(false);

    // No chained or re-scheduled save double-fires after the committed save.
    clock.advance(CHECKPOINT_INTERVAL_MS * 3);
    expect(pendingCalls).toHaveLength(1);
    expect(saves).toEqual(['n1']);
  });
});
