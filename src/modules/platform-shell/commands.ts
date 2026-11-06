import type { MessageKey } from '../presentation-i18n';

/**
 * Stable command identifiers (F13 §4). The localized label may change with the
 * interface language; the id never does, so a recent-command list stays valid
 * after a language switch (F13-A01).
 */
export type CommandId =
  | 'app.home'
  | 'app.settings'
  | 'workspace.search'
  | 'workspace.newNote'
  | 'workspace.duplicateNote'
  | 'workspace.rename'
  | 'workspace.drafts'
  | 'workspace.history'
  | 'workspace.metadata'
  | 'workspace.trash'
  | 'workspace.backup'
  | 'workspace.export'
  | 'workspace.delete'
  | 'editor.mode.live'
  | 'editor.mode.source'
  | 'editor.mode.read';

/** What the workspace can do right now. Commands never guess from rendered UI. */
export interface CommandAvailability {
  hasActiveNote: boolean;
  writable: boolean;
  /** A blocking surface (composition, create, restore, backup, trash) is busy. */
  busy: boolean;
  canDelete: boolean;
  canDuplicate: boolean;
}

export interface CommandDefinition {
  id: CommandId;
  labelKey: MessageKey;
  /** Stable English aliases so a user can type `search` in any interface locale. */
  aliases: readonly string[];
  /** `null` means enabled; otherwise the message key that explains why not. */
  enabledReason(availability: CommandAvailability): MessageKey | null;
  /** Runs only after `enabledReason` returned null and the caller checked it. */
  run(): void;
}

export interface CommandMatch {
  command: CommandDefinition;
  label: string;
  reason: MessageKey | null;
}

export interface CommandRegistry {
  readonly all: readonly CommandDefinition[];
  search(
    query: string,
    translate: (key: MessageKey) => string,
    availability: CommandAvailability,
  ): CommandMatch[];
}

function normalize(value: string): string {
  return value.trim().toLocaleLowerCase();
}

function scoreText(value: string, query: string): number | null {
  const haystack = normalize(value);
  const needle = normalize(query);
  if (needle === '') {
    return 0;
  }
  if (haystack === needle) {
    return 0;
  }
  if (haystack.startsWith(needle)) {
    return 1;
  }
  const index = haystack.indexOf(needle);
  if (index === 0) {
    return 1;
  }
  return index > 0 ? 2 : null;
}

/**
 * Builds the palette catalogue. Registration is a one-time list: a duplicate id
 * is a programming error, not a merge, because two handlers for one stable id
 * would make Enter depend on array order (F13-R01/R06).
 */
export function createCommandRegistry(
  definitions: readonly CommandDefinition[],
): CommandRegistry {
  const seen = new Set<CommandId>();
  for (const definition of definitions) {
    if (seen.has(definition.id)) {
      throw new Error(`duplicate command id: ${definition.id}`);
    }
    seen.add(definition.id);
  }
  return {
    all: definitions,
    search(query, translate, availability) {
      const needle = normalize(query);
      const matches: { match: CommandMatch; score: number }[] = [];
      for (const command of definitions) {
        const label = translate(command.labelKey);
        let best: number | null = null;
        for (const candidate of [label, ...command.aliases, command.id]) {
          const score = scoreText(candidate, needle);
          if (score !== null && (best === null || score < best)) {
            best = score;
          }
        }
        if (best === null) {
          continue;
        }
        matches.push({
          score: best,
          match: {
            command,
            label,
            reason: command.enabledReason(availability),
          },
        });
      }
      matches.sort((left, right) => {
        if (left.score !== right.score) {
          return left.score - right.score;
        }
        return left.match.command.id.localeCompare(right.match.command.id);
      });
      return matches.map((entry) => entry.match);
    },
  };
}

/** Recent first, then the stable registration order; unknown ids are ignored. */
export function orderByRecent<T>(
  items: readonly T[],
  recentIds: readonly string[],
  idOf: (item: T) => string,
): T[] {
  const byId = new Map(items.map((item) => [idOf(item), item] as const));
  const ordered: T[] = [];
  for (const id of recentIds) {
    const item = byId.get(id);
    if (item !== undefined && !ordered.includes(item)) {
      ordered.push(item);
    }
  }
  for (const item of items) {
    if (!ordered.includes(item)) {
      ordered.push(item);
    }
  }
  return ordered;
}
