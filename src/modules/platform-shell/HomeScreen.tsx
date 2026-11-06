import { useEffect, useId, useRef, useState } from 'react';
import { ArrowLeft, FolderOpen } from 'lucide-react';
import type { Translator } from '../presentation-i18n';
import type { VaultAction } from './ShellApp';
import type { ChosenVaultFolder, VaultDialogOptions } from './port';

export interface HomeScreenProps {
  translator: Translator;
  connected: boolean;
  failureMessage: string | null;
  onBack: (() => void) | null;
  onVaultAction: (action: VaultAction, sampleOptIn: boolean, options?: VaultDialogOptions) => Promise<unknown>;
  onChooseFolder?: () => Promise<ChosenVaultFolder | null>;
}

export function HomeScreen({ translator, connected, failureMessage, onBack, onVaultAction, onChooseFolder }: HomeScreenProps) {
  const [name, setName] = useState('');
  const [folder, setFolder] = useState<ChosenVaultFolder | null>(null);
  const [sampleOptIn, setSampleOptIn] = useState(false);
  const [busy, setBusy] = useState(false);
  const [explaining, setExplaining] = useState(false);
  const [localError, setLocalError] = useState(false);
  const operation = useRef(false);
  const id = useId();
  const validName = name.trim().length > 0 && [...name.trim()].length <= 120 && !/[\u0000-\u001f\u007f-\u009f]/u.test(name);

  useEffect(() => {
    function handleKeydown(event: KeyboardEvent) {
      if (event.key === 'Escape' && !event.isComposing && !operation.current) onBack?.();
    }
    window.addEventListener('keydown', handleKeydown);
    return () => window.removeEventListener('keydown', handleKeydown);
  }, [onBack]);

  async function run(work: () => Promise<unknown>) {
    if (operation.current) return;
    operation.current = true;
    setBusy(true);
    setLocalError(false);
    try { await work(); } catch { setLocalError(true); }
    finally { operation.current = false; setBusy(false); }
  }

  async function chooseFolder() {
    if (!connected || !onChooseFolder) { setExplaining(true); return; }
    await run(async () => {
      const choice = await onChooseFolder();
      // Cancel and errors retain the previous choice and all form fields.
      if (choice !== null) setFolder(choice);
    });
  }

  return <div className="content-panel vault-setup">
    {onBack ? <button type="button" className="welcome-open" disabled={busy} onClick={onBack}><ArrowLeft size={16} aria-hidden="true" />{translator('home.back')}</button> : null}
    <h1>{translator('home.create.title')}</h1>
    <p className="notice">{translator('onboarding.welcome.summary')}</p>
    <form onSubmit={event => {
      event.preventDefault();
      if (!connected) { setExplaining(true); return; }
      if (!validName || !folder || busy) return;
      void run(() => onVaultAction('create', sampleOptIn, { selectionId: folder.selectionId, displayName: name.trim() }));
    }}>
      <label htmlFor={`${id}-name`}>{translator('home.create.name')}</label>
      <input id={`${id}-name`} value={name} disabled={busy} required aria-describedby={`${id}-name-help`} onChange={event => setName(event.target.value)} />
      <p id={`${id}-name-help`} className="notice">{translator('home.create.nameHint')}</p>
      <label htmlFor={`${id}-location`}>{translator('home.create.location')}</label>
      <div className="vault-location-row">
        <input id={`${id}-location`} value={folder?.displayPath ?? ''} readOnly placeholder={translator('home.create.locationHint')} />
        <button type="button" className="secondary" disabled={busy} onClick={() => void chooseFolder()}><FolderOpen size={16} aria-hidden="true" />{translator('home.create.choose')}</button>
      </div>
      <label className="sample-opt-in"><input type="checkbox" checked={sampleOptIn} disabled={busy} onChange={event => setSampleOptIn(event.target.checked)} />{translator('onboarding.sample.optIn')}</label>
      <button type="submit" className="primary" disabled={busy || !connected || !validName || !folder}>{translator('home.create.title')}</button>
    </form>
    <button type="button" className="welcome-open" disabled={busy} onClick={() => {
      if (!connected) { setExplaining(true); return; }
      void run(() => onVaultAction('open', false));
    }}>{translator('home.open.title')}</button>
    {busy ? <p role="status">{translator('home.create.working')}</p> : null}
    {explaining ? <div className="explanation"><h2>{translator('home.explain.title')}</h2><p>{translator('home.explain.body')}</p><button type="button" className="secondary" onClick={() => setExplaining(false)}>{translator('home.explain.close')}</button></div> : null}
    {failureMessage || localError ? <div className="error-panel" role="alert"><p>{failureMessage ?? translator('dialog.error.fallback')}</p></div> : null}
  </div>;
}