import type { GraphEdge, GraphNode, GraphReport } from '../editor';

/**
 * S22/S23 graph view logic.
 *
 * F15 asks for a graph that never invents an edge, that keeps a node's identity
 * when a title changes, and that is usable without a mouse. So the layout here is
 * a deterministic circular arrangement rather than a force simulation: it needs
 * no animation frames, produces identical output for identical input, and every
 * node has a stable position the list alternative can mirror (F15-R04).
 */

export interface PositionedNode extends GraphNode {
  x: number;
  y: number;
}

export interface PositionedEdge {
  from: string;
  to: string;
  kind: 'link' | 'embed';
  /** True when either end is a missing target (F15-R03). */
  dangling: boolean;
}

export interface GraphLayout {
  nodes: PositionedNode[];
  edges: PositionedEdge[];
  /** Radius used, so a caller can size its viewport. */
  radius: number;
  /** Node ids by depth, so the list view can group them. */
  byDepth: { depth: number; ids: string[] }[];
}

export const GRAPH_RADIUS = 220;
export const NODE_RADIUS = 7;

/**
 * Places nodes on concentric circles by hop distance.
 *
 * The anchor sits at the centre; each depth ring is evenly spaced starting at
 * the top. Even spacing is what makes the result stable: a force layout would
 * move nodes on every read and make the list alternative disagree with the
 * picture.
 */
export function layoutGraph(report: GraphReport): GraphLayout {
  const nodes = [...report.nodes];
  const depths = [...new Set(nodes.map((node) => node.depth))].sort((a, b) => a - b);
  const positioned: PositionedNode[] = [];
  for (const depth of depths) {
    const ring = nodes.filter((node) => node.depth === depth);
    if (depth === 0 && ring.length === 1 && ring[0]?.anchor === true) {
      // A single anchor is the centre, not a ring of one.
      const anchor = ring[0];
      positioned.push({ ...anchor, x: 0, y: 0 });
      continue;
    }
    const radius = depth === 0 ? 0 : (GRAPH_RADIUS * depth) / Math.max(1, depths.length - 1);
    ring.forEach((node, index) => {
      const angle = (index / ring.length) * Math.PI * 2 - Math.PI / 2;
      positioned.push({
        ...node,
        x: Math.cos(angle) * radius,
        y: Math.sin(angle) * radius,
      });
    });
  }
  const known = new Map(positioned.map((node) => [node.id, node]));
  const edges: PositionedEdge[] = [];
  for (const edge of report.edges) {
    const from = known.get(edge.from);
    const to = known.get(edge.to);
    if (from === undefined || to === undefined) {
      // An edge to a node the limit excluded is dropped rather than drawn to
      // nowhere; `truncated` already tells the user the view is partial.
      continue;
    }
    edges.push({
      from: edge.from,
      to: edge.to,
      kind: edge.kind,
      dangling: from.kind === 'missing' || to.kind === 'missing',
    });
  }
  return {
    nodes: positioned,
    edges,
    radius: GRAPH_RADIUS,
    byDepth: depths.map((depth) => ({
      depth,
      ids: nodes.filter((node) => node.depth === depth).map((node) => node.id),
    })),
  };
}

/** A straight line between two positioned nodes, for the SVG path. */
export function edgeLine(from: PositionedNode, to: PositionedNode): string {
  return `M ${from.x.toFixed(2)} ${from.y.toFixed(2)} L ${to.x.toFixed(2)} ${to.y.toFixed(2)}`;
}

/** Filters the graph by title text; matching is case-insensitive. */
export function filterNodes(nodes: readonly GraphNode[], query: string): GraphNode[] {
  const trimmed = query.trim().toLocaleLowerCase();
  if (trimmed === '') {
    return [...nodes];
  }
  return nodes.filter(
    (node) =>
      node.title.toLocaleLowerCase().includes(trimmed) ||
      node.relativePath.toLocaleLowerCase().includes(trimmed),
  );
}

/**
 * Keeps the nodes that match plus the edges between them.
 *
 * A filter that hid a node while drawing its edge would show a relationship to
 * nothing, so the edge set is derived from the surviving nodes.
 */
export function filterReport(report: GraphReport, query: string): GraphReport {
  const nodes = filterNodes(report.nodes, query);
  const ids = new Set(nodes.map((node) => node.id));
  const edges = report.edges.filter(
    (edge: GraphEdge) => ids.has(edge.from) && ids.has(edge.to),
  );
  return { ...report, nodes, edges };
}

/** The depth actually shown, for the "showing 1–N hops" label (F15 §4). */
export function shownDepth(report: GraphReport): number {
  let deepest = 0;
  for (const node of report.nodes) {
    deepest = Math.max(deepest, node.depth);
  }
  return deepest;
}

/**
 * A plain-text summary for the list alternative and the screen reader.
 *
 * F15-R04 requires a non-visual path, so the same numbers the picture shows are
 * available as text.
 */
export interface GraphSummary {
  notes: number;
  missing: number;
  links: number;
  embeds: number;
  truncated: boolean;
  deepest: number;
}

export function summarizeGraph(report: GraphReport): GraphSummary {
  let notes = 0;
  let missing = 0;
  for (const node of report.nodes) {
    if (node.kind === 'missing') {
      missing += 1;
    } else {
      notes += 1;
    }
  }
  let links = 0;
  let embeds = 0;
  for (const edge of report.edges) {
    if (edge.kind === 'embed') {
      embeds += 1;
    } else {
      links += 1;
    }
  }
  return {
    notes,
    missing,
    links,
    embeds,
    truncated: report.truncated,
    deepest: shownDepth(report),
  };
}

/**
 * Neighbours of one node, for the local graph preview.
 *
 * Direction is preserved so the caller can say "links to" versus "linked from".
 */
export function neighbours(
  report: GraphReport,
  noteId: string,
): { outgoing: string[]; incoming: string[] } {
  const outgoing: string[] = [];
  const incoming: string[] = [];
  for (const edge of report.edges) {
    if (edge.from === noteId) {
      outgoing.push(edge.to);
    }
    if (edge.to === noteId) {
      incoming.push(edge.from);
    }
  }
  return { outgoing: [...new Set(outgoing)], incoming: [...new Set(incoming)] };
}
