import { useEffect, useState } from 'react';
import { FilePlus2, FolderOpen, ShieldCheck, Sigma } from 'lucide-react';
import { PanelHeader } from './PanelHeader';
import { useOverlayFocus } from './use-overlay-focus';
import type { ProjectInspection } from '../editor';
import type { ServiceError } from '../domain';
import type {
  ChosenLatexFolder,
  LatexEnvironmentService,
  LatexEnvironmentStatus,
  LatexProjectService,
  LatexProjectSummary,
} from '../latex-workspace/ports';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  fileCount,
  findingText,
  partitionFindings,
  rankedCandidates,
  unknownDependencies,
  verdictKey,
} from './project-inspect-view';
import { environmentStepKey, projectErrorKey, registrableEntry } from './latex-project-view';
import { newOperationId } from './tauri-note-service';

interface Props {
  latexService: LatexProjectService;
  environment: LatexEnvironmentService;
  vaultId: string;
  writable: boolean;
  translator: Translator;
  locale: SupportedLocaleId;
  /** Opens a registered project's workspace (sources beside the PDF). */
  onOpenProject(projectId: string): void;
  onOpenEnvironment(): void;
  onCreateProject(): void;
  onClose(): void;
}

/**
 * The LaTeX entry point, in the order the work happens: the local environment,
 * then a new or registered project, then opening one to build.
 *
 * S76 project inspection and registration.
 *
 * The host lists and reads the picked folder itself; the panel never submits a
 * path. Nothing is executed: a shell escape blocks the project instead of being
 * quietly allowed (F45-R02). An ambiguous entry is chosen by the user, never
 * guessed. Removing a registration keeps every project file.
 */
export function ProjectInspectPanel({
  latexService, environment, vaultId, writable, translator, locale, onOpenProject, onOpenEnvironment, onCreateProject, onClose,
}: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [selection, setSelection] = useState<ChosenLatexFolder | null>(null);
  const [inspection, setInspection] = useState<ProjectInspection | null>(null);
  const [chosenEntry, setChosenEntry] = useState<string | null>(null);
  // One id per registration attempt, so a retry after a lost reply is idempotent.
  const [registerOperation, setRegisterOperation] = useState(newOperationId);
  const [projects, setProjects] = useState<readonly LatexProjectSummary[] | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [busy, setBusy] = useState(false);
  // `undefined` while checking, `null` when the status could not be read.
  const [envStatus, setEnvStatus] = useState<LatexEnvironmentStatus | null | undefined>(undefined);

  async function refreshProjects(): Promise<void> {
    const outcome = await latexService.listProjects(vaultId);
    if (outcome.ok) setProjects(outcome.value);
    else setError(outcome.error);
  }

  useEffect(() => {
    // Read-only: checking never starts a download.
    void environment.status().then((outcome) => setEnvStatus(outcome.ok ? outcome.value : null));
  }, [environment]);

  useEffect(() => {
    void refreshProjects();
    // The list belongs to this vault session only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [vaultId]);

  async function choose(): Promise<void> {
    setBusy(true);
    setStatus(null);
    const picked = await latexService.chooseProjectFolder(vaultId, translator('project.chooseTitle'));
    if (!picked.ok) {
      setBusy(false);
      setError(picked.error);
      return;
    }
    if (picked.value === null) {
      setBusy(false);
      return;
    }
    const inspected = await latexService.inspectProject(vaultId, picked.value.selectionId);
    setBusy(false);
    setSelection(picked.value);
    setRegisterOperation(newOperationId());
    if (inspected.ok) {
      setInspection(inspected.value);
      setChosenEntry(inspected.value.entry);
      setError(null);
    } else {
      setInspection(null);
      setError(inspected.error);
    }
  }

  async function register(entry: string): Promise<void> {
    if (selection === null) return;
    setBusy(true);
    const outcome = await latexService.registerProject(vaultId, selection.selectionId, entry, registerOperation);
    setBusy(false);
    if (outcome.ok) {
      setError(null);
      setStatus(translator('project.registered', { folder: outcome.value.folder || '/' }));
      setRegisterOperation(newOperationId());
      await refreshProjects();
    } else {
      setError(outcome.error);
    }
  }

  async function remove(projectId: string): Promise<void> {
    setBusy(true);
    const outcome = await latexService.unregisterProject(vaultId, projectId, newOperationId());
    setBusy(false);
    setConfirmRemove(null);
    if (outcome.ok) {
      setError(null);
      setStatus(translator('project.removed'));
      await refreshProjects();
    } else {
      setError(outcome.error);
    }
  }

  const split = inspection === null ? null : partitionFindings(inspection.findings);
  const unknown = inspection === null ? [] : unknownDependencies(inspection);
  const ranked = inspection === null ? [] : rankedCandidates(inspection);
  const entry = registrableEntry(inspection, chosenEntry);
  const errorKey = error === null ? null : projectErrorKey(error);
  const ready = envStatus?.environment.kind === 'ready' && envStatus.preparing === null;

  return (
    <aside
      className="inspect-panel"
      aria-labelledby="inspect-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <PanelHeader
        id="inspect-heading"
        icon={Sigma}
        title={translator('project.title')}
        purpose={translator('project.purpose')}
        closeLabel={translator('project.close')}
        onClose={onClose}
        status={
          <button type="button" className="panel-badge" data-tone={ready ? 'ok' : envStatus === undefined ? undefined : 'warn'}
            title={translator('latexEnv.title')} onClick={onOpenEnvironment}>
            {translator(ready ? 'project.badge.ready' : envStatus === undefined ? 'project.badge.checking' : 'project.badge.setup')}
          </button>
        }
      />
      {/* Only a problem is spelled out; a ready environment is the badge alone. */}
      {!ready && envStatus !== undefined ? <p className="inspect-note">{translator(environmentStepKey(envStatus))}</p> : null}

      <div className="inspect-cards">
        <button type="button" className="inspect-card" disabled={!writable} onClick={onCreateProject}>
          <FilePlus2 size={20} aria-hidden="true" />
          <strong>{translator('project.card.new')}</strong>
          <span>{translator('project.card.newBody')}</span>
        </button>
        <button type="button" className="inspect-card" disabled={busy} onClick={() => void choose()}>
          <FolderOpen size={20} aria-hidden="true" />
          <strong>{translator('project.card.open')}</strong>
          <span>{translator('project.card.openBody')}</span>
        </button>
      </div>
      {selection !== null ? (
        <p className="inspect-path">{translator('project.chosen', { folder: selection.folder || '/' })}</p>
      ) : null}

      {error !== null ? (
        <div className="inspect-error" role="alert">
          <p>{errorKey !== null ? translator(errorKey) : serviceErrorMessage(locale, error.code)}</p>
        </div>
      ) : null}
      {status !== null ? <p className="inspect-note" role="status">{status}</p> : null}

      {inspection !== null && split !== null ? (
        <>
          <p
            className="inspect-verdict"
            data-ready={inspection.inspectable ? 'true' : undefined}
            role="status"
          >
            {translator(verdictKey(inspection))}
            {' · '}
            {translator('project.rootSummary', {
              root: inspection.root || '/',
              files: fileCount(inspection),
            })}
          </p>

          {ranked.length > 0 ? (
            <fieldset className="inspect-section">
              <legend>{translator(inspection.entry !== null ? 'project.entryLegend' : 'project.candidates')}</legend>
              {ranked.map((candidate) => (
                <label key={candidate.path} className="inspect-row theme-option">
                  <input
                    type="radio"
                    name="inspect-entry"
                    checked={chosenEntry === candidate.path}
                    onChange={() => setChosenEntry(candidate.path)}
                  />
                  <span className="inspect-path">{candidate.path}</span>
                  <span className="inspect-score">
                    {translator(candidate.reasonKey as 'project.entryDeclaresClass')}
                  </span>
                </label>
              ))}
            </fieldset>
          ) : null}

          {split.blocking.length > 0 ? (
            <section className="inspect-section" aria-labelledby="inspect-blocking">
              <h3 id="inspect-blocking">{translator('project.blocking')}</h3>
              <ul>
                {split.blocking.map((finding, index) => (
                  <li key={`${finding.kind}:${index}`} className="inspect-row inspect-danger">
                    {findingText(finding, translator)}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          {split.warnings.length > 0 ? (
            <section className="inspect-section" aria-labelledby="inspect-warnings">
              <h3 id="inspect-warnings">{translator('project.warnings')}</h3>
              <ul>
                {split.warnings.map((finding, index) => (
                  <li key={`${finding.kind}:${index}`} className="inspect-row inspect-warn">
                    {findingText(finding, translator)}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          {unknown.length > 0 ? (
            <p className="inspect-warn" role="status">
              {translator('project.unknownDependencies', {
                count: unknown.length,
                names: unknown.join(', '),
              })}
            </p>
          ) : null}

          <div className="inspect-controls">
            <button
              type="button"
              className="primary"
              disabled={busy || !writable || entry === null}
              onClick={() => { if (entry !== null) void register(entry); }}
            >
              {translator('project.register')}
            </button>
          </div>
          {!writable ? <p className="inspect-note">{translator('workspace.vault.readOnly')}</p> : null}
          <p className="inspect-note">{translator('project.noExec')}</p>
        </>
      ) : null}

      <section className="inspect-section" aria-labelledby="inspect-projects">
        <h3 id="inspect-projects">{translator('project.mine')}</h3>
        {projects !== null && projects.length === 0 ? (
          <p className="inspect-note">{translator('project.none')}</p>
        ) : null}
        <ul>
          {(projects ?? []).map((project) => (
            <li key={project.projectId} className="inspect-row inspect-project">
              <Sigma size={16} aria-hidden="true" />
              <span className="inspect-path">
                {project.readable
                  ? <><strong>{projectName(project)}</strong> <small>{translator('project.listed', { folder: project.folder || '/', entry: project.entryPath ?? '—' })}</small></>
                  : translator('project.unreadable')}
              </span>
              {confirmRemove === project.projectId ? (
                <>
                  <span className="inspect-warn">{translator('project.removeConfirm')}</span>
                  <button type="button" className="secondary" disabled={busy} onClick={() => void remove(project.projectId)}>
                    {translator('project.removeYes')}
                  </button>
                  <button type="button" className="secondary" onClick={() => setConfirmRemove(null)}>
                    {translator('project.removeNo')}
                  </button>
                </>
              ) : (
                <>
                  <button
                    type="button"
                    className="primary"
                    disabled={busy || !project.readable}
                    onClick={() => onOpenProject(project.projectId)}
                  >
                    {translator('project.open')}
                  </button>
                  <button
                    type="button"
                    className="secondary"
                    disabled={busy || !writable}
                    onClick={() => setConfirmRemove(project.projectId)}
                  >
                    {translator('project.remove')}
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      </section>

      <p className="panel-note inspect-safety"><ShieldCheck size={14} aria-hidden="true" /> {translator('project.safety')}</p>
    </aside>
  );
}

/** The folder's own name, or the main file's for a project at the vault root. */
function projectName(project: LatexProjectSummary): string {
  const folder = (project.folder ?? '').replace(/\/+$/, '');
  if (folder !== '') return folder.split('/').pop() ?? folder;
  return (project.entryPath ?? 'main.tex').split('/').pop()?.replace(/\.tex$/i, '') ?? 'main';
}
