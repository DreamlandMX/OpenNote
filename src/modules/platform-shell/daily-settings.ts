import type { DailySettings, DevicePreferences } from './device-prefs';

/**
 * F17 daily-note settings.
 *
 * The host remains the authority for a safe folder pattern and for whether a
 * template exists. These helpers only keep the device preference, present the
 * default, and reject the two mistakes the user can see before an open fails:
 * an absolute/`..` path and a brace placeholder this build does not expand.
 */

/** Host default, repeated here so the form can show it without a round trip. */
export const DEFAULT_DAILY_FOLDER_PATTERN = 'Daily/YYYY';

const MAX_PATTERN_LENGTH = 240;
const MAX_TEMPLATE_ID_LENGTH = 200;

const EMPTY_SETTINGS: DailySettings = { folderPattern: null, templateId: null };

/** Stored settings for a vault; an unknown vault gets the defaults. */
export function dailySettingsFor(
  preferences: DevicePreferences,
  vaultId: string,
): DailySettings {
  return preferences.dailySettings[vaultId] ?? EMPTY_SETTINGS;
}

/**
 * Returns the next map with one vault's settings replaced.
 *
 * Clearing both fields removes the entry, so the map stays an accurate record
 * of configured vaults and a later default change is not masked by `{}`.
 */
export function withDailySettings(
  preferences: DevicePreferences,
  vaultId: string,
  settings: DailySettings,
): Readonly<Record<string, DailySettings>> {
  const next: Record<string, DailySettings> = { ...preferences.dailySettings };
  // A vault id comes from the host, but keep prototype keys out of the map in
  // case a caller ever passes through untrusted state.
  if (vaultId === '' || vaultId === '__proto__') {
    return next;
  }
  if (settings.folderPattern === null && settings.templateId === null) {
    delete next[vaultId];
  } else {
    next[vaultId] = settings;
  }
  return next;
}

/** The pattern actually passed to the host, including the built-in default. */
export function effectiveDailyPattern(settings: DailySettings): string {
  return settings.folderPattern ?? DEFAULT_DAILY_FOLDER_PATTERN;
}

export type DailyPatternProblem = 'invalid' | 'unsafe';

/**
 * Mirrors the host checks the form can explain before it stores a pattern.
 *
 * `onote_tasks::expand_pattern` rejects absolute paths, `..`, and braces;
 * `pathcheck::validate_new_note_path` additionally rejects empty/`.` segments
 * and characters Windows cannot put in a file name. A pattern accepted here can
 * still be refused by the host (it stays authoritative), but the common
 * mistakes are caught while the user is looking at the field.
 */
export function dailyPatternProblem(pattern: string): DailyPatternProblem | null {
  const trimmed = pattern.trim();
  if (trimmed === '') {
    return null;
  }
  if (trimmed.length > MAX_PATTERN_LENGTH) {
    return 'invalid';
  }
  const normalised = trimmed.replace(/\\/g, '/');
  const segments = normalised.split('/');
  if (normalised.startsWith('/') || segments.includes('..')) {
    return 'unsafe';
  }
  if (/[{}]/.test(normalised)) {
    return 'invalid';
  }
  if (segments.some((segment) => segment === '' || segment === '.')) {
    return 'invalid';
  }
  if (
    segments.some(
      (segment) => segment.includes(':') || /["<>|*?\u0000-\u001f\u007f]/.test(segment),
    )
  ) {
    return 'invalid';
  }
  return null;
}

export interface DailySettingsDraft {
  folderPattern: string;
  templateId: string;
}

/** Form state from stored settings; `null` becomes an empty field. */
export function dailySettingsDraft(settings: DailySettings): DailySettingsDraft {
  return {
    folderPattern: settings.folderPattern ?? '',
    templateId: settings.templateId ?? '',
  };
}

/**
 * Form state back to stored settings.
 *
 * `null` means the draft cannot be saved yet; the caller shows the problem
 * instead of writing a partial setting. A blank pattern is a deliberate reset
 * to the host default, not an empty folder.
 */
export function draftToDailySettings(
  draft: DailySettingsDraft,
): { settings: DailySettings | null; problem: DailyPatternProblem | null } {
  const problem = dailyPatternProblem(draft.folderPattern);
  if (problem !== null) {
    return { settings: null, problem };
  }
  const pattern = draft.folderPattern.trim();
  const templateId = draft.templateId.trim();
  if (pattern.length > MAX_PATTERN_LENGTH || templateId.length > MAX_TEMPLATE_ID_LENGTH) {
    return { settings: null, problem: 'invalid' };
  }
  return {
    settings: {
      folderPattern: pattern === '' ? null : pattern,
      templateId: templateId === '' ? null : templateId,
    },
    problem: null,
  };
}

/** Whether a stored template id is absent from the current list. */
export function dailyTemplateMissing(
  templateId: string | null,
  templates: readonly { noteId: string }[],
): boolean {
  return (
    templateId !== null &&
    templateId !== '' &&
    !templates.some((template) => template.noteId === templateId)
  );
}
