import { useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import type { ServiceError } from '../domain';
import type { LatexEnvironmentPlan, LatexEnvironmentService, LatexEnvironmentStatus } from '../latex-workspace/ports';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import { environmentView, formatBytes, reasonKey, STAGE_KEYS } from './latex-environment-view';
import { newOperationId } from './tauri-note-service';

interface Props {
  service: LatexEnvironmentService;
  translator: Translator;
  locale: SupportedLocaleId;
  onClose(): void;
}

const POLL_MS = 1000;

/**
 * The local LaTeX environment.
 *
 * Nothing is downloaded until the user has seen what would be fetched, from
 * where and how much, and pressed the agree button; only then is `prepare`
 * called with consent. Compiles never use the network. Removing the
 * environment also removes its sandbox profiles and their folder permissions.
 */
export function LatexEnvironmentPanel({ service, translator, locale, onClose }: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [plan, setPlan] = useState<LatexEnvironmentPlan | null>(null);
  const [status, setStatus] = useState<LatexEnvironmentStatus | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function refresh(): Promise<void> {
    const outcome = await service.status();
    if (outcome.ok) setStatus(outcome.value);
    else setError(outcome.error);
  }

  useEffect(() => {
    void service.plan().then((outcome) => {
      if (outcome.ok) setPlan(outcome.value);
      else setError(outcome.error);
    });
    void refresh();
    // The service is stable for the panel's lifetime.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const view = environmentView(status);
  useEffect(() => {
    if (view !== 'preparing') return undefined;
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view]);

  async function agreeAndPrepare(): Promise<void> {
    setBusy(true);
    setNotice(null);
    const outcome = await service.prepare(true, newOperationId());
    setBusy(false);
    if (outcome.ok) {
      setError(null);
      await refresh();
    } else {
      setError(outcome.error);
    }
  }

  async function remove(): Promise<void> {
    setBusy(true);
    const outcome = await service.remove();
    setBusy(false);
    setConfirmRemove(false);
    if (!outcome.ok) {
      setError(outcome.error);
      return;
    }
    setError(null);
    setNotice(
      outcome.value.errors.length === 0
        ? translator('latexEnv.removed')
        : translator('latexEnv.removedPartly', { count: outcome.value.errors.length }),
    );
    await refresh();
  }

  const bytes = (value: number) => formatBytes(value, locale);
  const environment = status?.environment ?? null;
  const preparing = status?.preparing ?? null;
  const last = status?.last ?? null;

  return (
    <aside
      className="inspect-panel"
      aria-labelledby="latex-env-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="inspect-head">
        <h2 id="latex-env-heading">{translator('latexEnv.title')}</h2>
        <button
          type="button"
          className="inspect-close"
          aria-label={translator('latexEnv.close')}
          title={translator('latexEnv.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      {error !== null ? (
        <div className="inspect-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
        </div>
      ) : null}
      {notice !== null ? <p className="inspect-note" role="status">{notice}</p> : null}

      {view === 'loading' ? <p className="inspect-note">{translator('latexEnv.loading')}</p> : null}

      {view === 'ready' && environment?.kind === 'ready' ? (
        <>
          <p className="inspect-verdict" data-ready="true" role="status">
            {translator('latexEnv.ready', { files: environment.fileCount, size: bytes(environment.totalBytes) })}
          </p>
          <p className="inspect-note">{translator('latexEnv.offline')}</p>
        </>
      ) : null}

      {view === 'preparing' && preparing !== null ? (
        <section className="inspect-section" aria-labelledby="latex-env-progress">
          <h3 id="latex-env-progress">{translator(STAGE_KEYS[preparing.progress.stage])}</h3>
          <p className="inspect-note" role="status">
            {preparing.progress.steps > 0
              ? translator('latexEnv.step', { step: preparing.progress.step, steps: preparing.progress.steps })
              : null}
            {' '}
            {preparing.progress.totalBytes !== null
              ? translator('latexEnv.bytesOf', { done: bytes(preparing.progress.bytes), total: bytes(preparing.progress.totalBytes) })
              : bytes(preparing.progress.bytes)}
          </p>
          <div className="inspect-controls">
            <button type="button" className="secondary" onClick={() => void service.cancel().then(() => refresh())}>
              {translator('latexEnv.cancel')}
            </button>
          </div>
        </section>
      ) : null}

      {view === 'notReady' ? (
        <>
          {environment?.kind === 'invalid' ? (
            <p className="inspect-warn" role="status">
              {translator('latexEnv.invalid')} {translator(reasonKey(environment.reasonKey))}
            </p>
          ) : (
            <p className="inspect-body">{translator('latexEnv.notPrepared')}</p>
          )}
          {last !== null && !last.succeeded ? (
            <p className="inspect-warn" role="status">
              {translator('latexEnv.lastFailed')} {translator(reasonKey(last.reasonKey))}
            </p>
          ) : null}
          {plan !== null ? (
            <section className="inspect-section" aria-labelledby="latex-env-plan">
              <h3 id="latex-env-plan">{translator('latexEnv.planTitle')}</h3>
              <ul>
                <li className="inspect-row">
                  {translator('latexEnv.planEngine', {
                    name: plan.engineName,
                    version: plan.engineVersion,
                    license: plan.engineLicense,
                    host: plan.engineHost,
                    size: bytes(plan.engineDownloadBytes),
                  })}
                </li>
                <li className="inspect-row">
                  {translator('latexEnv.planResources', {
                    host: plan.resourceHost,
                    size: bytes(plan.resourceEstimatedBytes),
                    max: bytes(plan.resourceMaxBytes),
                  })}
                </li>
                <li className="inspect-row">
                  {translator('latexEnv.planLimits', { minutes: Math.ceil(plan.prepareSeconds / 60) })}
                </li>
              </ul>
              <p className="inspect-note">{translator('latexEnv.planPrivacy')}</p>
              <div className="inspect-controls">
                <button type="button" className="primary" disabled={busy} onClick={() => void agreeAndPrepare()}>
                  {translator('latexEnv.agree')}
                </button>
              </div>
            </section>
          ) : null}
        </>
      ) : null}

      {view !== 'preparing' && view !== 'loading' ? (
        <section className="inspect-section" aria-labelledby="latex-env-remove">
          <h3 id="latex-env-remove">{translator('latexEnv.removeTitle')}</h3>
          <p className="inspect-note">{translator('latexEnv.removeBody')}</p>
          {confirmRemove ? (
            <div className="inspect-controls">
              <span className="inspect-warn">{translator('latexEnv.removeConfirm')}</span>
              <button type="button" className="secondary" disabled={busy} onClick={() => void remove()}>
                {translator('latexEnv.removeYes')}
              </button>
              <button type="button" className="secondary" onClick={() => setConfirmRemove(false)}>
                {translator('latexEnv.removeNo')}
              </button>
            </div>
          ) : (
            <div className="inspect-controls">
              <button type="button" className="secondary" disabled={busy} onClick={() => setConfirmRemove(true)}>
                {translator('latexEnv.remove')}
              </button>
            </div>
          )}
        </section>
      ) : null}
    </aside>
  );
}
