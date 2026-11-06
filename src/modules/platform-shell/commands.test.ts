import { describe, expect, it } from 'vitest';
import {
  createCommandRegistry,
  orderByRecent,
  type CommandAvailability,
  type CommandDefinition,
} from './commands';
import type { MessageKey } from '../presentation-i18n';

const available: CommandAvailability = {
  hasActiveNote: true,
  writable: true,
  busy: false,
  canDelete: true,
  canDuplicate: true,
};

function command(
  id: CommandDefinition['id'],
  labelKey: MessageKey,
  aliases: readonly string[] = [],
  reason: CommandDefinition['enabledReason'] = () => null,
): CommandDefinition {
  return { id, labelKey, aliases, enabledReason: reason, run: () => undefined };
}

function translate(key: MessageKey): string {
  const labels: Partial<Record<MessageKey, string>> = {
    'search.title': '搜索',
    'shell.nav.settings': '设置',
    'backup.title': '备份',
    'workspace.select': '选择一篇笔记',
    'editor.mode.read': '阅读',
    'editor.draft.blocked': '请先保存当前编辑',
    'workspace.vault.readOnly': '此知识库为只读',
  };
  return labels[key] ?? key;
}

describe('command registry', () => {
  it('matches localized labels, stable ids and English aliases with ranking', () => {
    const registry = createCommandRegistry([
      command('workspace.search', 'search.title', ['search', 'find']),
      command('app.settings', 'shell.nav.settings', ['settings', 'preferences']),
      command('workspace.backup', 'backup.title', ['backup']),
    ]);

    const byLabel = registry.search('设置', translate, available);
    expect(byLabel.map((entry) => entry.command.id)).toEqual(['app.settings']);

    const byAlias = registry.search('SEARCH', translate, available);
    expect(byAlias[0]?.command.id).toBe('workspace.search');

    const byId = registry.search('workspace.search', translate, available);
    expect(byId[0]?.command.id).toBe('workspace.search');

    const prefixFirst = registry.search('back', translate, available);
    expect(prefixFirst[0]?.command.id).toBe('workspace.backup');
  });

  it('rejects duplicate stable ids instead of silently merging handlers', () => {
    expect(() =>
      createCommandRegistry([
        command('workspace.search', 'search.title'),
        command('workspace.search', 'search.title'),
      ]),
    ).toThrow(/duplicate command id/);
  });

  it('reports a disabled reason and never invents one for available commands', () => {
    const registry = createCommandRegistry([
      command('workspace.delete', 'trash.delete', [], availability =>
        availability.canDelete ? null : 'workspace.select',
      ),
      command('editor.mode.read', 'editor.mode.read', ['read'], requireIdle),
    ]);
    const disabled = registry.search('', translate, {
      ...available,
      canDelete: false,
    });
    const deletion = disabled.find((entry) => entry.command.id === 'workspace.delete');
    expect(deletion?.reason).toBe('workspace.select');

    const enabled = registry.search('', translate, available);
    expect(enabled.every((entry) => entry.reason === null)).toBe(true);

    const busy = registry.search('', translate, { ...available, busy: true });
    const readMode = busy.find((entry) => entry.command.id === 'editor.mode.read');
    expect(readMode?.reason).toBe('editor.draft.blocked');
  });

  it('returns an empty list rather than a fabricated command for no matches', () => {
    const registry = createCommandRegistry([
      command('workspace.search', 'search.title', ['search']),
    ]);
    expect(registry.search('nonexistent', translate, available)).toEqual([]);
  });

  it('puts recent ids first and keeps the registration order for the rest', () => {
    const items = [
      { id: 'a' },
      { id: 'b' },
      { id: 'c' },
    ];
    const idOf = (item: { id: string }): string => item.id;
    expect(orderByRecent(items, ['c', 'missing', 'a'], idOf).map(idOf)).toEqual([
      'c',
      'a',
      'b',
    ]);
    expect(orderByRecent(items, [], idOf).map(idOf)).toEqual(['a', 'b', 'c']);
    expect(orderByRecent(items, ['c', 'c'], idOf).map(idOf)).toEqual(['c', 'a', 'b']);
  });
});

function requireIdle(availability: CommandAvailability): MessageKey | null {
  return availability.busy ? 'editor.draft.blocked' : null;
}
