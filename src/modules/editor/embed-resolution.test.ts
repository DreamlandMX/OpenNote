import { describe, expect, it } from 'vitest';
import { embedTargetIds, resolveEmbedTree } from './embed-resolution';
import type { EmbedExpansion, ExpandEmbedRequest, ServiceOutcome } from './ports';

function expansion(overrides: Partial<EmbedExpansion> = {}): EmbedExpansion {
  return {
    status: 'ok',
    targetId: 'n2',
    targetPath: 'notes/b.md',
    anchor: null,
    content: 'body',
    chain: ['n1', 'n2'],
    ...overrides,
  };
}

function failure(): ServiceOutcome<EmbedExpansion> {
  return {
    ok: false,
    error: {
      code: 'IO_DENIED',
      messageKey: 'error.ioDenied',
      params: {},
      retryable: true,
      operationId: null,
      retryAfterSeconds: null,
    },
  };
}

describe('resolveEmbedTree', () => {
  it('resolves each unique target once and never chains siblings', async () => {
    const requests: ExpandEmbedRequest[] = [];
    const tree = await resolveEmbedTree(
      {
        expandNoteEmbed: async request => {
          requests.push(request);
          return {
            ok: true,
            value: expansion({
              targetId: `id-${request.target}`,
              targetPath: `notes/${request.target}.md`,
              content: `body of ${request.target}`,
              chain: ['n1', `id-${request.target}`],
            }),
          };
        },
      },
      'n1',
      '![[b]] ![[c]] ![[b]]',
    );
    expect(requests.map(request => [request.noteId, request.target, request.anchor ?? null])).toEqual([
      ['n1', 'b', null],
      ['n1', 'c', null],
    ]);
    // The regression this locks: the second request used to inherit the first
    // target's chain and was reported as a cycle / too deep. Siblings start
    // fresh, so no chain is sent at all.
    for (const request of requests) {
      expect('chain' in request).toBe(false);
    }
    expect(tree).toEqual({
      'b#': {
        expansion: expansion({
          targetId: 'id-b',
          targetPath: 'notes/b.md',
          content: 'body of b',
          chain: ['n1', 'id-b'],
        }),
        children: {},
      },
      'c#': {
        expansion: expansion({
          targetId: 'id-c',
          targetPath: 'notes/c.md',
          content: 'body of c',
          chain: ['n1', 'id-c'],
        }),
        children: {},
      },
    });
  });

  it('resolves nested embeds with the parent note and chain', async () => {
    const requests: ExpandEmbedRequest[] = [];
    const tree = await resolveEmbedTree(
      {
        expandNoteEmbed: async request => {
          requests.push(request);
          if (request.target === 'b') {
            return {
              ok: true,
              value: expansion({
                targetId: 'id-b',
                targetPath: 'notes/b.md',
                content: 'B says ![[c]]',
                chain: ['n1', 'id-b'],
              }),
            };
          }
          return {
            ok: true,
            value: expansion({
              targetId: 'id-c',
              targetPath: 'notes/c.md',
              content: 'leaf',
              chain: ['n1', 'id-b', 'id-c'],
            }),
          };
        },
      },
      'n1',
      '![[b]]',
    );
    expect(requests).toEqual([
      { noteId: 'n1', target: 'b', anchor: undefined, chain: undefined },
      { noteId: 'id-b', target: 'c', anchor: undefined, chain: ['n1', 'id-b'] },
    ]);
    expect(tree).toEqual({
      'b#': {
        expansion: expansion({
          targetId: 'id-b',
          targetPath: 'notes/b.md',
          content: 'B says ![[c]]',
          chain: ['n1', 'id-b'],
        }),
        children: {
          'c#': {
            expansion: expansion({
              targetId: 'id-c',
              targetPath: 'notes/c.md',
              content: 'leaf',
              chain: ['n1', 'id-b', 'id-c'],
            }),
            children: {},
          },
        },
      },
    });
  });

  it('keeps a cycle as a childless placeholder', async () => {
    const requests: ExpandEmbedRequest[] = [];
    const tree = await resolveEmbedTree(
      {
        expandNoteEmbed: async request => {
          requests.push(request);
          return {
            ok: true,
            value: expansion({
              status: 'cycle',
              targetId: 'n1',
              targetPath: 'notes/a.md',
              content: null,
              chain: ['n1'],
            }),
          };
        },
      },
      'n1',
      '![[a]]',
    );
    expect(requests).toHaveLength(1);
    expect(tree).toEqual({
      'a#': {
        expansion: expansion({
          status: 'cycle',
          targetId: 'n1',
          targetPath: 'notes/a.md',
          content: null,
          chain: ['n1'],
        }),
        children: {},
      },
    });
  });

  it('does not descend past a depth-exceeded expansion', async () => {
    const requests: ExpandEmbedRequest[] = [];
    const tree = await resolveEmbedTree(
      {
        expandNoteEmbed: async request => {
          requests.push(request);
          if (request.target === 'b') {
            return {
              ok: true,
              value: expansion({
                targetId: 'id-b',
                content: '![[c]]',
                chain: ['n1', 'id-b'],
              }),
            };
          }
          return {
            ok: true,
            value: expansion({
              status: 'depthExceeded',
              targetId: 'id-c',
              content: null,
              chain: ['n1', 'id-b', 'id-c'],
            }),
          };
        },
      },
      'n1',
      '![[b]]',
    );
    expect(requests.map(request => request.target)).toEqual(['b', 'c']);
    expect(tree['b#']?.children['c#']).toEqual({
      expansion: expansion({
        status: 'depthExceeded',
        targetId: 'id-c',
        content: null,
        chain: ['n1', 'id-b', 'id-c'],
      }),
      children: {},
    });
  });

  it('keeps a keyed null when a nested request fails so the row stops loading', async () => {
    const tree = await resolveEmbedTree(
      {
        expandNoteEmbed: async request =>
          request.target === 'b'
            ? {
                ok: true,
                value: expansion({
                  targetId: 'id-b',
                  content: '![[missing]]',
                  chain: ['n1', 'id-b'],
                }),
              }
            : failure(),
      },
      'n1',
      '![[b]]',
    );
    expect(tree['b#']?.children).toEqual({ 'missing#': null });
  });

  it('treats a rejected call as a finished failure', async () => {
    const tree = await resolveEmbedTree(
      {
        expandNoteEmbed: async () => {
          throw new Error('ipc unavailable');
        },
      },
      'n1',
      '![[b#x]]',
    );
    expect(tree).toEqual({ 'b#x': null });
  });

  it('does not call the service when the body has no embeds', async () => {
    let calls = 0;
    const tree = await resolveEmbedTree(
      {
        expandNoteEmbed: async () => {
          calls += 1;
          return { ok: true, value: expansion() };
        },
      },
      'n1',
      'plain body',
    );
    expect(tree).toEqual({});
    expect(calls).toBe(0);
  });

  it('stops descending when a host keeps answering ok without a growing chain', async () => {
    const requests: ExpandEmbedRequest[] = [];
    const tree = await resolveEmbedTree(
      {
        expandNoteEmbed: async request => {
          requests.push(request);
          return {
            ok: true,
            value: expansion({
              targetId: `id-${request.target}`,
              content: 'again ![[loop]]',
              chain: ['n1'],
            }),
          };
        },
      },
      'n1',
      '![[loop]]',
    );
    // level 1 is the open note's embed; levels 2 and 3 are allowed, level 4 is
    // not requested. A malformed host therefore cannot recurse forever.
    expect(requests).toHaveLength(3);
    const level3 = tree['loop#']?.children['loop#']?.children['loop#'];
    expect(level3?.children).toEqual({});
  });
});

describe('embedTargetIds', () => {
  it('collects top-level and nested source ids once', () => {
    const tree = {
      'b#': {
        expansion: expansion({ targetId: 'id-b' }),
        children: {
          'c#': {
            expansion: expansion({ targetId: 'id-c' }),
            children: {},
          },
        },
      },
      'd#': {
        expansion: expansion({ targetId: 'id-b' }),
        children: {},
      },
    };
    expect([...embedTargetIds(tree)].sort()).toEqual(['id-b', 'id-c']);
  });

  it('ignores failed requests and expansions without a source id', () => {
    const tree = {
      'missing#': null,
      'broken#': {
        expansion: expansion({ status: 'brokenLink', targetId: null, content: null }),
        children: { 'x#': null },
      },
      'ok#': {
        expansion: expansion({ targetId: 'id-ok' }),
        children: {},
      },
    };
    expect([...embedTargetIds(tree)]).toEqual(['id-ok']);
  });
});
