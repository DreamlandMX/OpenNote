import type { MessageKey } from '../presentation-i18n';

/**
 * The one registry for rebindable commands (F30-R01).
 *
 * A command's stable id, localized name and the workspace surface it opens are
 * declared together. The settings panel, the header menu and the shortcut model
 * all read this list, so a command cannot exist in one place with a different
 * name, destination or set of keys.
 */

/** The workspace surface a rebindable command opens. */
export type CommandSurface =
  | 'quickOpen'
  | 'palette'
  | 'search'
  | 'export'
  | 'import'
  | 'create'
  | 'rename'
  /** Handled by the focused editor or reading pane, not the workspace. */
  | 'editor';

export const BINDABLE_COMMANDS = [
  { id: 'workspace.search', labelKey: 'search.title', surface: 'search' },
  { id: 'workspace.quickOpen', labelKey: 'quickOpen.title', surface: 'quickOpen' },
  { id: 'workspace.commandPalette', labelKey: 'commandPalette.title', surface: 'palette' },
  { id: 'workspace.export', labelKey: 'export.title', surface: 'export' },
  { id: 'workspace.import', labelKey: 'import.title', surface: 'import' },
  { id: 'workspace.newNote', labelKey: 'workspace.create.heading', surface: 'create' },
  { id: 'workspace.rename', labelKey: 'rename.title', surface: 'rename' },
  { id: 'editor.find', labelKey: 'find.command.find', surface: 'editor' },
  { id: 'editor.replace', labelKey: 'find.command.replace', surface: 'editor' },
] as const satisfies readonly {
  readonly id: string;
  readonly labelKey: MessageKey;
  readonly surface: CommandSurface;
}[];

/** Stable command ids the settings screen can rebind. */
export type BindableCommandId = (typeof BINDABLE_COMMANDS)[number]['id'];

export interface BindableCommandDefinition {
  readonly id: BindableCommandId;
  readonly labelKey: MessageKey;
  readonly surface: CommandSurface;
}

export const BINDABLE_COMMAND_IDS: readonly BindableCommandId[] = BINDABLE_COMMANDS.map(
  (command) => command.id,
);

const BY_ID = Object.fromEntries(
  BINDABLE_COMMANDS.map((command) => [command.id, command]),
) as Record<BindableCommandId, BindableCommandDefinition>;

export function isBindableCommandId(value: string): value is BindableCommandId {
  return Object.prototype.hasOwnProperty.call(BY_ID, value);
}

export function bindableCommand(id: BindableCommandId): BindableCommandDefinition {
  return BY_ID[id];
}
