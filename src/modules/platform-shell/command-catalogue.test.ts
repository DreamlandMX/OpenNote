import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import {
  BINDABLE_COMMANDS,
  BINDABLE_COMMAND_IDS,
  bindableCommand,
  isBindableCommandId,
} from './command-catalogue';
import { ShortcutSettings } from './ShortcutSettings';
import { DEFAULT_BINDINGS, parseBindings } from './shortcuts';

describe('bindable command catalogue (F30-R01)', () => {
  it('registers each command once with a name and a destination', () => {
    expect(new Set(BINDABLE_COMMAND_IDS).size).toBe(BINDABLE_COMMANDS.length);
    const translator = new LocaleContext('en').translator;
    for (const command of BINDABLE_COMMANDS) {
      expect(translator(command.labelKey).trim()).not.toBe('');
      expect(bindableCommand(command.id)).toBe(command);
      expect(isBindableCommandId(command.id)).toBe(true);
    }
    expect(isBindableCommandId('workspace.unknown')).toBe(false);
    expect(isBindableCommandId('toString')).toBe(false);
    expect(isBindableCommandId('__proto__')).toBe(false);
  });

  it('keeps every dispatch destination in the catalogue', () => {
    expect(BINDABLE_COMMANDS.map(({ id, surface }) => `${id}:${surface}`)).toEqual([
      'workspace.search:search',
      'workspace.quickOpen:quickOpen',
      'workspace.commandPalette:palette',
      'workspace.export:export',
      'workspace.import:import',
      'workspace.newNote:create',
      'workspace.rename:rename',
      'editor.find:editor',
      'editor.replace:editor',
    ]);
  });

  it('loads stored bindings only for catalogued commands', () => {
    for (const id of BINDABLE_COMMAND_IDS) {
      expect(parseBindings({ [id]: 'Ctrl+Alt+K' })).toEqual({
        [id]: [{ modifiers: ['ctrl', 'alt'], key: 'k' }],
      });
    }
    expect(parseBindings({ 'workspace.unknown': 'Ctrl+Alt+K' })).toEqual({});
    expect(parseBindings({ toString: 'Ctrl+Alt+K', constructor: 'Ctrl+Alt+K' })).toEqual({});
  });

  it('defines defaults only for catalogued commands', () => {
    for (const id of Object.keys(DEFAULT_BINDINGS)) {
      expect(isBindableCommandId(id)).toBe(true);
    }
  });

  it('offers exactly one settings row per catalogued command', () => {
    const html = renderToStaticMarkup(
      createElement(ShortcutSettings, {
        bindings: {},
        translator: new LocaleContext('en').translator,
        onChange: () => undefined,
        onClose: () => undefined,
      }),
    );
    expect(html.split('<li class="shortcut-row">').length - 1).toBe(BINDABLE_COMMANDS.length);
  });
});
