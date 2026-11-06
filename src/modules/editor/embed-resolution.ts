import { embedRequests } from './embed-render';
import type { EmbedExpansion, ExpandEmbedRequest, NoteService } from './ports';

/**
 * One resolved expansion plus the embeds found inside its content.
 *
 * `children` is keyed `target#anchor`, the same key used at the top level, so a
 * renderer can resolve a nested span without knowing which note it came from.
 */
export interface EmbedNode {
  expansion: EmbedExpansion;
  children: Readonly<Record<string, EmbedNode | null>>;
}

/**
 * Resolved embeds for one note, keyed `target#anchor`.
 *
 * `null` means the request finished without an expansion (IPC failure). It is
 * distinct from a missing key, which means the request is still pending, so the
 * UI can stop claiming "loading" once it is known to have failed.
 */
export type EmbedTree = Readonly<Record<string, EmbedNode | null>>;

/**
 * Hard stop for malformed hosts that keep answering `ok` without growing the
 * chain. The backend already refuses a fourth level; this only guarantees that
 * a wrong answer cannot make the frontend recurse forever.
 */
const MAX_EMBED_LEVEL = 3;

/**
 * Expands one embed, then the embeds inside its content.
 *
 * The chain grows per content tree: the child request carries the parent's
 * chain, so a cycle or the depth limit is decided by the backend for this exact
 * path. Sibling embeds never share a chain.
 */
async function expandNode(
  service: Pick<NoteService, 'expandNoteEmbed'>,
  noteId: string,
  target: string,
  anchor: string | null,
  chain: readonly string[] | undefined,
  level: number,
): Promise<EmbedNode | null> {
  try {
    const request: ExpandEmbedRequest = { noteId, target };
    if (anchor !== null) {
      request.anchor = anchor;
    }
    if (chain !== undefined) {
      request.chain = [...chain];
    }
    const outcome = await service.expandNoteEmbed(request);
    if (!outcome.ok) {
      return null;
    }
    const expansion = outcome.value;
    if (
      expansion.status !== 'ok' ||
      expansion.content === null ||
      expansion.targetId === null
    ) {
      return { expansion, children: {} };
    }
    const children: Record<string, EmbedNode | null> = {};
    if (level < MAX_EMBED_LEVEL) {
      for (const request of embedRequests(expansion.content)) {
        const key = `${request.target}#${request.anchor ?? ''}`;
        children[key] = await expandNode(
          service,
          expansion.targetId,
          request.target,
          request.anchor,
          expansion.chain,
          level + 1,
        );
      }
    }
    return { expansion, children };
  } catch {
    // A rejected IPC call is a finished failure, not a pending load: keep the
    // key so the row shows the unavailable placeholder instead of spinning.
    return null;
  }
}

/**
 * Resolves the whole embed tree written in `body`.
 *
 * Top-level embeds are siblings of the open note and are sent without a chain;
 * nested embeds are resolved per parent content with that parent's chain. The
 * loop is sequential on purpose: it keeps request order matching the document
 * and avoids a burst of full-index reads on the host.
 */
export async function resolveEmbedTree(
  service: Pick<NoteService, 'expandNoteEmbed'>,
  noteId: string,
  body: string,
): Promise<EmbedTree> {
  const tree: Record<string, EmbedNode | null> = {};
  for (const request of embedRequests(body)) {
    const key = `${request.target}#${request.anchor ?? ''}`;
    tree[key] = await expandNode(
      service,
      noteId,
      request.target,
      request.anchor,
      undefined,
      1,
    );
  }
  return tree;
}

/**
 * Note ids that the resolved tree actually reads.
 *
 * A commit of one of these notes can change what is on screen, so the caller
 * re-resolves then. Unrelated saves are ignored instead of rebuilding the
 * whole tree, and notes that failed to expand carry no id to match.
 */
export function embedTargetIds(tree: EmbedTree): ReadonlySet<string> {
  const ids = new Set<string>();
  const visit = (node: EmbedNode | null): void => {
    if (node === null) {
      return;
    }
    if (node.expansion.targetId !== null) {
      ids.add(node.expansion.targetId);
    }
    for (const child of Object.values(node.children)) {
      visit(child);
    }
  };
  for (const node of Object.values(tree)) {
    visit(node);
  }
  return ids;
}
