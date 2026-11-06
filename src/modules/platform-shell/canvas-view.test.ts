import { describe, expect, it } from 'vitest';
import type { CanvasDocument, CanvasNode } from '../editor';
import canvasWire from '../../../crates/onote-tasks/tests/fixtures/canvas-wire.json?raw';
import {
  canvasPathFromName,
  canvasSaveState,
  arrowDelta,
  beginDrag,
  commitDrag,
  connectNodes,
  connectionPaths,
  dragRect,
  exportSummary,
  isValidRect,
  listItems,
  MIN_NODE_HEIGHT,
  MIN_NODE_WIDTH,
  MOVE_FINE_STEP,
  MOVE_STEP,
  moveNode,
  newTextNode,
  reassociateNode,
  removeNodeLocally,
  resizeRect,
  reduceCanvasHistory,
  type CanvasHistory,
} from './canvas-view';

function node(overrides: Partial<CanvasNode> & { id: string }): CanvasNode {
  return {
    type: 'text',
    x: 0,
    y: 0,
    width: 240,
    height: 160,
    ...overrides,
  };
}

function doc(overrides: Partial<CanvasDocument> = {}): CanvasDocument {
  return {
    formatVersion: '1.0',
    nodes: [
      node({ id: 'n1', noteId: 'note-a', type: 'note' }),
      node({ id: 'n2', x: 300, text: 'hello' }),
    ],
    edges: [{ id: 'e1', from: 'n1', to: 'n2' }],
    ...overrides,
  };
}

describe('native canvas wire format', () => {
  it('creates cards with the type field required by Rust serialization', () => {
    const card = newTextNode(doc({ nodes: [], edges: [] }), 'new', '你好');
    expect(JSON.parse(JSON.stringify(card))).toEqual({
      id: 'new', type: 'text', x: 24, y: 0, width: 240, height: 120, text: '你好',
    });
  });

  it('repairs a parsed native reference and preserves unknown fields at every level', () => {
    const source = JSON.parse(canvasWire);
    const parsed: CanvasDocument = JSON.parse(JSON.stringify(source));
    expect(listItems(parsed, [])[0]).toMatchObject({ kind: 'note', broken: true });
    const repaired = reassociateNode(parsed, 'n', 'exists');
    expect(JSON.parse(JSON.stringify(moveNode(repaired, 'n', 10, 20)))).toEqual({
      ...source, nodes: [{ ...source.nodes[0], noteId: 'exists', x: 10, y: 20 }],
    });
    expect(parsed).toEqual(source);
  });
});

describe('dragging', () => {
  it('rejects invalid geometry, removed nodes and drags from an old layout', () => {
    const original = doc();
    const drag = beginDrag(original.nodes[0]!, 0, 0);
    expect(commitDrag(original, drag, { ...drag.origin, x: Infinity })).toBe(original);
    expect(commitDrag(original, drag, { ...drag.origin, width: 1 })).toBe(original);
    const removed = removeNodeLocally(original, 'n1');
    expect(commitDrag(removed, drag, { ...drag.origin, x: 10 })).toBe(removed);
    const moved = moveNode(original, 'n1', 5, 0);
    expect(commitDrag(moved, drag, { ...drag.origin, x: 10 })).toBe(moved);
  });
  it('produces whole-pixel geometry so two equal drags match', () => {
    const drag = beginDrag(node({ id: 'n1', x: 10, y: 20 }), 15, 25);
    const rect = dragRect(drag, 100.4, 200.6);
    expect(rect).toEqual({ x: 95, y: 196, width: 240, height: 160 });
    expect(dragRect(drag, 100.4, 200.6)).toEqual(rect);
  });

  it('commits one change per drag', () => {
    // F21-R03: the whole drag is a single document change.
    const drag = beginDrag(doc().nodes[0]!, 0, 0);
    const next = commitDrag(doc(), drag, { x: 50, y: 60, width: 240, height: 160 });
    expect(next.nodes[0]).toMatchObject({ x: 50, y: 60 });
    expect(next.edges).toHaveLength(1);
  });

  it('a drag that moved nothing returns the same document', () => {
    const original = doc();
    const drag = beginDrag(original.nodes[0]!, 0, 0);
    expect(commitDrag(original, drag, { x: 0, y: 0, width: 240, height: 160 })).toBe(original);
  });

  it('does not touch any other node', () => {
    const drag = beginDrag(doc().nodes[1]!, 300, 0);
    const next = commitDrag(doc(), drag, { x: 400, y: 0, width: 240, height: 160 });
    expect(next.nodes[1]?.x).toBe(400);
    expect(next.nodes[0]?.x).toBe(0);
  });
});

describe('canvas undo and redo', () => {
  it.each([
    [Number.MAX_VALUE, 0, Number.MAX_VALUE, 0],
    [-Number.MAX_VALUE, 0, -Number.MAX_VALUE, 0],
    [0, Number.MAX_VALUE, 0, Number.MAX_VALUE],
    [0, -Number.MAX_VALUE, 0, -Number.MAX_VALUE],
  ])('rejects coordinate overflow without changing layout or history (%s, %s)', (x, y, dx, dy) => {
    const document = doc({ nodes: [node({ id: 'n1', x, y })], edges: [] });
    const before: CanvasHistory = { document, past: [], future: [doc()] };
    expect(moveNode(document, 'n1', dx, dy)).toBe(document);
    expect(reduceCanvasHistory(before, {
      type: 'edit', change: (current) => moveNode(current, 'n1', dx, dy),
    })).toBe(before);
    expect(document.nodes[0]).toMatchObject({ x, y });
  });

  function initial(): CanvasHistory {
    return { document: doc(), past: [], future: [] };
  }

  it('restores the entire completed drag in one undo and one redo', () => {
    const before = initial();
    const drag = beginDrag(before.document.nodes[0]!, 0, 0);
    const after = reduceCanvasHistory(before, {
      type: 'edit', change: (current) => commitDrag(current, drag, { ...drag.origin, x: 50, y: 60 }),
    });
    expect(after.past).toHaveLength(1);
    expect(after.document.nodes[0]).toMatchObject({ x: 50, y: 60 });
    const undone = reduceCanvasHistory(after, { type: 'undo' });
    expect(undone.document).toBe(before.document);
    expect(undone.past).toHaveLength(0);
    const redone = reduceCanvasHistory(undone, { type: 'redo' });
    expect(redone.document).toBe(after.document);
    expect(redone.future).toHaveLength(0);
  });

  it('restores a removed card with its incident edges and unknown fields', () => {
    const before: CanvasHistory = { document: JSON.parse(canvasWire), past: [], future: [] };
    const after = reduceCanvasHistory(before, { type: 'edit', change: (current) => removeNodeLocally(current, 'n') });
    expect(after.document.nodes).toHaveLength(0);
    expect(after.document.edges).toHaveLength(0);
    expect(reduceCanvasHistory(after, { type: 'undo' }).document).toEqual(JSON.parse(canvasWire));
  });

  it('retains redo after a no-op but discards it after an actual new edit', () => {
    const before = initial();
    const after = reduceCanvasHistory(before, { type: 'edit', change: (current) => moveNode(current, 'n1', 10, 0) });
    const undone = reduceCanvasHistory(after, { type: 'undo' });
    expect(reduceCanvasHistory(undone, { type: 'edit', change: (current) => moveNode(current, 'ghost', 1, 0) })).toBe(undone);
    const changed = reduceCanvasHistory(undone, { type: 'edit', change: (current) => moveNode(current, 'n1', 0, 20) });
    expect(changed.future).toHaveLength(0);
    expect(reduceCanvasHistory(changed, { type: 'redo' })).toBe(changed);
    expect(reduceCanvasHistory(before, { type: 'undo' })).toBe(before);
  });
});

describe('connection geometry', () => {
  it('connects card boundaries rather than hiding the arrowhead under the target', () => {
    const original = doc();
    const view = connectionPaths(original);
    expect(view.paths[0]?.path).toMatch(/^M 240 80 C /);
    expect(view.paths[0]?.path).toMatch(/ 300 80$/);
    const vertical = connectionPaths(doc({ nodes: [node({ id: 'n1' }), node({ id: 'n2', y: 240 })] }));
    expect(vertical.paths[0]?.path).toMatch(/^M 120 160 C /);
    expect(vertical.paths[0]?.path).toMatch(/ 120 240$/);
  });

  it('follows a drag preview without changing the document and restores after cancellation', () => {
    const original = doc();
    const before = JSON.stringify(original);
    const preview = connectionPaths(original, { nodeId: 'n1', rect: { x: 40, y: 20, width: 240, height: 160 } });
    expect(preview.paths[0]?.path).toMatch(/^M 280 100 C /);
    expect(connectionPaths(original).paths[0]?.path).toMatch(/^M 240 80 C /);
    expect(JSON.stringify(original)).toBe(before);
  });

  it('renders self loops and separates opposite directions', () => {
    const loop = connectionPaths(doc({ edges: [{ id: 'loop', from: 'n1', to: 'n1' }] }));
    expect(loop.paths[0]?.path).toMatch(/^M 240 80 C /);
    expect(loop.paths[0]?.path).toMatch(/ 120 160$/);
    expect(loop.paths[0]?.path).not.toMatch(/NaN|Infinity/);
    const pair = connectionPaths(doc({ edges: [
      { id: 'forward', from: 'n1', to: 'n2' }, { id: 'back', from: 'n2', to: 'n1' },
    ] }));
    expect(pair.paths[0]?.path).toContain('104');
    expect(pair.paths[1]?.path).toContain('56');
  });

  it('covers offscreen targets and refuses to invent paths for missing or unsafe endpoints', () => {
    const far = connectionPaths(doc({ nodes: [node({ id: 'n1' }), node({ id: 'n2', x: 1800, y: 900 })] }));
    expect(far.width).toBeGreaterThan(2040);
    expect(far.height).toBeGreaterThan(1060);
    expect(connectionPaths(doc({ nodes: [] })).paths).toEqual([]);
    expect(connectionPaths(doc({ nodes: [node({ id: 'n1' }), node({ id: 'n2', x: Infinity })] })).paths).toEqual([]);
  });
});

describe('keyboard movement', () => {
  it('maps arrow keys to a step and Shift to the fine step', () => {
    // F30-R03: this is the drag alternative for keyboard users.
    expect(arrowDelta('ArrowRight', false)).toEqual({ dx: MOVE_STEP, dy: 0 });
    expect(arrowDelta('ArrowUp', false)).toEqual({ dx: 0, dy: -MOVE_STEP });
    expect(arrowDelta('ArrowLeft', true)).toEqual({ dx: -MOVE_FINE_STEP, dy: 0 });
    expect(arrowDelta('ArrowDown', true)).toEqual({ dx: 0, dy: MOVE_FINE_STEP });
    expect(arrowDelta('Enter', false)).toBeNull();
  });

  it('moves exactly one card and leaves size and edges alone', () => {
    const original = doc();
    const delta = arrowDelta('ArrowRight', false)!;
    const next = moveNode(original, 'n2', delta.dx, delta.dy);
    expect(next.nodes[1]).toMatchObject({
      x: 310,
      y: 0,
      width: 240,
      height: 160,
    });
    expect(next.nodes[0]).toEqual(original.nodes[0]);
    expect(next.edges).toEqual(original.edges);
  });

  it('returns the same document when nothing can move', () => {
    const original = doc();
    expect(moveNode(original, 'ghost', MOVE_STEP, 0)).toBe(original);
    expect(moveNode(original, 'n1', 0, 0)).toBe(original);
    expect(moveNode(original, 'n1', Number.NaN, 0)).toBe(original);
    expect(moveNode(original, 'n1', Number.POSITIVE_INFINITY, 0)).toBe(original);
  });

  it('keeps geometry whole-pixel after repeated moves', () => {
    let current = doc();
    for (let index = 0; index < 3; index += 1) {
      current = moveNode(current, 'n1', MOVE_FINE_STEP, -MOVE_STEP);
    }
    expect(current.nodes[0]).toMatchObject({ x: 3, y: -30 });
    expect(Number.isInteger(current.nodes[0]?.x)).toBe(true);
    expect(Number.isInteger(current.nodes[0]?.y)).toBe(true);
  });
});

describe('geometry', () => {
  it('clamps a resize to the documented minimum', () => {
    const rect = { x: 0, y: 0, width: 240, height: 160 };
    expect(resizeRect(rect, 10, 10, 'text')).toMatchObject({
      width: MIN_NODE_WIDTH,
      height: MIN_NODE_HEIGHT,
    });
  });

  it('lets a group be smaller than a content node', () => {
    const rect = { x: 0, y: 0, width: 240, height: 160 };
    expect(resizeRect(rect, 5, 5, 'group')).toMatchObject({ width: 5, height: 5 });
  });

  it('ignores a non-finite resize instead of corrupting the node', () => {
    const rect = { x: 0, y: 0, width: 240, height: 160 };
    expect(resizeRect(rect, Number.NaN, Number.POSITIVE_INFINITY, 'text')).toEqual(rect);
  });

  it('validates a rect the same way the document model does', () => {
    expect(isValidRect({ x: 0, y: 0, width: 240, height: 160 }, 'text')).toBe(true);
    expect(isValidRect({ x: 0, y: 0, width: 119, height: 160 }, 'text')).toBe(false);
    expect(isValidRect({ x: 0, y: 0, width: 10, height: 10 }, 'group')).toBe(true);
    expect(isValidRect({ x: Number.NaN, y: 0, width: 240, height: 160 }, 'text')).toBe(false);
  });
});

describe('removing and connecting', () => {
  it('removing a card drops its edges and leaves the note id untouched', () => {
    // F21-R02: only the view node goes away.
    const next = removeNodeLocally(doc(), 'n1');
    expect(next.nodes.map((entry) => entry.id)).toEqual(['n2']);
    expect(next.edges).toEqual([]);
  });

  it('removing an absent node returns the same document', () => {
    const original = doc();
    expect(removeNodeLocally(original, 'nope')).toBe(original);
  });

  it('connects only existing nodes and refuses a duplicate edge', () => {
    expect(connectNodes(doc(), 'e2', 'n1', 'n2').edges).toHaveLength(1);
    expect(connectNodes(doc(), 'e2', 'n1', 'ghost').edges).toHaveLength(1);
    const withEdge = connectNodes(doc({ edges: [] }), 'e2', 'n1', 'n2');
    expect(withEdge.edges).toHaveLength(1);
    expect(withEdge.edges[0]).toEqual({ id: 'e2', from: 'n1', to: 'n2' });
  });

  it('allows a self-loop', () => {
    const next = connectNodes(doc({ edges: [] }), 'e2', 'n1', 'n1');
    expect(next.edges).toHaveLength(1);
  });
});

describe('new cards', () => {
  it('places a new card to the right of existing content', () => {
    const created = newTextNode(doc(), 'n3', 'note');
    expect(created.x).toBe(300 + 240 + 24);
    expect(created.y).toBe(0);
    expect(created.width).toBeGreaterThanOrEqual(MIN_NODE_WIDTH);
    expect(created.height).toBeGreaterThanOrEqual(MIN_NODE_HEIGHT);
  });

  it('wraps to a new row instead of running off the canvas', () => {
    const wide = doc({
      nodes: [node({ id: 'n1', x: 1300, width: 240, height: 160 })],
    });
    const created = newTextNode(wide, 'n3', 'note');
    expect(created.x).toBe(0);
    expect(created.y).toBe(160 + 24);
  });

  it('starts at the origin on an empty canvas', () => {
    const created = newTextNode({ formatVersion: '1.0', nodes: [], edges: [] }, 'n1', 'x');
    expect(created.x).toBe(24);
    expect(created.y).toBe(0);
  });
});

describe('list alternative', () => {
  it('lists every node with its reference and broken state', () => {
    // A canvas that can only be read by dragging excludes keyboard users.
    const items = listItems(doc(), ['note-a']);
    expect(items.map((item) => item.id)).toEqual(['n1', 'n2']);
    expect(items[0]).toMatchObject({ reference: 'note-a', broken: false, kind: 'note' });
    expect(items[1]).toMatchObject({ reference: null, broken: false });
  });

  it('marks a card whose note is missing rather than hiding it', () => {
    const items = listItems(doc(), []);
    expect(items[0]).toMatchObject({ reference: 'note-a', broken: true });
  });

  it('truncates a long label to one short line', () => {
    const long = doc({
      nodes: [node({ id: 'n1', text: `first line\n${'x'.repeat(200)}` })],
    });
    expect(listItems(long, [])[0]?.label).toBe('first line');
  });
});

describe('broken references', () => {
  it('reassociates a missing note without losing the card label or edges', () => {
    // F21 §7 BROKEN_NODE: repair the reference, not the whole canvas.
    const original = doc({
      nodes: [
        node({ id: 'n1', noteId: 'gone', type: 'note', label: 'Kept title' }),
        node({ id: 'n2', x: 300, text: 'hello' }),
      ],
    });
    expect(listItems(original, [])[0]?.broken).toBe(true);
    const next = reassociateNode(original, 'n1', 'note-b');
    expect(next.nodes[0]).toMatchObject({ noteId: 'note-b', label: 'Kept title' });
    expect(next.edges).toEqual(original.edges);
    expect(listItems(next, ['note-b'])[0]?.broken).toBe(false);
  });

  it('ignores an empty target, an unknown card and a no-op target', () => {
    const original = doc();
    expect(reassociateNode(original, 'n1', '  ')).toBe(original);
    expect(reassociateNode(original, 'ghost', 'note-b')).toBe(original);
    expect(reassociateNode(original, 'n1', 'note-a')).toBe(original);
  });

  it('never turns a text or group card into a reference', () => {
    const original = doc();
    expect(reassociateNode(original, 'n2', 'note-b')).toBe(original);
  });
});

describe('export summary', () => {
  it('reports what the export will contain', () => {
    expect(exportSummary(doc())).toEqual({ nodes: 2, edges: 1, formatVersion: '1.0' });
  });
});

describe('canvas files', () => {
  const doc = (id: string): CanvasDocument => ({ formatVersion: '1.0', nodes: [{ id, type: 'text', x: 0, y: 0, width: 200, height: 100, text: id }], edges: [] } as unknown as CanvasDocument);

  it('loading a file replaces the document and clears undo history', () => {
    let history = reduceCanvasHistory({ document: doc('a'), past: [], future: [] }, { type: 'edit', change: () => doc('b') });
    expect(history.past).toHaveLength(1);
    history = reduceCanvasHistory(history, { type: 'load', document: doc('disk') });
    expect(history).toEqual({ document: doc('disk'), past: [], future: [] });
  });

  it('turns a typed name into a .canvas path and rejects traversal', () => {
    expect(canvasPathFromName(' Boards/Plan ')).toBe('Boards/Plan.canvas');
    expect(canvasPathFromName('x.CANVAS')).toBe('x.CANVAS');
    expect(canvasPathFromName('a\\b')).toBe('a/b.canvas');
    expect(canvasPathFromName('')).toBeNull();
    expect(canvasPathFromName('../up')).toBeNull();
    expect(canvasPathFromName('a//b')).toBeNull();
  });

  it('reports save state without calling an unsaved file saved', () => {
    expect(canvasSaveState(null, false, false)).toBe('unsaved');
    expect(canvasSaveState({ path: 'a.canvas' }, true, false)).toBe('dirty');
    expect(canvasSaveState({ path: 'a.canvas' }, false, false)).toBe('saved');
    expect(canvasSaveState({ path: 'a.canvas' }, true, true)).toBe('saving');
  });
});

describe('pickConnection', () => {
  it('takes the first card as the start and connects on the next one', async () => {
    const { pickConnection } = await import('./canvas-view');
    const first = pickConnection(null, 'a');
    expect(first).toEqual({ start: 'a', connect: null });
    expect(pickConnection(first.start, 'b')).toEqual({ start: null, connect: { from: 'a', to: 'b' } });
  });

  it('cancels when the start card is picked again', async () => {
    const { pickConnection } = await import('./canvas-view');
    expect(pickConnection('a', 'a')).toEqual({ start: null, connect: null });
  });
});
