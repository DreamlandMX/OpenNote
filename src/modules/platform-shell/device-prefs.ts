import type { SupportedLocaleId } from '../presentation-i18n';
import { isSupportedLocaleId, matchLocaleTag } from '../presentation-i18n';

export type ThemePreference = 'system' | 'light' | 'dark';

/**
 * F17 daily-note settings for one vault.
 *
 * These are presentation choices, not note content: the host still owns the
 * path validation and the note itself. `null` means "use the built-in default",
 * so an absent value and an explicit reset behave the same.
 */
export interface DailySettings {
  folderPattern: string | null;
  templateId: string | null;
}

export interface DevicePreferences {
  locale: SupportedLocaleId | null;
  theme: ThemePreference | null;
  introSeen: boolean;
  /** Stable command ids of the most recently run commands, newest first. */
  recentCommands: readonly string[];
  /** Stable note ids of the most recently opened notes, newest first. */
  recentNotes: readonly string[];
  /** F12 query text executed on this device, newest first. */
  searchHistory: readonly string[];
  /**
   * F04 favorite note ids, newest first. Device state only: a favorite is a
   * pointer to a note, never a copy of its path or content.
   */
  favoriteNotes: readonly string[];
  /** F14 folded heading slugs; layout only, never a note change. */
  collapsedOutlineIds: readonly string[];
  /** F29 body text size in px; null means the default. */
  bodySize: number | null;
  /** F29 reading line measure in ch; null means the script default. */
  lineMeasure: number | null;
  /** F30 reduced motion: null follows the system, true/false override it. */
  reduceMotion: boolean | null;
  /** F30 stored chords per command id; empty text explicitly disables a binding. */
  shortcutBindings: Readonly<Record<string, string>>;
  /** F31 explicit region for dates/numbers; null follows the interface locale. */
  region: string | null;
  /** F31 explicit IANA zone; null means the device zone. */
  timeZone: string | null;
  /**
   * F04 split orientation. Device state only: F04-R04 says layout is not required
   * sync data, so it lives here rather than in a note or a synced record.
   */
  splitOrientation: 'horizontal' | 'vertical' | null;
  /**
   * F17 daily folder/template per vault. Keyed by vault id because two vaults
   * may use different daily layouts; a default is never written back.
   */
  dailySettings: Readonly<Record<string, DailySettings>>;
  /** Whether the workspace sidebar is folded away on this device. */
  sidebarCollapsed: boolean;
  /** Sidebar width in px chosen by dragging its edge; null means the default. */
  sidebarWidth: number | null;
  /** LaTeX projects rebuild on their own after typing pauses; on unless turned off. */
  latexAutoBuild: boolean;
}

const STORAGE_KEY = 'onote.devicePrefs.v1';

const THEME_VALUES: readonly ThemePreference[] = ['system', 'light', 'dark'];

const MAX_RECENT_COMMANDS = 8;
const MAX_RECENT_NOTES = 12;
const MAX_SEARCH_HISTORY = 12;
const MAX_SEARCH_QUERY_CODE_POINTS = 500;
const MAX_FAVORITE_NOTES = 100;
const MAX_COLLAPSED_OUTLINE_IDS = 200;
const MAX_DAILY_VAULTS = 32;
const MAX_DAILY_PATTERN_LENGTH = 240;
const MAX_DAILY_TEMPLATE_ID_LENGTH = 200;

function readStorage(): Record<string, unknown> | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === null) {
      return null;
    }
    const parsed: unknown = JSON.parse(raw);
    if (parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
    return null;
  } catch {
    return null;
  }
}

function parseTheme(value: unknown): ThemePreference | null {
  return typeof value === 'string' && (THEME_VALUES as readonly string[]).includes(value)
    ? (value as ThemePreference)
    : null;
}

/** Stored chords are text; anything that is not a string is dropped on read. */
function parseBindings(value: unknown): Readonly<Record<string, string>> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return {};
  }
  const bindings: Record<string, string> = {};
  for (const [key, chord] of Object.entries(value as Record<string, unknown>)) {
    if (typeof chord === 'string' && (chord === '' || chord.trim() !== '')) {
      bindings[key] = chord;
    }
  }
  return bindings;
}

function parseRecentIds(value: unknown, limit: number): readonly string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const ids: string[] = [];
  for (const entry of value) {
    if (typeof entry !== 'string' || entry === '' || ids.includes(entry)) {
      continue;
    }
    ids.push(entry);
    if (ids.length === limit) {
      break;
    }
  }
  return ids;
}

function normalizedSearchQuery(value: unknown): string | null {
  if (typeof value !== 'string') {
    return null;
  }
  const trimmed = value.trim();
  return trimmed !== '' && Array.from(trimmed).length <= MAX_SEARCH_QUERY_CODE_POINTS
    ? trimmed
    : null;
}

function parseSearchHistory(value: unknown): readonly string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const queries: string[] = [];
  for (const entry of value) {
    const query = normalizedSearchQuery(entry);
    if (query === null || queries.includes(query)) {
      continue;
    }
    queries.push(query);
    if (queries.length === MAX_SEARCH_HISTORY) {
      break;
    }
  }
  return queries;
}

function boundedText(value: unknown, limit: number): string | null {
  if (typeof value !== 'string') {
    return null;
  }
  const trimmed = value.trim();
  if (trimmed === '' || trimmed.length > limit) {
    return null;
  }
  return trimmed;
}

/**
 * F17 stored daily settings, one entry per vault.
 *
 * A malformed field is dropped rather than coerced: a bad pattern must fall
 * back to the default, not create a note in an unexpected folder. Entries with
 * no usable fields are dropped too, so `{}` stays the "nothing configured"
 * state and the map cannot grow with empty records.
 */
function parseDailySettings(value: unknown): Readonly<Record<string, DailySettings>> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return {};
  }
  const settings: Record<string, DailySettings> = {};
  let count = 0;
  for (const [vaultId, entry] of Object.entries(value as Record<string, unknown>)) {
    if (vaultId === '' || vaultId === '__proto__' || count >= MAX_DAILY_VAULTS) {
      continue;
    }
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) {
      continue;
    }
    const record = entry as Record<string, unknown>;
    const folderPattern = boundedText(record.folderPattern, MAX_DAILY_PATTERN_LENGTH);
    const templateId = boundedText(record.templateId, MAX_DAILY_TEMPLATE_ID_LENGTH);
    if (folderPattern === null && templateId === null) {
      continue;
    }
    settings[vaultId] = { folderPattern, templateId };
    count += 1;
  }
  return settings;
}

/** Newest first, de-duplicated, capped. Pure so the UI can reuse it in state. */
export function pushRecent(
  recent: readonly string[],
  id: string,
  limit: number,
): readonly string[] {
  if (id === '') {
    return recent;
  }
  return [id, ...recent.filter((entry) => entry !== id)].slice(0, limit);
}

export function recentCommandLimit(): number {
  return MAX_RECENT_COMMANDS;
}

export function recentNoteLimit(): number {
  return MAX_RECENT_NOTES;
}

export function searchHistoryLimit(): number {
  return MAX_SEARCH_HISTORY;
}

export function favoriteNoteLimit(): number {
  return MAX_FAVORITE_NOTES;
}

export const SIDEBAR_WIDTH_MIN = 200;
export const SIDEBAR_WIDTH_MAX = 480;

/** Keeps a dragged or stored sidebar width inside the usable range. */
export function clampSidebarWidth(width: number): number {
  if (!Number.isFinite(width)) return SIDEBAR_WIDTH_MIN;
  return Math.round(Math.min(SIDEBAR_WIDTH_MAX, Math.max(SIDEBAR_WIDTH_MIN, width)));
}

export function readDevicePreferences(): DevicePreferences {
  if (typeof window === 'undefined') {
    return {
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
  }
  const stored = readStorage();
  if (!stored) {
    return {
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
  }
  const locale = typeof stored.locale !== 'string' ? null
    : isSupportedLocaleId(stored.locale) ? stored.locale
    : matchLocaleTag(stored.locale) === 'zh-CN' ? 'zh-CN' : null;
  const theme = parseTheme(stored.theme);
  return {
    locale,
    theme,
    introSeen: stored.introSeen === true,
    recentCommands: parseRecentIds(stored.recentCommands, MAX_RECENT_COMMANDS),
    recentNotes: parseRecentIds(stored.recentNotes, MAX_RECENT_NOTES),
    searchHistory: parseSearchHistory(stored.searchHistory),
    favoriteNotes: parseRecentIds(stored.favoriteNotes, MAX_FAVORITE_NOTES),
    collapsedOutlineIds: parseRecentIds(stored.collapsedOutlineIds, MAX_COLLAPSED_OUTLINE_IDS),
    bodySize: typeof stored.bodySize === 'number' ? stored.bodySize : null,
    lineMeasure: typeof stored.lineMeasure === 'number' ? stored.lineMeasure : null,
    reduceMotion: typeof stored.reduceMotion === 'boolean' ? stored.reduceMotion : null,
    shortcutBindings: parseBindings(stored.shortcutBindings),
    region: typeof stored.region === 'string' && stored.region.trim() !== '' ? stored.region : null,
    timeZone:
      typeof stored.timeZone === 'string' && stored.timeZone.trim() !== ''
        ? stored.timeZone
        : null,
    // An unknown orientation is dropped rather than guessed.
    splitOrientation:
      stored.splitOrientation === 'horizontal' || stored.splitOrientation === 'vertical'
        ? stored.splitOrientation
        : null,
    dailySettings: parseDailySettings(stored.dailySettings),
    sidebarCollapsed: stored.sidebarCollapsed === true,
    sidebarWidth: typeof stored.sidebarWidth === 'number' ? clampSidebarWidth(stored.sidebarWidth) : null,
    latexAutoBuild: stored.latexAutoBuild !== false,
  };
}

/** F12 history is device-local; clearing it never touches notes or recents. */
export function clearSearchHistory(): void {
  writeDevicePreferences({ searchHistory: [] });
}

export function rememberSearchHistoryQuery(
  query: string,
): readonly string[] {
  const normalized = normalizedSearchQuery(query);
  if (normalized === null) {
    return readDevicePreferences().searchHistory;
  }
  const next = pushRecent(
    readDevicePreferences().searchHistory,
    normalized,
    MAX_SEARCH_HISTORY,
  );
  writeDevicePreferences({ searchHistory: next });
  return next;
}

export function writeDevicePreferences(patch: Partial<DevicePreferences>): void {
  if (typeof window === 'undefined') {
    return;
  }
  const stored = readStorage() ?? {};
  const next = { ...stored, ...patch };
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    return;
  }
}
