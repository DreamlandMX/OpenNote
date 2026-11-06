import { afterEach, describe, expect, it, vi } from 'vitest';
import { applyBinding, chordText, effectiveBinding, parseBindings, parseChord } from './shortcuts';
import { resolveLocaleFromTags } from '../presentation-i18n';
import {
  clearSearchHistory,
  pushRecent,
  readDevicePreferences,
  recentCommandLimit,
  recentNoteLimit,
  searchHistoryLimit,
  writeDevicePreferences,
} from './device-prefs';

function fakeStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('device preferences', () => {
  it.each(['zh-TW', 'zh-HK', 'zh-Hant'])('maps old Chinese preference %s without rewriting other settings', (locale) => {
    const raw = JSON.stringify({ locale, theme: 'dark', recentNotes: ['kept'], searchHistory: ['原文'] });
    const storage = fakeStorage({ 'onote.devicePrefs.v1': raw });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences()).toMatchObject({ locale: 'zh-CN', theme: 'dark', recentNotes: ['kept'], searchHistory: ['原文'] });
    expect(storage.getItem('onote.devicePrefs.v1')).toBe(raw);
  });

  it.each(['ja', 'ko', 'de', 'fr', 'pt-BR', 'it', 'ru', 'ar', 'hi', 'id', 'tr', 'vi'])('falls back safely from removed preference %s', (locale) => {
    const storage = fakeStorage({ 'onote.devicePrefs.v1': JSON.stringify({ locale, theme: 'dark' }) });
    vi.stubGlobal('window', { localStorage: storage });
    const preferences = readDevicePreferences();
    expect(preferences.locale).toBeNull();
    expect(preferences.theme).toBe('dark');
    expect(preferences.locale ?? resolveLocaleFromTags(['ja', 'es-MX'])).toBe('es');
    expect(preferences.locale ?? resolveLocaleFromTags(['ja'])).toBe('en');
  });

  it('preserves replaced shortcut unbindings through actual preference storage', () => {
    vi.stubGlobal('window', { localStorage: fakeStorage() });
    const replaced = applyBinding({}, 'workspace.commandPalette', parseChord('Ctrl+P')!, true);
    const stored = Object.fromEntries(Object.entries(replaced.bindings).map(([id, chord]) => [id, chordText(chord!)]));
    writeDevicePreferences({ shortcutBindings: stored });
    const reopened = parseBindings(readDevicePreferences().shortcutBindings);
    expect(effectiveBinding(reopened, 'workspace.quickOpen')).toBeNull();
    expect(effectiveBinding(reopened, 'workspace.commandPalette')).toEqual(parseChord('Ctrl+P'));
  });
  it('defaults a missing or malformed onboarding flag to incomplete', () => {
    const storage = fakeStorage();
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().introSeen).toBe(false);

    storage.setItem('onote.devicePrefs.v1', JSON.stringify({ introSeen: 'true' }));
    expect(readDevicePreferences().introSeen).toBe(false);
  });

  it('persists onboarding completion without replacing other device preferences', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ locale: 'zh-CN', theme: 'dark' }),
    });
    vi.stubGlobal('window', { localStorage: storage });

    writeDevicePreferences({ introSeen: true });

    expect(readDevicePreferences()).toMatchObject({
      locale: 'zh-CN',
      theme: 'dark',
      introSeen: true,
    });
  });

  it('reads an invalid stored locale as absent so startup can fall back', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ locale: 'xx-XY' }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().locale).toBeNull();
  });

  it('reads recent ids defensively and ignores junk entries', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({
        recentCommands: ['workspace.search', 7, '', 'workspace.search', 'app.settings'],
        recentNotes: 'not-an-array',
      }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().recentCommands).toEqual([
      'workspace.search',
      'app.settings',
    ]);
    expect(readDevicePreferences().recentNotes).toEqual([]);
  });

  it('keeps newest first, de-duplicates and caps the list', () => {
    expect(pushRecent(['a', 'b'], 'c', 3)).toEqual(['c', 'a', 'b']);
    expect(pushRecent(['a', 'b', 'c'], 'b', 3)).toEqual(['b', 'a', 'c']);
    expect(pushRecent(['a', 'b', 'c'], 'd', 3)).toEqual(['d', 'a', 'b']);
    expect(pushRecent([], '', 3)).toEqual([]);
    expect(pushRecent([], 'a', recentCommandLimit())).toHaveLength(1);
    expect(pushRecent([], 'a', recentNoteLimit())).toHaveLength(1);
  });

  it('persists recent lists without dropping the other preferences', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ locale: 'es', theme: 'light' }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    writeDevicePreferences({ recentCommands: ['workspace.search'], recentNotes: ['n1'] });
    expect(readDevicePreferences()).toMatchObject({
      locale: 'es',
      theme: 'light',
      recentCommands: ['workspace.search'],
      recentNotes: ['n1'],
      favoriteNotes: [],
      collapsedOutlineIds: [],
      bodySize: null,
      lineMeasure: null,
      reduceMotion: null,
      shortcutBindings: {},
      region: null,
      timeZone: null,
    });
  });

  it('keeps search history bounded and clears it without touching recent notes', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({
        searchHistory: ['  alpha  ', 7, '', 'alpha', 'beta', '😀'.repeat(501)],
        recentNotes: ['n1'],
        locale: 'zh-CN',
      }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().searchHistory).toEqual(['alpha', 'beta']);

    writeDevicePreferences({
      searchHistory: pushRecent(readDevicePreferences().searchHistory, 'gamma', searchHistoryLimit()),
    });
    expect(readDevicePreferences().searchHistory).toEqual(['gamma', 'alpha', 'beta']);

    clearSearchHistory();
    expect(readDevicePreferences()).toMatchObject({
      searchHistory: [],
      recentNotes: ['n1'],
      locale: 'zh-CN',
    });
  });

  it('reads favorite ids defensively and keeps them across a write', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({
        favoriteNotes: ['n2', 7, '', 'n2', 'n1'],
        locale: 'zh-CN',
      }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().favoriteNotes).toEqual(['n2', 'n1']);

    writeDevicePreferences({ favoriteNotes: ['n3'] });
    expect(readDevicePreferences()).toMatchObject({
      locale: 'zh-CN',
      favoriteNotes: ['n3'],
    });

    const malformed = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ favoriteNotes: 'n1' }),
    });
    vi.stubGlobal('window', { localStorage: malformed });
    expect(readDevicePreferences().favoriteNotes).toEqual([]);
  });

  it('reads reduced motion and shortcut bindings defensively', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({
        reduceMotion: true,
        shortcutBindings: { 'workspace.search': 'Ctrl+Alt+F', bogus: 42, empty: '  ' },
      }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    const prefs = readDevicePreferences();
    expect(prefs.reduceMotion).toBe(true);
    expect(prefs.shortcutBindings).toEqual({ 'workspace.search': 'Ctrl+Alt+F' });

    const wrong = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ reduceMotion: 'yes', shortcutBindings: [] }),
    });
    vi.stubGlobal('window', { localStorage: wrong });
    expect(readDevicePreferences().reduceMotion).toBeNull();
    expect(readDevicePreferences().shortcutBindings).toEqual({});
  });

  it('folds outline ids from device storage and ignores malformed entries', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ collapsedOutlineIds: ['本周行动', 4, '细节'] }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().collapsedOutlineIds).toEqual(['本周行动', '细节']);

    const malformed = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ collapsedOutlineIds: '本周行动' }),
    });
    vi.stubGlobal('window', { localStorage: malformed });
    expect(readDevicePreferences().collapsedOutlineIds).toEqual([]);
  });
});

describe('split orientation', () => {
  it('restores a stored direction', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ splitOrientation: 'vertical' }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().splitOrientation).toBe('vertical');
  });

  it('drops an unknown direction rather than guessing one', () => {
    // Coercing an unrecognised value would invent a layout choice the user never made.
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ splitOrientation: 'diagonal' }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().splitOrientation).toBeNull();
  });

  it('writes the direction without disturbing other preferences', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ locale: 'zh-CN' }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    writeDevicePreferences({ splitOrientation: 'horizontal' });
    const stored = JSON.parse(storage.getItem('onote.devicePrefs.v1') ?? '{}');
    expect(stored.splitOrientation).toBe('horizontal');
    expect(stored.locale).toBe('zh-CN');
  });
});

describe('daily note settings', () => {
  it('reads a per-vault pattern and template, trimming stored text', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({
        dailySettings: {
          vaultA: { folderPattern: '  Journal/YYYY/MM  ', templateId: ' tpl-1 ' },
        },
      }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().dailySettings).toEqual({
      vaultA: { folderPattern: 'Journal/YYYY/MM', templateId: 'tpl-1' },
    });
  });

  it('keeps one vault independent from another', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({
        dailySettings: {
          a: { folderPattern: 'A/YYYY', templateId: null },
          b: { folderPattern: null, templateId: 'tpl-b' },
        },
      }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().dailySettings).toEqual({
      a: { folderPattern: 'A/YYYY', templateId: null },
      b: { folderPattern: null, templateId: 'tpl-b' },
    });
  });

  it('reads each field independently and drops unusable values', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({
        dailySettings: {
          array: [],
          scalar: 'Daily/YYYY',
          empty: { folderPattern: '   ', templateId: '' },
          badPatternType: { folderPattern: 42, templateId: 'tpl' },
          good: { folderPattern: 'Daily/YYYY', templateId: null },
        },
      }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    expect(readDevicePreferences().dailySettings).toEqual({
      badPatternType: { folderPattern: null, templateId: 'tpl' },
      good: { folderPattern: 'Daily/YYYY', templateId: null },
    });

    const notAnObject = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({ dailySettings: ['Daily/YYYY'] }),
    });
    vi.stubGlobal('window', { localStorage: notAnObject });
    expect(readDevicePreferences().dailySettings).toEqual({});
  });

  it('ignores a prototype key instead of changing the map prototype', () => {
    // Built from JSON text: an object literal would treat __proto__ as the
    // prototype, so it would never reach the parser as a key.
    const storage = fakeStorage({
      'onote.devicePrefs.v1':
        '{"dailySettings":{"__proto__":{"folderPattern":"Daily/YYYY","templateId":null},"good":{"folderPattern":"Journal/YYYY","templateId":null}}}',
    });
    vi.stubGlobal('window', { localStorage: storage });
    const settings = readDevicePreferences().dailySettings;
    expect(Object.keys(settings)).toEqual(['good']);
    expect(Object.getPrototypeOf(settings)).toBe(Object.prototype);
  });

  it('persists one vault without replacing the others', () => {
    const storage = fakeStorage({
      'onote.devicePrefs.v1': JSON.stringify({
        dailySettings: { a: { folderPattern: 'A/YYYY', templateId: null } },
      }),
    });
    vi.stubGlobal('window', { localStorage: storage });
    writeDevicePreferences({
      dailySettings: {
        a: { folderPattern: 'A/YYYY', templateId: null },
        b: { folderPattern: 'B/YYYY', templateId: 'tpl-b' },
      },
    });
    expect(readDevicePreferences().dailySettings).toEqual({
      a: { folderPattern: 'A/YYYY', templateId: null },
      b: { folderPattern: 'B/YYYY', templateId: 'tpl-b' },
    });
  });
});
