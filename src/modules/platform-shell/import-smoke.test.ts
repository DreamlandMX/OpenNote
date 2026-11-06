import { describe, expect, it } from 'vitest';
import {
  createTauriShellPort,
  disconnectedShellPort,
  readDevicePreferences,
  resolveShellPort,
} from './index';

describe('platform shell', () => {
  it('resolves the reserved shell port without touching a real backend', async () => {
    expect(resolveShellPort(undefined)).toBe(disconnectedShellPort);
    expect(createTauriShellPort()).toBeNull();
    const result = createTauriShellPort() ?? disconnectedShellPort;
    const outcome = await result.createVault();
    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.error.code).toBe('NOT_IMPLEMENTED');
    }
  });

  it('survives an environment without device storage and never invents preferences', () => {
    if (typeof window !== 'undefined') {
      return;
    }
    expect(readDevicePreferences()).toEqual({
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
      // F04 layout is device state; with no storage it must stay unset rather
      // than default to a direction the user never chose.
      splitOrientation: null,
      // F17 settings are per-vault too; an empty map is the only honest value
      // before the user configures a vault.
      dailySettings: {},
      sidebarCollapsed: false,
      sidebarWidth: null,
      latexAutoBuild: true,
    });
    expect(() => resolveShellPort(null)).not.toThrow();
  });
});
