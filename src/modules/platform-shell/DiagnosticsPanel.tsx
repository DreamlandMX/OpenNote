import { useCallback, useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import type {
  DiagnosticStatus,
  DiagnosticsHostFacts,
  DiagnosticsPreview,
  DiagnosticsRequest,
  NoteService,
} from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  capabilitiesFromProbe,
  correlationText,
  countsFromProbe,
  declaresRequiredOmissions,
  displayFields,
  includesLog,
  isHealthy,
  orderScopes,
  redactionSummary,
  scopeKey,
  statusesFromProbe,
  statusKey,
} from './diagnostics-view';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  onClose(): void;
}

/**
 * S54/S60 storage status and diagnostics.
 *
 * The report is assembled by M11 from an allowlist, so this screen cannot add a
 * path or a note body even by accident. Nothing is sent anywhere: the user
 * reviews the preview, then explicitly saves it to a path they pick (F44-R02).
 */
export function DiagnosticsPanel({ service, translator, locale, onClose }: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [facts, setFacts] = useState<DiagnosticsHostFacts | null>(null);
  const [preview, setPreview] = useState<DiagnosticsPreview | null>(null);
  const [overall, setOverall] = useState<DiagnosticStatus | null>(null);
  const [exportText, setExportText] = useState<string | null>(null);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const [request, setRequest] = useState<DiagnosticsRequest | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async (): Promise<void> => {
    setBusy(true);
    const hostFacts = await service.diagnosticsHostFacts();
    if (!hostFacts.ok) {
      setBusy(false);
      setError(hostFacts.error);
      return;
    }
    setFacts(hostFacts.value);
    // Real probe values: a null stays unknown inside countsFromProbe, so an
    // unreadable counter cannot become 0 here.
    const probeOutcome = await service.diagnosticsProbe();
    const probe = probeOutcome.ok
      ? probeOutcome.value
      : {
          notes: null,
          trashEntries: null,
          drafts: null,
          compileRunning: null,
          compileQueued: null,
          storageWritable: null,
          searchDocuments: null,
          storageStatus: { state: 'unknown' as const, reasonKey: 'diagnostics.noVault' },
          compileStatus: { state: 'unknown' as const, reasonKey: 'diagnostics.noToolchain' },
          searchStatus: { state: 'unknown' as const, reasonKey: 'diagnostics.noIndex' },
        };
    const request: DiagnosticsRequest = {
      facts: hostFacts.value,
      // The id identifies the operation only; M11 derives it from the seed.
      correlationId: `${hostFacts.value.platform}:${hostFacts.value.appVersion}:${Date.now()}`,
      counts: countsFromProbe(probe),
      capabilities: capabilitiesFromProbe(probe),
      statuses: statusesFromProbe(probe),
      lastErrorCode: null,
      timestampMillis: Date.now(),
      rawLog: null,
    };
    setRequest(request);
    setExportText(null);
    setSavedPath(null);
    const built = await service.diagnosticsPreview(request);
    setBusy(false);
    if (!built.ok) {
      setError(built.error);
      return;
    }
    setPreview(built.value);
    setError(null);
    const verdict = await service.diagnosticsOverall(built.value);
    setOverall(verdict.ok ? verdict.value : null);
  }, [service]);

  useEffect(() => {
    void load();
  }, [load]);

  /**
   * The request the export is rendered from.
   *
   * It reuses the request that produced the preview so the export cannot describe
   * a different probe set than the one the user just reviewed.
   */
  function requestFor(current: DiagnosticsPreview): DiagnosticsRequest {
    if (request !== null) {
      return { ...request, correlationId: current.correlationId };
    }
    return {
      facts: facts ?? {
        appVersion: '',
        buildChannel: '',
        platform: '',
        locale,
        roots: [],
      },
      correlationId: current.correlationId,
      counts: [],
      capabilities: [],
      statuses: current.statuses.map((entry) => ({
        scope: entry.scope,
        status: entry.status,
      })),
      lastErrorCode: null,
      timestampMillis: Date.now(),
      rawLog: null,
    };
  }

  const fields = preview === null ? [] : displayFields(preview.fields, translator('diagnostics.unknown'));
  const scopes = preview === null ? [] : orderScopes(preview.statuses);
  const redactions = preview === null ? null : redactionSummary(preview);

  return (
    <aside
      className="diagnostics-panel"
      aria-labelledby="diagnostics-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="diagnostics-head">
        <h2 id="diagnostics-heading">{translator('diagnostics.title')}</h2>
        <button
          type="button"
          className="diagnostics-close"
          aria-label={translator('diagnostics.close')}
          title={translator('diagnostics.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <p className="diagnostics-body">{translator('diagnostics.body')}</p>

      {busy ? <p role="status">{translator('diagnostics.loading')}</p> : null}
      {error !== null ? (
        <div className="diagnostics-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
        </div>
      ) : null}

      {preview !== null ? (
        <>
          <p
            className="diagnostics-verdict"
            data-healthy={overall !== null && isHealthy(overall) ? 'true' : undefined}
            role="status"
          >
            {overall === null
              ? translator('diagnostics.status.unknown')
              : translator(statusKey(overall))}
          </p>

          <section className="diagnostics-section" aria-labelledby="diagnostics-scopes">
            <h3 id="diagnostics-scopes">{translator('diagnostics.scopes')}</h3>
            <ul>
              {scopes.map((entry) => (
                <li key={entry.scope} className="diagnostics-row">
                  <span className="diagnostics-scope">{translator(scopeKey(entry.scope))}</span>
                  <span
                    className="diagnostics-status"
                    data-state={entry.status.state}
                  >
                    {translator(statusKey(entry.status))}
                  </span>
                </li>
              ))}
            </ul>
          </section>

          <section className="diagnostics-section" aria-labelledby="diagnostics-fields">
            <h3 id="diagnostics-fields">{translator('diagnostics.fields')}</h3>
            <ul>
              {fields.map((field) => (
                <li key={field.key} className="diagnostics-row">
                  <span className="diagnostics-key">{field.key}</span>
                  <span className="diagnostics-value" data-unknown={field.unknown ? 'true' : undefined}>
                    {field.text}
                  </span>
                </li>
              ))}
            </ul>
          </section>

          <p className="diagnostics-note">
            {translator('diagnostics.correlation', { id: correlationText(preview) })}
          </p>

          {preview.omitted.length > 0 ? (
            <section className="diagnostics-section" aria-labelledby="diagnostics-omitted">
              <h3 id="diagnostics-omitted">{translator('diagnostics.omittedTitle')}</h3>
              <ul>
                {preview.omitted.map((omission) => (
                  <li key={omission.key} className="diagnostics-row">
                    <span className="diagnostics-key">{omission.key}</span>
                    <span className="diagnostics-value">
                      {translator('diagnostics.omitted')}
                    </span>
                  </li>
                ))}
              </ul>
              {!declaresRequiredOmissions(preview) ? (
                <p className="diagnostics-warning" role="alert">
                  {translator('diagnostics.omissionsIncomplete')}
                </p>
              ) : null}
            </section>
          ) : null}

          {redactions !== null && redactions.total > 0 ? (
            <p className="diagnostics-warning" role="status">
              {translator('diagnostics.redacted', {
                count: redactions.total,
                kinds: redactions.kinds.join(', '),
              })}
            </p>
          ) : null}
          {includesLog(preview) ? (
            <p className="diagnostics-warning" role="status">
              {translator('diagnostics.includesLog')}
            </p>
          ) : null}

          <div className="diagnostics-controls">
            <button
              type="button"
              className="primary"
              onClick={() => {
                void service
                  .diagnosticsExportText(preview, requestFor(preview))
                  .then((outcome) => {
                    if (outcome.ok) {
                      setExportText(outcome.value);
                      setSavedPath(null);
                      setError(null);
                    } else {
                      setError(outcome.error);
                    }
                  });
              }}
            >
              {translator('diagnostics.previewExport')}
            </button>
            <button type="button" className="secondary" onClick={() => void load()}>
              {translator('diagnostics.refresh')}
            </button>
          </div>

          {exportText !== null ? (
            <>
              <p className="diagnostics-note">{translator('diagnostics.exportNote')}</p>
              <pre className="diagnostics-export" aria-label={translator('diagnostics.exportPreview')}>
                {exportText}
              </pre>
              <div className="diagnostics-controls">
                <button
                  type="button"
                  className="primary"
                  disabled={saving}
                  onClick={() => {
                    if (preview === null) {
                      return;
                    }
                    setSaving(true);
                    void service
                      .diagnosticsExport(
                        preview,
                        requestFor(preview),
                        translator('diagnostics.exportDialogTitle'),
                      )
                      .then((outcome) => {
                        setSaving(false);
                        if (!outcome.ok) {
                          // The preview stays on screen, so the report is not
                          // lost and another destination can be chosen.
                          setError(outcome.error);
                          return;
                        }
                        if (outcome.value !== null) {
                          setSavedPath(outcome.value);
                          setError(null);
                        }
                      });
                  }}
                >
                  {translator('diagnostics.saveReport')}
                </button>
              </div>
              {savedPath !== null ? (
                <p className="diagnostics-warning" role="status">
                  {translator('diagnostics.savedReport', { path: savedPath })}
                </p>
              ) : null}
            </>
          ) : null}
        </>
      ) : null}

      <p className="diagnostics-note">{translator('diagnostics.hint')}</p>
    </aside>
  );
}
