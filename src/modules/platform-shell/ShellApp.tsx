import { useEffect, useMemo, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import { Leaf, Menu } from 'lucide-react';
import {
  dirForLocale,
  resolveLocaleFromTags,
  serviceErrorMessage,
  translate,
  type MessageKey,
  type SupportedLocaleId,
} from '../presentation-i18n';
import type { ServiceError, Vault } from '../domain';
import { SaveController } from '../editor';
import { acceptOverwrite } from '../history-backup/overwrite-result';
import { acceptUpgrade } from '../history-backup/upgrade-result';
import type { Overwritten, Upgraded } from '../history-backup/ports';
import { readDevicePreferences, writeDevicePreferences, type ThemePreference } from './device-prefs';
import { appearanceStyle, resolveAppearance } from './appearance';
import { chordText, parseBindings, type BindableCommandId, type Chord } from './shortcuts';
import { validateTimeZone, type RegionalPreferences } from './regional';
import { changeLocale as commitLocaleChange } from './locale-change';
import { resolveShellPort, serviceErrorFromThrown, type ShellPort, type VaultDialogOptions, type ChosenVaultFolder } from './port';
import { newOperationId, resolveNoteService, resolveBackupService } from './tauri-note-service';
import { reopenLastVault, shouldReopenLastVault } from './last-vault';
import { WelcomeScreen } from './WelcomeScreen';
import { HomeScreen } from './HomeScreen';
import { SettingsScreen } from './SettingsScreen';
import { Sidebar, type Screen } from './Sidebar';
import {
  WorkspaceScreen,
  type GuardBlockedEntry,
  type WorkspaceGate,
  type VaultAction,
} from './WorkspaceScreen';

export interface ShellAppProps {
  shellPort?: ShellPort;
}

export type { VaultAction } from './WorkspaceScreen';

const THEME_KEYS: Record<ThemePreference, MessageKey> = {
  system: 'settings.theme.system',
  light: 'settings.theme.light',
  dark: 'settings.theme.dark',
};

const GUARD_STATUS_KEYS: Record<GuardBlockedEntry['status'], MessageKey> = {
  working: 'trash.working',
  clean: 'editor.save.saved',
  dirty: 'editor.save.dirty',
  saving: 'editor.save.saving',
  failed: 'editor.save.failed.title',
  conflict: 'editor.conflict.title',
};

const THEME_VALUES: readonly ThemePreference[] = ['system', 'light', 'dark'];

interface GuardState {
  options?: VaultDialogOptions;
  purpose: 'close' | 'switch';
  action?: VaultAction;
  sampleOptIn?: boolean;
}

function navigatorLocaleTags(): readonly string[] {
  if (typeof navigator === 'undefined') {
    return [];
  }
  return navigator.languages ?? [];
}

function pathLabel(value: string): string {
  const prefix = '\\\\?\\';
  return value.startsWith(prefix + 'UNC\\') ? '\\\\' + value.slice(prefix.length + 4) : value.startsWith(prefix) ? value.slice(prefix.length) : value;
}

export function ShellApp({ shellPort }: ShellAppProps) {
  const port = useMemo(() => resolveShellPort(shellPort), [shellPort]);
  const [localeId, setLocaleId] = useState<SupportedLocaleId>(() => {
    const prefs = readDevicePreferences();
    if (prefs.locale) {
      return prefs.locale;
    }
    return resolveLocaleFromTags([...navigatorLocaleTags(), 'en']);
  });
  const [theme, setTheme] = useState<ThemePreference>(() => {
    const stored = readDevicePreferences().theme;
    return stored && THEME_VALUES.includes(stored) ? stored : 'system';
  });
  const [bodySize, setBodySize] = useState<number | null>(
    () => readDevicePreferences().bodySize,
  );
  const [lineMeasure, setLineMeasure] = useState<number | null>(
    () => readDevicePreferences().lineMeasure,
  );
  const [reduceMotion, setReduceMotion] = useState<boolean | null>(
    () => readDevicePreferences().reduceMotion,
  );
  const [region, setRegion] = useState<string | null>(() => readDevicePreferences().region);
  const [timeZone, setTimeZone] = useState<string | null>(
    () => readDevicePreferences().timeZone,
  );
  const [shortcutBindings, setShortcutBindings] = useState<
    Partial<Record<BindableCommandId, Chord>>
  >(() => {
    return parseBindings(readDevicePreferences().shortcutBindings);
  });
  useEffect(() => {
    const refresh = (): void => setShortcutBindings(parseBindings(readDevicePreferences().shortcutBindings));
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, []);
  const [introComplete, setIntroComplete] = useState(
    () => readDevicePreferences().introSeen,
  );
  const [screen, setScreen] = useState<Exclude<Screen, 'welcome'>>('home');
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [announcement, setAnnouncement] = useState('');
  const [localeLoadFailed, setLocaleLoadFailed] = useState(false);
  const [vault, setVault] = useState<Vault | null>(null);
  const [vaultSession, setVaultSession] = useState(0);
  const handledUpgrades = useRef(new Set<string>());
  const handledOverwrites = useRef(new Set<string>());
  const localeChangeSequence = useRef(0);
  const [vaultActionError, setVaultActionError] = useState<ServiceError | null>(null);
  const [guard, setGuard] = useState<GuardState | null>(null);
  const [guardBusy, setGuardBusy] = useState(false);
  const [guardRows, setGuardRows] = useState<readonly GuardBlockedEntry[]>([]);
  const [lifecycleFreeze, setLifecycleFreeze] = useState(false);
  const [vaultPickerPending, setVaultPickerPending] = useState(false);
  const [vaultCreating, setVaultCreating] = useState(false);
  // While the previous vault is being reopened nothing is drawn, so the start
  // screen does not flash before the workspace appears.
  const [reopening, setReopening] = useState(
    () => shouldReopenLastVault(port, readDevicePreferences().introSeen),
  );
  const reopenStarted = useRef(false);
  const pendingWork = guardRows.some(row => row.status === 'working');
  const onlyPendingWork = pendingWork && guardRows.every(row => row.status === 'working');

  const noteService = useMemo(
    () => resolveNoteService(vault?.vaultId),
    [vault?.vaultId],
  );

  const translator = useMemo(() => translate.bind(null, localeId), [localeId]);
  const backupService = useMemo(() => resolveBackupService(vault?.vaultId), [vault?.vaultId, vaultSession]);

  const saveController = useMemo(
    () =>
      new SaveController(
        { saveNote: (request) => noteService.saveNote(request), restoreHistory: request => noteService.restoreHistory(request) },
        { uuid: () => newOperationId() },
      ),
    [noteService, vaultSession],
  );

  const guardRef = useRef<GuardState | null>(null);
  guardRef.current = guard;
  const vaultRef = useRef<Vault | null>(null);
  vaultRef.current = vault;
  const workspaceGateRef = useRef<WorkspaceGate | null>(null);
  const lifecycleBusyRef = useRef(false);
  const handleHostCloseRequestRef = useRef<() => void>(() => undefined);

  useEffect(() => {
    document.documentElement.lang = localeId;
    document.documentElement.dir = dirForLocale(localeId);
  }, [localeId]);

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
  }, [theme]);

  // F31: an explicit region wins; otherwise formatting follows the interface
  // locale, and an unset zone means "use the device zone".
  const regional: RegionalPreferences = {
    region: region ?? localeId,
    timeZone: timeZone ?? Intl.DateTimeFormat().resolvedOptions().timeZone ?? 'UTC',
  };

  // F30: `system` keeps the media query in charge; an explicit choice wins.
  useEffect(() => {
    document.documentElement.setAttribute(
      'data-reduce-motion',
      reduceMotion === null ? 'system' : reduceMotion ? 'reduce' : 'full',
    );
  }, [reduceMotion]);

  // F29: body size and measure are CSS variables on the root, so they apply to
  // the reading pane and the editor without touching a note (F29-R04).
  const appearance = resolveAppearance(localeId, {
    bodySize: bodySize ?? undefined,
    lineMeasure: lineMeasure ?? undefined,
  });
  useEffect(() => {
    const style = appearanceStyle(appearance, localeId);
    for (const [property, value] of Object.entries(style)) {
      document.documentElement.style.setProperty(property, value);
    }
  }, [appearance.bodySize, appearance.lineMeasure, localeId]);

  useEffect(() => {
    const unsubscribe = saveController.subscribe(() => {
      if (guardRef.current !== null) {
        setGuardRows(workspaceGateRef.current?.blockedEntries() ?? []);
      }
    });
    return unsubscribe;
  }, [saveController]);

  useEffect(() => {
    if (!drawerOpen) {
      return;
    }
    function handleKeydown(event: globalThis.KeyboardEvent) {
      if (event.key === 'Escape') {
        setDrawerOpen(false);
      }
    }
    window.addEventListener('keydown', handleKeydown);
    return () => {
      window.removeEventListener('keydown', handleKeydown);
    };
  }, [drawerOpen]);

  useEffect(() => {
    // A ref rather than a cleanup flag: StrictMode re-runs this effect, and the
    // first call must still be the one that lands.
    if (!reopening || reopenStarted.current) {
      return;
    }
    reopenStarted.current = true;
    void reopenLastVault(port).then((reopened) => {
      if (reopened !== null) {
        setVault(reopened);
        setVaultSession((session) => session + 1);
        setScreen('workspace');
      }
      setReopening(false);
    });
  }, [port, reopening]);

  useEffect(() => {
    const unlisten = port.onCloseRequested(() => {
      handleHostCloseRequestRef.current();
    });
    return () => {
      void unlisten();
    };
  }, [port]);

  async function changeLocale(next: SupportedLocaleId): Promise<void> {
    const sequence = ++localeChangeSequence.current;
    try {
      const result = await commitLocaleChange(
        next,
        sequence,
        () => localeChangeSequence.current,
        {
          currentLocale: () => localeId,
          activate: (locale) => {
            setLocaleId(locale);
            setLocaleLoadFailed(false);
          },
          persist: (locale) => writeDevicePreferences({ locale }),
          announce: setAnnouncement,
        },
      );
      if (result !== 'superseded') {
        setLocaleLoadFailed(result === 'kept');
      }
    } catch {
      if (sequence === localeChangeSequence.current) {
        setLocaleLoadFailed(true);
        setAnnouncement(translator('onboarding.locale.failed'));
      }
    }
  }
  function completeUpgrade(value: Upgraded): 'accepted' | 'wait' | 'rejected' {
    return acceptUpgrade(value, vault?.vaultId ?? null, handledUpgrades.current, saveController.notesNeedingAttentionOnExit().length > 0, next => {
      setVault(next); setVaultSession(session => session + 1); setVaultActionError(null);
      setAnnouncement(translator('backup.upgradeDone')); navigate('workspace');
    });
  }
  function completeOverwrite(value: Overwritten): 'accepted' | 'wait' | 'rejected' {
    return acceptOverwrite(value, vault?.vaultId ?? null, handledOverwrites.current, saveController.notesNeedingAttentionOnExit().length > 0, (next, previousRoot) => {
      setVault(next); setVaultSession(session => session + 1); setVaultActionError(null);
      setAnnouncement(translator('backup.overwriteDone', { path: pathLabel(previousRoot) })); navigate('workspace');
    });
  }

  function changeTheme(next: ThemePreference) {
    setTheme(next);
    writeDevicePreferences({ theme: next });
    setAnnouncement(translate(localeId, 'a11y.settings.themeSet', { name: translate(localeId, THEME_KEYS[next]) }));
  }

  function navigate(screenTarget: Exclude<Screen, 'welcome'>) {
    setScreen(screenTarget);
    if (screenTarget !== 'workspace') {
      setIntroComplete(true);
    }
    setDrawerOpen(false);
  }

  function blockedCount(): number {
    return workspaceGateRef.current?.blockedEntries().length ?? saveController.notesNeedingAttentionOnExit().length;
  }

  function refreshGuardRows(): void {
    setGuardRows(workspaceGateRef.current?.blockedEntries() ?? []);
  }

  function freezeEditingSurface(): void {
    if (lifecycleBusyRef.current) {
      return;
    }
    lifecycleBusyRef.current = true;
    flushSync(() => {
      setLifecycleFreeze(true);
    });
  }

  function unfreezeEditingSurface(): void {
    lifecycleBusyRef.current = false;
    setLifecycleFreeze(false);
  }

  function confirmHostClose(): void {
    freezeEditingSurface();
    void port.confirmClose().then((result) => {
      if (!result.ok) {
        unfreezeEditingSurface();
        setAnnouncement(translator('dialog.error.fallback'));
      }
    });
  }

  async function runVaultAction(action: VaultAction, sampleOptIn = false, options?: VaultDialogOptions): Promise<void> {
    if (lifecycleBusyRef.current) {
      return;
    }
    freezeEditingSurface();
    try {
      if (vaultRef.current !== null && blockedCount() > 0) {
        setGuard({ purpose: 'switch', action, sampleOptIn, options });
        refreshGuardRows();
        return;
      }
      const dialogTitle =
        action === 'create' ? translator('home.create.title') : translator('home.open.title');
      setVaultPickerPending(true);
      setVaultCreating(action === 'create' && options?.selectionId !== undefined);
      const outcome =
        action === 'create'
          ? await port.createVault({ ...options, dialogTitle, sampleOptIn })
          : await port.openVault({ dialogTitle });
      if (!outcome.ok) {
        setVaultActionError(outcome.error);
        return;
      }
      if (outcome.value === null) {
        if (vaultRef.current !== null) {
          navigate('workspace');
        }
        return;
      }
      if (vaultRef.current !== null && blockedCount() > 0) {
        setGuard({ purpose: 'switch', action, sampleOptIn, options });
        refreshGuardRows();
        return;
      }
      setVault(outcome.value);
      setVaultSession((session) => session + 1);
      setVaultActionError(null);
      writeDevicePreferences({ introSeen: true });
      navigate('workspace');
    } catch (error) {
      setVaultActionError(serviceErrorFromThrown(error));
    } finally {
      setVaultPickerPending(false);
      unfreezeEditingSurface();
    }
  }

  async function performVaultAction(action: VaultAction, sampleOptIn = false, options?: VaultDialogOptions): Promise<void> {
    if (lifecycleBusyRef.current) {
      return;
    }
    setVaultActionError(null);
    if (vaultRef.current !== null) {
      if (guardRef.current !== null) {
        return;
      }
      if (blockedCount() > 0) {
        setGuard({ purpose: 'switch', action, sampleOptIn, options });
        refreshGuardRows();
        return;
      }
    }
    await runVaultAction(action, sampleOptIn, options);
  }

  async function chooseVaultFolder(): Promise<ChosenVaultFolder | null> {
    if (lifecycleBusyRef.current || guardRef.current !== null || !port.chooseVaultFolder) return null;
    freezeEditingSurface();
    setVaultCreating(false);
    setVaultPickerPending(true);
    setVaultActionError(null);
    try {
      const outcome = await port.chooseVaultFolder(translator('home.create.choose'));
      if (!outcome.ok) { setVaultActionError(outcome.error); return null; }
      return outcome.value;
    } catch (error) {
      setVaultActionError(serviceErrorFromThrown(error));
      return null;
    } finally {
      setVaultPickerPending(false);
      unfreezeEditingSurface();
    }
  }

  handleHostCloseRequestRef.current = () => {
    if (lifecycleBusyRef.current) {
      return;
    }
    if (guardRef.current !== null) {
      return;
    }
    if (vaultRef.current === null || blockedCount() === 0) {
      confirmHostClose();
      return;
    }
    setGuard({ purpose: 'close' });
    refreshGuardRows();
  };

  async function continueGuard(): Promise<void> {
    const state = guard;
    if (state === null || guardBusy) {
      return;
    }
    flushSync(() => {
      setGuardBusy(true);
    });
    const gate = workspaceGateRef.current;
    const clean = gate
      ? await gate.saveBlockedAndWait()
      : blockedCount() === 0;
    setGuardBusy(false);
    if (!clean) {
      refreshGuardRows();
      return;
    }
    if (state.purpose === 'close') {
      setGuard(null);
      confirmHostClose();
      return;
    }
    const action = state.action;
    setGuard(null);
    if (action !== undefined) {
      await runVaultAction(action, state.sampleOptIn, state.options);
    }
  }

  async function exportGuardBlocked(): Promise<void> {
    if (guardBusy) {
      return;
    }
    setGuardBusy(true);
    const gate = workspaceGateRef.current;
    if (gate) {
      await gate.exportBlocked();
    }
    setGuardBusy(false);
    refreshGuardRows();
  }

  function cancelGuard(): void {
    if (guardBusy) {
      return;
    }
    setGuard(null);
  }

  function backToWelcome() {
    setIntroComplete(false);
  }

  const active: Screen = introComplete ? screen : 'welcome';
  const connected = port.kind === 'tauri';
  const failureMessage =
    vaultActionError === null
      ? null
      : serviceErrorMessage(localeId, vaultActionError.code, vaultActionError.messageKey);
  const neutralLocalSummary = translator('onboarding.welcome.summary');
  const statusTop =
    !connected
      ? translator('shell.status.disconnected')
      : vault === null
        ? translator('shell.brand.name')
        : translator('workspace.status.vault', { name: vault.displayName });
  const statusBottom =
    !connected
      ? translator('shell.status.mode')
      : vault === null
        ? neutralLocalSummary
        : vault.writable
          ? neutralLocalSummary
          : translator('workspace.vault.readOnly');

  const surfaceInert = lifecycleFreeze || (guard !== null && guardBusy);

  if (reopening) {
    return <div className="app-shell" data-screen="welcome" aria-busy="true" />;
  }

  return (
    <div className="app-shell" data-workspace={active === 'workspace'} data-screen={active}>
      {vaultPickerPending ? <div className="vault-picker-progress" role="status" aria-live="polite"><p>{translator(vaultCreating ? 'home.create.working' : 'home.picker.waiting')}</p></div> : null}
      <header className="topbar">
        <button
          type="button"
          className="drawer-toggle"
          aria-expanded={drawerOpen ? 'true' : 'false'}
          aria-label={drawerOpen ? translator('a11y.nav.closeMenu') : translator('a11y.nav.openMenu')}
          inert={surfaceInert}
          onClick={() => setDrawerOpen((open) => !open)}
        >
          <Menu size={20} aria-hidden="true" />
        </button>
        <p className="app-title">
          <Leaf size={18} aria-hidden="true" />
          {translator('shell.brand.name')}
        </p>
        {active === 'home' ? <button type="button" className="welcome-open" inert={surfaceInert} onClick={() => navigate('settings')}>{translator('shell.nav.settings')}</button> : null}
      </header>
      <div inert={surfaceInert} style={{ display: 'contents' }}>
        <Sidebar
          translator={translator}
          screen={screen}
          drawerOpen={drawerOpen}
          showWorkspace={vault !== null}
          onNavigate={navigate}
          statusTop={statusTop}
          statusBottom={statusBottom}
        />
      </div>
      <main className="content" id="main-content" inert={surfaceInert}>
        {localeLoadFailed ? (
          <p className="notice" role="alert">
            {translator('onboarding.locale.failed')}
          </p>
        ) : null}
        {active === 'welcome' ? (
          <WelcomeScreen
            translator={translator}
            locale={localeId}
            onLocaleChange={changeLocale}
            onContinue={() => navigate('home')}
            onOpenExisting={() => { navigate('home'); if (connected) void performVaultAction('open'); }}
          />
        ) : null}
        {active === 'home' ? (
          <HomeScreen
            translator={translator}
            connected={connected}
            failureMessage={failureMessage}
            onBack={vault === null ? backToWelcome : () => navigate('workspace')}
            onVaultAction={performVaultAction}
            onChooseFolder={connected && port.chooseVaultFolder ? chooseVaultFolder : undefined}
          />
        ) : null}
        {active === 'settings' ? (
          <SettingsScreen
            onBack={() => navigate(vault === null ? 'home' : 'workspace')}
            translator={translator}
            locale={localeId}
            onLocaleChange={changeLocale}
            currentTheme={theme}
            onThemeChange={changeTheme}
            bodySize={bodySize}
            lineMeasure={lineMeasure}
            onBodySizeChange={(size) => {
              setBodySize(size);
              writeDevicePreferences({ bodySize: size });
            }}
            onLineMeasureChange={(measure) => {
              setLineMeasure(measure);
              writeDevicePreferences({ lineMeasure: measure });
            }}
            reduceMotion={reduceMotion}
            onReduceMotionChange={(value) => {
              setReduceMotion(value);
              writeDevicePreferences({ reduceMotion: value });
            }}
            regional={regional}
            onRegionChange={(value) => {
              setRegion(value);
              writeDevicePreferences({ region: value });
            }}
            onTimeZoneChange={(value) => {
              // An unusable zone would make every formatted instant throw, so
              // invalid input is ignored rather than stored.
              if (value === null || validateTimeZone(value)) {
                setTimeZone(value);
                writeDevicePreferences({ timeZone: value });
              }
            }}
            shortcutBindings={shortcutBindings}
            onShortcutBindingsChange={(next) => {
              setShortcutBindings(next);
              const stored: Record<string, string> = {};
              for (const [key, chord] of Object.entries(next)) {
                if (chord !== undefined) {
                  stored[key] = chordText(chord);
                }
              }
              writeDevicePreferences({ shortcutBindings: stored });
            }}
            onOpenShortcuts={() => undefined}
          />
        ) : null}
        {vault !== null ? (
          <div className="workspace-holder" hidden={active !== 'workspace'}>
            <WorkspaceScreen
              key={vaultSession}
              translator={translator}
              locale={localeId}
              regional={regional}
              vault={vault}
              noteService={noteService}
              backupService={backupService}
              saveController={saveController}
              gateRef={workspaceGateRef}
              onUpgraded={completeUpgrade}
              onNavigate={navigate}
              onOverwritten={completeOverwrite}
              active={active === 'workspace'}
              shortcutBindings={shortcutBindings}
            />
          </div>
        ) : null}
      </main>
      <footer className="statusbar">
        <span>
          <strong>{statusTop}</strong> {statusBottom}
        </span>
      </footer>
      <p className="visually-hidden" role="status" aria-live="polite">
        {announcement}
      </p>
      {guard !== null ? (
        <div className="guard-overlay" role="dialog" aria-modal="true" aria-label={translator(onlyPendingWork ? 'trash.working' : 'editor.save.dirty')}>
          <div className="guard-dialog">
            <h2>{translator(onlyPendingWork ? 'trash.working' : 'editor.save.dirty')}</h2>
            <p className="small-print">{translator(pendingWork ? 'lifecycle.wait.body' : 'editor.save.failed.body')}</p>
            <ul className="guard-rows">
              {guardRows.map((row) => (
                <li key={row.noteId} className="guard-row">
                  <span>{row.path ?? row.noteId}</span>
                  <strong>{translator(GUARD_STATUS_KEYS[row.status])}</strong>
                </li>
              ))}
            </ul>
            <div className="guard-actions">
              <button type="button" className="primary" disabled={guardBusy} onClick={() => void continueGuard()}>
                {translator(pendingWork ? 'lifecycle.wait.action' : 'editor.save.failed.retry')}
              </button>
              <button type="button" className="secondary" disabled={guardBusy || pendingWork} onClick={() => void exportGuardBlocked()}>
                {translator('editor.save.failed.export')}
              </button>
              <button type="button" className="secondary" disabled={guardBusy} onClick={cancelGuard}>
                {translator('workspace.create.cancel')}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
