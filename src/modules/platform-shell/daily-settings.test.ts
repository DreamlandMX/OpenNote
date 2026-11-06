import { describe, expect, it } from 'vitest';
import type { DevicePreferences } from './device-prefs';
import {
  DEFAULT_DAILY_FOLDER_PATTERN,
  dailyPatternProblem,
  dailySettingsDraft,
  dailySettingsFor,
  dailyTemplateMissing,
  draftToDailySettings,
  effectiveDailyPattern,
  withDailySettings,
} from './daily-settings';

const BASE: DevicePreferences = {
  locale: null,
  theme: null,
  introSeen: false,
  recentCommands: [],
  recentNotes: [],
  searchHistory: [],
  favoriteNotes: [],
  collapsedOutlineIds: [],
  bodySize: null,
  lineMeasure: null,
  reduceMotion: null,
  shortcutBindings: {},
  region: null,
  timeZone: null,
  splitOrientation: null,
  dailySettings: {},
  sidebarCollapsed: false,
  sidebarWidth: null,
  latexAutoBuild: true,
};

describe('daily settings lookup', () => {
  it('uses the built-in pattern when a vault has no setting', () => {
    expect(dailySettingsFor(BASE, 'vault-a')).toEqual({
      folderPattern: null,
      templateId: null,
    });
    expect(effectiveDailyPattern(dailySettingsFor(BASE, 'vault-a'))).toBe(
      DEFAULT_DAILY_FOLDER_PATTERN,
    );
  });

  it('reads only the requested vault', () => {
    const preferences: DevicePreferences = {
      ...BASE,
      dailySettings: {
        a: { folderPattern: 'A/YYYY', templateId: null },
        b: { folderPattern: 'B/YYYY', templateId: 'tpl-b' },
      },
    };
    expect(dailySettingsFor(preferences, 'b')).toEqual({
      folderPattern: 'B/YYYY',
      templateId: 'tpl-b',
    });
  });

  it('removes a vault entry when both fields are cleared', () => {
    const preferences: DevicePreferences = {
      ...BASE,
      dailySettings: { a: { folderPattern: 'A/YYYY', templateId: 'tpl' } },
    };
    expect(withDailySettings(preferences, 'a', { folderPattern: null, templateId: null })).toEqual(
      {},
    );
    expect(withDailySettings(preferences, '', { folderPattern: 'X', templateId: null })).toEqual({
      a: { folderPattern: 'A/YYYY', templateId: 'tpl' },
    });
    expect(
      withDailySettings(preferences, '__proto__', { folderPattern: 'X', templateId: null }),
    ).toEqual({ a: { folderPattern: 'A/YYYY', templateId: 'tpl' } });
  });

  it('does not mutate the map it was given', () => {
    const preferences: DevicePreferences = {
      ...BASE,
      dailySettings: { a: { folderPattern: 'A/YYYY', templateId: null } },
    };
    withDailySettings(preferences, 'b', { folderPattern: 'B/YYYY', templateId: null });
    expect(preferences.dailySettings).toEqual({
      a: { folderPattern: 'A/YYYY', templateId: null },
    });
  });
});

describe('daily pattern validation', () => {
  it('accepts the default, a nested pattern, and a blank reset', () => {
    expect(dailyPatternProblem(DEFAULT_DAILY_FOLDER_PATTERN)).toBeNull();
    expect(dailyPatternProblem('Journal/YYYY/MM')).toBeNull();
    expect(dailyPatternProblem('   ')).toBeNull();
  });

  it('refuses absolute paths and parent traversal like the host does', () => {
    expect(dailyPatternProblem('/Daily/YYYY')).toBe('unsafe');
    expect(dailyPatternProblem('Daily/../YYYY')).toBe('unsafe');
    expect(dailyPatternProblem('Daily\\..\\YYYY')).toBe('unsafe');
  });

  it('refuses a path shape the host cannot create', () => {
    expect(dailyPatternProblem('Daily/YYYY/')).toBe('invalid');
    expect(dailyPatternProblem('Daily//YYYY')).toBe('invalid');
    expect(dailyPatternProblem('Daily/./YYYY')).toBe('invalid');
    expect(dailyPatternProblem('C:/Daily/YYYY')).toBe('invalid');
    expect(dailyPatternProblem('Daily:YYYY')).toBe('invalid');
    expect(dailyPatternProblem('Daily/*/YYYY')).toBe('invalid');
  });

  it('refuses brace placeholders this build cannot expand', () => {
    expect(dailyPatternProblem('Daily/{year}')).toBe('invalid');
    expect(dailyPatternProblem('Daily/YYYY}')).toBe('invalid');
  });

  it('refuses control characters and overlong patterns', () => {
    expect(dailyPatternProblem('Daily/\u0000YYYY')).toBe('invalid');
    expect(dailyPatternProblem(`Daily/${'Y'.repeat(300)}`)).toBe('invalid');
  });
});

describe('daily settings draft', () => {
  it('round-trips a configured vault', () => {
    const settings = { folderPattern: 'Journal/YYYY', templateId: 'tpl-1' };
    const draft = dailySettingsDraft(settings);
    expect(draft).toEqual({ folderPattern: 'Journal/YYYY', templateId: 'tpl-1' });
    expect(draftToDailySettings(draft)).toEqual({ settings, problem: null });
  });

  it('turns blank fields into the defaults rather than empty strings', () => {
    expect(
      draftToDailySettings({ folderPattern: '  ', templateId: ' ' }),
    ).toEqual({
      settings: { folderPattern: null, templateId: null },
      problem: null,
    });
  });

  it('returns no settings when the pattern is unsafe', () => {
    expect(draftToDailySettings({ folderPattern: '../Daily', templateId: 'tpl' })).toEqual({
      settings: null,
      problem: 'unsafe',
    });
  });

  it('returns no settings when a field is overlong', () => {
    expect(
      draftToDailySettings({ folderPattern: 'Daily/YYYY', templateId: 't'.repeat(201) }),
    ).toEqual({ settings: null, problem: 'invalid' });
  });
});

describe('missing daily template', () => {
  const templates = [{ noteId: 'tpl-1' }, { noteId: 'tpl-2' }];

  it('is true only when a stored id is absent from the list', () => {
    expect(dailyTemplateMissing('tpl-1', templates)).toBe(false);
    expect(dailyTemplateMissing('tpl-gone', templates)).toBe(true);
  });

  it('does not report a missing template when none is configured', () => {
    expect(dailyTemplateMissing(null, templates)).toBe(false);
    expect(dailyTemplateMissing('', templates)).toBe(false);
  });
});
