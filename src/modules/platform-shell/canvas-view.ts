import type { CanvasDocument, CanvasNode } from '../editor';

/**
 * S30/S31 canvas interaction model.
 *
 * The document itself is parsed, validated and serialised in M06; this module
 * owns the editing rules that must stay consistent between the canvas and any
 * list or keyboard path:
 *
 * * a drag becomes exactly one change, so it can be undone in one step
 *   (F21-R03);
 * * geometry can never leave the validated range, even during a drag;
 * * removing a card never touches a note (F21-R02) — the document only holds the
 *   reference id.
 */

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface CanvasHistory {
  document: CanvasDocument;
  past: readonly CanvasDocument[];
  future: readonly CanvasDocument[];
}

export type CanvasHistoryAction =
  | { type: 'edit'; change(document: CanvasDocument): CanvasDocument }
  | { type: 'undo' }
  | { type: 'redo' }
  /** Replaces the document with one read from disk; history starts fresh. */
  | { type: 'load'; document: CanvasDocument };

export function reduceCanvasHistory(history: CanvasHistory, action: CanvasHistoryAction): CanvasHistory {
  if (action.type === 'load') {
    return { document: action.document, past: [], future: [] };
  }
  if (action.type === 'edit') {
    const document = action.change(history.document);
    return document === history.document ? history : {
      document, past: [...history.past, history.document], future: [],
    };
  }
  if (action.type === 'undo') {
    const document = history.past.at(-1);
    return document === undefined ? history : {
      document, past: history.past.slice(0, -1), future: [history.document, ...history.future],
    };
  }
  const document = history.future[0];
  return document === undefined ? history : {
    document, past: [...history.past, history.document], future: history.future.slice(1),
  };
}

/** F21 §4 minimum for a content node, mirrored so a drag cannot cross it. */
export const MIN_NODE_WIDTH = 120;
export const MIN_NODE_HEIGHT = 80;

/** Keyboard movement of a focused card, in logical pixels (F30-R03). */
export const MOVE_STEP = 10;
export const MOVE_FINE_STEP = 1;

/** A drag that has started but not yet been committed. */
export interface DragState {
  nodeId: string;
  /** Pointer offset from the node origin when the drag began. */
  grabX: number;
  grabY: number;
  /** The node's geometry before the drag began, for cancel and for undo. */
  origin: Rect;
}

export function beginDrag(node: CanvasNode, pointerX: number, pointerY: number): DragState {
  return {
    nodeId: node.id,
    grabX: pointerX - node.x,
    grabY: pointerY - node.y,
    origin: { x: node.x, y: node.y, width: node.width, height: node.height },
  };
}

/**
 * The geometry a drag currently implies.
 *
 * Coordinates are rounded to whole pixels because a canvas is a layout, not a
 * measurement: sub-pixel drift makes two identical drags produce different files.
 */
export function dragRect(drag: DragState, pointerX: number, pointerY: number): Rect {
  return {
    x: Math.round(pointerX - drag.grabX),
    y: Math.round(pointerY - drag.grabY),
    width: drag.origin.width,
    height: drag.origin.height,
  };
}

/** Clamps a resize to the documented minimum and to finite values. */
export function resizeRect(rect: Rect, width: number, height: number, kind: string): Rect {
  const limit = kind === 'group' ? 1 : MIN_NODE_WIDTH;
  const limitHeight = kind === 'group' ? 1 : MIN_NODE_HEIGHT;
  const nextWidth = Number.isFinite(width) ? Math.max(limit, Math.round(width)) : rect.width;
  const nextHeight = Number.isFinite(height) ? Math.max(limitHeight, Math.round(height)) : rect.height;
  return { ...rect, width: nextWidth, height: nextHeight };
}

/** True when the geometry is acceptable to store. */
export function isValidRect(rect: Rect, kind: string): boolean {
  if (!Number.isFinite(rect.x) || !Number.isFinite(rect.y)) {
    return false;
  }
  if (!Number.isFinite(rect.width) || !Number.isFinite(rect.height)) {
    return false;
  }
  if (kind === 'group') {
    return rect.width > 0 && rect.height > 0;
  }
  return rect.width >= MIN_NODE_WIDTH && rect.height >= MIN_NODE_HEIGHT;
}

/**
 * Applies a finished drag to the document.
 *
 * A drag that produced no movement returns the same document object, so the
 * caller can skip an undo entry and a save entirely (F21-R03).
 */
export function commitDrag(
  document: CanvasDocument,
  drag: DragState,
  rect: Rect,
): CanvasDocument {
  const node = document.nodes.find((candidate) => candidate.id === drag.nodeId);
  if (node === undefined || !isValidRect(rect, node.type)
    || node.x !== drag.origin.x || node.y !== drag.origin.y
    || node.width !== drag.origin.width || node.height !== drag.origin.height) {
    return document;
  }
  if (
    drag.origin.x === rect.x &&
    drag.origin.y === rect.y &&
    drag.origin.width === rect.width &&
    drag.origin.height === rect.height
  ) {
    return document;
  }
  return {
    ...document,
    nodes: document.nodes.map((node) =>
      node.id === drag.nodeId ? { ...node, ...rect } : node,
    ),
  };
}

/**
 * The movement an arrow key implies for a focused card.
 *
 * Shift is the fine step, matching the conventional canvas modifier. Any other
 * key returns null so the caller leaves normal focus navigation alone.
 */
export function arrowDelta(key: string, fine: boolean): { dx: number; dy: number } | null {
  const step = fine ? MOVE_FINE_STEP : MOVE_STEP;
  if (key === 'ArrowLeft') {
    return { dx: -step, dy: 0 };
  }
  if (key === 'ArrowRight') {
    return { dx: step, dy: 0 };
  }
  if (key === 'ArrowUp') {
    return { dx: 0, dy: -step };
  }
  if (key === 'ArrowDown') {
    return { dx: 0, dy: step };
  }
  return null;
}

/**
 * Moves one card by a whole-pixel delta (F30-R03).
 *
 * This is the keyboard alternative to a pointer drag: it changes only x/y, so
 * size rules are untouched, and a no-op returns the same document so the caller
 * can skip a change.
 */
export function moveNode(
  document: CanvasDocument,
  nodeId: string,
  dx: number,
  dy: number,
): CanvasDocument {
  if (!Number.isFinite(dx) || !Number.isFinite(dy)) {
    return document;
  }
  const node = document.nodes.find((candidate) => candidate.id === nodeId);
  if (node === undefined) {
    return document;
  }
  const x = Math.round(node.x + dx);
  const y = Math.round(node.y + dy);
  if (!isValidRect({ ...node, x, y }, node.type)) {
    return document;
  }
  if (x === node.x && y === node.y) {
    return document;
  }
  return {
    ...document,
    nodes: document.nodes.map((candidate) =>
      candidate.id === nodeId ? { ...candidate, x, y } : candidate,
    ),
  };
}

/** The document with one node removed and its edges dropped. */
export function removeNodeLocally(document: CanvasDocument, nodeId: string): CanvasDocument {
  const next = document.nodes.filter((node) => node.id !== nodeId);
  if (next.length === document.nodes.length) {
    return document;
  }
  return {
    ...document,
    nodes: next,
    edges: document.edges.filter((edge) => edge.from !== nodeId && edge.to !== nodeId),
  };
}

/** A new text card placed so it does not overlap an existing node. */
export function newTextNode(
  document: CanvasDocument,
  id: string,
  text: string,
): CanvasNode {
  // Place to the right of everything, then wrap to a new row when it would drift
  // far; a fixed cascade would stack overlapping cards.
  const rightmost = document.nodes.reduce(
    (max, node) => Math.max(max, node.x + node.width),
    0,
  );
  const rowBottom = document.nodes.reduce(
    (max, node) => Math.max(max, node.y + node.height),
    0,
  );
  const farRight = rightmost > 1200;
  return {
    id,
    type: 'text',
    x: farRight ? 0 : Math.round(rightmost + 24),
    y: farRight ? Math.round(rowBottom + 24) : 0,
    width: MIN_NODE_WIDTH * 2,
    height: MIN_NODE_HEIGHT * 1.5,
    text,
  };
}

/**
 * One click in connect mode. The first card picked becomes the start; the next
 * distinct card completes a connection from it. Picking the start again cancels
 * it. Either way a completed or cancelled pick leaves no start.
 */
export function pickConnection(
  start: string | null,
  nodeId: string,
): { start: string | null; connect: { from: string; to: string } | null } {
  if (start === null) return { start: nodeId, connect: null };
  return { start: null, connect: start === nodeId ? null : { from: start, to: nodeId } };
}

/** Adds an edge only when both endpoints exist and it is not a duplicate. */
export function connectNodes(
  document: CanvasDocument,
  id: string,
  from: string,
  to: string,
): CanvasDocument {
  const has = (nodeId: string): boolean => document.nodes.some((node) => node.id === nodeId);
  if (!has(from) || !has(to)) {
    return document;
  }
  const duplicate = document.edges.some(
    (edge) => edge.from === from && edge.to === to,
  );
  if (duplicate) {
    return document;
  }
  // Edge kind is not part of the port shape: a canvas edge is a connection, and
  // whether it was an embed is a note-level fact, not a canvas-level one.
  return { ...document, edges: [...document.edges, { id, from, to }] };
}

/**
 * Repairs a broken reference card in memory (F21 §7 BROKEN_NODE).
 *
 * Only the reference id changes; the card keeps its label, geometry and edges,
 * and no note is read or written. An unknown or non-reference node leaves the
 * document untouched so a stray id cannot retarget a text or group card.
 */
export function reassociateNode(
  document: CanvasDocument,
  nodeId: string,
  noteId: string,
): CanvasDocument {
  if (noteId.trim() === '') {
    return document;
  }
  const node = document.nodes.find((candidate) => candidate.id === nodeId);
  if (node === undefined || node.type !== 'note' || node.noteId === noteId) {
    return document;
  }
  return {
    ...document,
    nodes: document.nodes.map((candidate) =>
      candidate.id === nodeId ? { ...candidate, noteId } : candidate,
    ),
  };
}

/** A note the canvas can reference, with the title shown on a card. */
export interface CanvasNoteChoice {
  noteId: string;
  title: string;
}

/** A canvas node as the list alternative shows it. */
export interface CanvasListItem {
  id: string;
  kind: string;
  label: string;
  reference: string | null;
  /** True when the node's referenced note is missing (BROKEN_NODE). */
  broken: boolean;
}

/**
 * The list alternative to the canvas.
 *
 * F21 requires the same content to be reachable without a pointer; a canvas that
 * can only be explored by dragging excludes keyboard and screen-reader users.
 */
export function listItems(
  document: CanvasDocument,
  knownNoteIds: readonly string[],
): CanvasListItem[] {
  return document.nodes.map((node) => {
    const broken =
      typeof node.noteId === 'string' && !knownNoteIds.includes(node.noteId);
    const label = node.label ?? node.text ?? node.noteId ?? node.id;
    return {
      id: node.id,
      kind: node.type,
      label: label.split(/\r?\n/)[0]?.slice(0, 80) ?? node.id,
      reference: node.noteId ?? null,
      broken,
    };
  });
}

/** Physical canvas coordinates; routing never moves user-positioned cards. */
export function connectionPaths(
  document: CanvasDocument,
  preview: { nodeId: string; rect: Rect } | null = null,
): { paths: { id: string; path: string }[]; width: number; height: number } {
  const nodes = document.nodes.map((node) => preview?.nodeId === node.id ? { ...node, ...preview.rect } : node);
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const pairs = new Set(document.edges.map((edge) => JSON.stringify([edge.from, edge.to])));
  const paths: { id: string; path: string }[] = [];
  let width = 1;
  let height = 1;
  for (const node of nodes) {
    const right = node.x + node.width + 64;
    const bottom = node.y + node.height + 64;
    if (Number.isFinite(right)) width = Math.max(width, right);
    if (Number.isFinite(bottom)) height = Math.max(height, bottom);
  }
  for (const edge of document.edges) {
    const from = byId.get(edge.from);
    const to = byId.get(edge.to);
    if (from === undefined || to === undefined || !isValidRect(from, from.type) || !isValidRect(to, to.type)) continue;
    const fx = from.x + from.width / 2;
    const fy = from.y + from.height / 2;
    const tx = to.x + to.width / 2;
    const ty = to.y + to.height / 2;
    const dx = tx - fx;
    const dy = ty - fy;
    let points: number[];
    if (edge.from === edge.to || (dx === 0 && dy === 0)) {
      points = [from.x + from.width, fy,
        Math.max(from.x + from.width, to.x + to.width) + 64, fy,
        tx, Math.max(from.y + from.height, to.y + to.height) + 64,
        tx, to.y + to.height];
    } else {
      const horizontal = Math.abs(dx) >= Math.abs(dy);
      const sign = (horizontal ? dx : dy) > 0 ? 1 : -1;
      const sx = horizontal ? fx + sign * from.width / 2 : fx;
      const sy = horizontal ? fy : fy + sign * from.height / 2;
      const ex = horizontal ? tx - sign * to.width / 2 : tx;
      const ey = horizontal ? ty : ty - sign * to.height / 2;
      const bend = Math.max(32, Math.min(160, Math.abs(horizontal ? ex - sx : ey - sy) / 2));
      const offset = pairs.has(JSON.stringify([edge.to, edge.from])) ? 24 : 0;
      const distance = Math.hypot(dx, dy);
      const ox = -dy / distance * offset;
      const oy = dx / distance * offset;
      // shortcut: curves do not avoid other cards; add obstacle routing with a dedicated layout feature.
      points = [sx, sy,
        sx + (horizontal ? sign * bend : 0) + ox, sy + (horizontal ? 0 : sign * bend) + oy,
        ex - (horizontal ? sign * bend : 0) + ox, ey - (horizontal ? 0 : sign * bend) + oy,
        ex, ey];
    }
    if (!points.every(Number.isFinite)) continue;
    paths.push({ id: edge.id, path: `M ${points[0]} ${points[1]} C ${points.slice(2).join(' ')}` });
  }
  return { paths, width, height };
}

/** A short export summary, so the UI can state what it will write. */
export function exportSummary(document: CanvasDocument): {
  nodes: number;
  edges: number;
  formatVersion: string;
} {
  return {
    nodes: document.nodes.length,
    edges: document.edges.length,
    formatVersion: document.formatVersion,
  };
}

/**
 * A canvas file name typed by the user, as a vault-relative `.canvas` path.
 * Returns null for an empty name or one with traversal; the host still runs
 * the authoritative path checks.
 */
export function canvasPathFromName(raw: string): string | null {
  const trimmed = raw.trim().replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  if (trimmed === '' || trimmed.split('/').some((part) => part === '' || part === '.' || part === '..')) {
    return null;
  }
  return trimmed.toLowerCase().endsWith('.canvas') ? trimmed : `${trimmed}.canvas`;
}

/** The save state shown next to the file name. */
export function canvasSaveState(
  file: { path: string } | null,
  dirty: boolean,
  saving: boolean,
): 'unsaved' | 'saving' | 'dirty' | 'saved' {
  if (saving) return 'saving';
  if (file === null) return 'unsaved';
  return dirty ? 'dirty' : 'saved';
}
