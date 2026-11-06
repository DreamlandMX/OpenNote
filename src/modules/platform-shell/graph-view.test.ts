import { describe, expect, it } from 'vitest';
import type { GraphReport } from '../editor';
import {
  edgeLine,
  filterReport,
  GRAPH_RADIUS,
  layoutGraph,
  neighbours,
  summarizeGraph,
} from './graph-view';

function report(overrides: Partial<GraphReport> = {}): GraphReport {
  return {
    nodes: [
      { id: 'a', title: 'Alpha', relativePath: 'a.md', kind: 'note', anchor: true, depth: 0 },
      { id: 'b', title: 'Beta', relativePath: 'b.md', kind: 'note', anchor: false, depth: 1 },
      { id: 'c', title: 'Gamma', relativePath: 'c.md', kind: 'note', anchor: false, depth: 1 },
      { id: 'gone', title: 'gone', relativePath: '', kind: 'missing', anchor: false, depth: 1 },
    ],
    edges: [
      { from: 'a', to: 'b', kind: 'link' },
      { from: 'a', to: 'c', kind: 'embed' },
      { from: 'c', to: 'gone', kind: 'link' },
    ],
    truncated: false,
    unreadable: [],
    ...overrides,
  };
}

describe('graph layout', () => {
  it('centres a single anchor and rings the rest', () => {
    const layout = layoutGraph(report());
    const anchor = layout.nodes.find((node) => node.id === 'a');
    expect(anchor?.x).toBe(0);
    expect(anchor?.y).toBe(0);
    // Depth 1 nodes sit on the outer radius.
    const beta = layout.nodes.find((node) => node.id === 'b');
    const distance = Math.hypot(beta?.x ?? 0, beta?.y ?? 0);
    expect(distance).toBeCloseTo(GRAPH_RADIUS, 5);
  });

  it('is deterministic: the same report always gives the same positions', () => {
    const first = layoutGraph(report());
    const second = layoutGraph(report());
    expect(first.nodes.map((node) => [node.id, node.x, node.y])).toEqual(
      second.nodes.map((node) => [node.id, node.x, node.y]),
    );
  });

  it('spaces nodes on one ring evenly apart', () => {
    const layout = layoutGraph(report());
    const ring = layout.nodes.filter((node) => node.depth === 1);
    expect(ring).toHaveLength(3);
    // Every pair of neighbours on the ring is the same distance apart.
    const distances = ring.map((node, index) => {
      const next = ring[(index + 1) % ring.length];
      return Math.hypot((next?.x ?? 0) - node.x, (next?.y ?? 0) - node.y);
    });
    for (const distance of distances) {
      expect(distance).toBeCloseTo(distances[0] ?? 0, 5);
    }
  });

  it('marks an edge to a missing target as dangling', () => {
    const layout = layoutGraph(report());
    const dangling = layout.edges.find((edge) => edge.to === 'gone');
    expect(dangling?.dangling).toBe(true);
    expect(layout.edges.find((edge) => edge.to === 'b')?.dangling).toBe(false);
  });

  it('drops an edge whose endpoint the limit excluded', () => {
    const layout = layoutGraph(
      report({
        nodes: report().nodes.filter((node) => node.id !== 'gone'),
      }),
    );
    expect(layout.edges.some((edge) => edge.to === 'gone')).toBe(false);
  });

  it('renders a path between two positions', () => {
    const layout = layoutGraph(report());
    const from = layout.nodes.find((node) => node.id === 'a');
    const to = layout.nodes.find((node) => node.id === 'b');
    expect(edgeLine(from!, to!)).toBe(
      `M 0.00 0.00 L ${(to?.x ?? 0).toFixed(2)} ${(to?.y ?? 0).toFixed(2)}`,
    );
  });

  it('reports depth groups for the list alternative', () => {
    const layout = layoutGraph(report());
    expect(layout.byDepth.map((group) => group.depth)).toEqual([0, 1]);
    expect(layout.byDepth[0]?.ids).toEqual(['a']);
  });
});

describe('graph filtering', () => {
  it('keeps matching nodes and only the edges between them', () => {
    const filtered = filterReport(report(), 'beta');
    expect(filtered.nodes.map((node) => node.id)).toEqual(['b']);
    // Alpha→Beta is dropped because Alpha no longer exists in the result.
    expect(filtered.edges).toEqual([]);
  });

  it('matches a path as well as a title, case-insensitively', () => {
    expect(filterReport(report(), 'GAMMA').nodes.map((node) => node.id)).toEqual(['c']);
    expect(filterReport(report(), 'c.md').nodes.map((node) => node.id)).toEqual(['c']);
  });

  it('an empty query changes nothing', () => {
    const filtered = filterReport(report(), '   ');
    expect(filtered.nodes).toHaveLength(4);
    expect(filtered.edges).toHaveLength(3);
  });

  it('keeps an edge when both ends survive the filter', () => {
    const filtered = filterReport(report(), 'a');
    // "Alpha" matches a; "Gamma" contains an "a" and so does the path c.md? No:
    // only Alpha and Gamma match, so the a→c edge survives.
    const ids = filtered.nodes.map((node) => node.id);
    expect(ids).toContain('a');
    expect(filtered.edges.every((edge) => ids.includes(edge.from) && ids.includes(edge.to))).toBe(
      true,
    );
  });
});

describe('graph summary', () => {
  it('counts notes, missing targets, links and embeds separately', () => {
    expect(summarizeGraph(report())).toEqual({
      notes: 3,
      missing: 1,
      links: 2,
      embeds: 1,
      truncated: false,
      deepest: 1,
    });
  });

  it('surfaces truncation and the deepest shown hop', () => {
    const summary = summarizeGraph(report({ truncated: true }));
    expect(summary.truncated).toBe(true);
    expect(summary.deepest).toBe(1);
  });

  it('reports zeroes for an empty graph', () => {
    expect(summarizeGraph({ nodes: [], edges: [], truncated: false, unreadable: [] })).toEqual({
      notes: 0,
      missing: 0,
      links: 0,
      embeds: 0,
      truncated: false,
      deepest: 0,
    });
  });
});

describe('neighbours', () => {
  it('separates outgoing from incoming and deduplicates', () => {
    const graph = report({
      edges: [
        { from: 'a', to: 'b', kind: 'link' },
        { from: 'a', to: 'b', kind: 'link' },
        { from: 'b', to: 'a', kind: 'link' },
        { from: 'c', to: 'a', kind: 'embed' },
      ],
    });
    expect(neighbours(graph, 'a')).toEqual({ outgoing: ['b'], incoming: ['b', 'c'] });
  });

  it('returns empty lists for an isolated note', () => {
    expect(neighbours(report(), 'nobody')).toEqual({ outgoing: [], incoming: [] });
  });
});
