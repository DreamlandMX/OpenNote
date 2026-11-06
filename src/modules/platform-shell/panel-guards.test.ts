import { describe, expect, it } from 'vitest';
import { SaveController } from '../editor';
import type { NoteService } from '../editor';
import { createWorkspaceGate } from './WorkspaceScreen';
import { PanelGuardRegistry, type PanelGuard } from './panel-guards';

function panel(overrides: Partial<PanelGuard> & { key: string }): PanelGuard & { rescues: number } {
  const guard = {
    rescues: 0,
    label: () => `${overrides.key}.canvas`,
    status: () => 'dirty' as const,
    save: async () => true,
    draft: () => ({ relativePath: `${overrides.key}.canvas`, source: '{}' }),
    rescued() { guard.rescues += 1; },
    ...overrides,
  };
  return guard;
}

function harness(exportOutcome: 'saved' | 'cancelled' = 'saved') {
  const registry = new PanelGuardRegistry();
  const exported: { noteId: string; relativePath: string; source: string }[] = [];
  const noteService = {
    exportDraft: async (request: { noteId: string; relativePath: string; source: string }) => {
      exported.push(request);
      return { ok: true as const, value: { outcome: exportOutcome } };
    },
  } as unknown as NoteService;
  const saveController = new SaveController({ saveNote: () => new Promise(() => undefined) }, { uuid: () => 'op' });
  const gate = createWorkspaceGate({
    saveController,
    noteService,
    pathOf: () => undefined,
    bufferOf: () => undefined,
    pendingOperation: () => null,
    panels: () => registry.list(),
  });
  return { registry, gate, exported };
}

describe('panel exit guards', () => {
  it('lists an unsaved canvas beside the notes so closing the window warns about it', () => {
    const { registry, gate } = harness();
    expect(gate.blockedEntries()).toEqual([]);
    registry.set('canvas', panel({ key: 'canvas', status: () => 'conflict' }));
    expect(gate.blockedEntries()).toEqual([{ noteId: 'panel:canvas', path: 'canvas.canvas', status: 'conflict' }]);
    registry.set('canvas', null);
    expect(gate.blockedEntries()).toEqual([]);
  });

  it('lets the window close only when every panel saved', async () => {
    const { registry, gate } = harness();
    let saves = 0;
    registry.set('canvas', panel({ key: 'canvas', save: async () => { saves += 1; return true; } }));
    expect(await gate.saveBlockedAndWait()).toBe(true);
    expect(saves).toBe(1);
  });

  it('keeps the window open when a panel cannot save, such as a canvas without a file', async () => {
    const { registry, gate } = harness();
    registry.set('canvas', panel({ key: 'canvas', save: async () => false }));
    registry.set('database-view', panel({ key: 'database-view' }));
    expect(await gate.saveBlockedAndWait()).toBe(false);
  });

  it('treats a throwing save as not saved', async () => {
    const { registry, gate } = harness();
    registry.set('canvas', panel({ key: 'canvas', save: () => Promise.reject(new Error('io')) }));
    expect(await gate.saveBlockedAndWait()).toBe(false);
  });

  it('exports the panel text as a rescue copy and only then stops blocking', async () => {
    const { registry, gate, exported } = harness();
    const canvas = panel({ key: 'canvas', draft: () => ({ relativePath: 'untitled.canvas', source: '{"nodes":[]}' }) });
    registry.set('canvas', canvas);
    expect(await gate.exportBlocked()).toBe('exported');
    expect(exported).toEqual([{ noteId: 'panel:canvas', relativePath: 'untitled.canvas', source: '{"nodes":[]}' }]);
    expect(canvas.rescues).toBe(1);
  });

  it('keeps blocking when the rescue export is cancelled', async () => {
    const { registry, gate } = harness('cancelled');
    const canvas = panel({ key: 'canvas' });
    registry.set('canvas', canvas);
    expect(await gate.exportBlocked()).toBe('retained');
    expect(canvas.rescues).toBe(0);
  });
});
