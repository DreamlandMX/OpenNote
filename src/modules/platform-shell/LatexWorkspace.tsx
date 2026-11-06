import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Ban, ChevronRight, FileArchive, FileDown, FileText, Hammer, PanelLeftClose, PanelLeftOpen, X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import { newOperationId } from './tauri-note-service';
import type { ServiceError } from '../domain';
import type { TexDiagnostic } from '../editor';
import type { MessageKey, Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import { PdfPane, type PdfMark, type PdfSource } from '../latex-workspace/PdfPane';
import { LatexSourceEditor } from '../latex-workspace/LatexSourceEditor';
import { nextAutoBuild } from '../latex-workspace/latex-autobuild';
import { readDevicePreferences, writeDevicePreferences } from './device-prefs';
import type {
  LatexBuildService,
  LatexBuildStatus,
  LatexProject,
  LatexProjectService,
} from '../latex-workspace/ports';
import {
  activeJob,
  artifactFreshness,
  buildErrorKey,
  diagnosticTarget,
  jobReasonKey,
  jobStateKey,
  lastFinishedJob,
  visibleDiagnostics,
} from '../latex-workspace/latex-build-view';
import './latex-workspace.css';
import type { FindKeys } from '../editor/find-replace';

interface Props {
  projects: LatexProjectService;
  builds: LatexBuildService;
  vaultId: string;
  projectId: string;
  translator: Translator;
  locale: SupportedLocaleId;
  findKeys?: FindKeys;
  onOpenEnvironment(): void;
  onClose(): void;
}

interface OpenSource {
  fileId: string;
  path: string;
  revision: number;
  /** Text as last read or saved. */
  saved: string;
}

/** Polls the host while a job runs or waits. */
const POLL_MS = 1000;

/**
 * F45/F51/F52 LaTeX workspace: the project's sources beside its PDF.
 *
 * Builds compile the project as saved on disk, so unsaved text is saved first.
 * Every job state and PDF comes from the host; the PDF shown is the published,
 * checked one, marked outdated once the sources changed after it was built.
 */
export function LatexWorkspace({ projects, builds, vaultId, projectId, translator, locale, findKeys, onOpenEnvironment, onClose }: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [project, setProject] = useState<LatexProject | null>(null);
  const [source, setSource] = useState<OpenSource | null>(null);
  const [draft, setDraft] = useState('');
  const [status, setStatus] = useState<LatexBuildStatus | null>(null);
  const [pdf, setPdf] = useState<{ artifactId: string; bytes: Uint8Array; label: string } | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [busy, setBusy] = useState(false);
  const pendingJump = useRef<number | null>(null);
  const caretLine = useRef(1);
  const jumpNonce = useRef(0);
  const [pdfJump, setPdfJump] = useState<{ page: number; nonce: number; mark: PdfMark } | null>(null);
  const [syncNotice, setSyncNotice] = useState<MessageKey | null>(null);
  const [exportNotice, setExportNotice] = useState<MessageKey | null>(null);
  const [jump, setJump] = useState<{ line: number; nonce: number } | null>(null);
  const [autoBuild, setAutoBuild] = useState(() => readDevicePreferences().latexAutoBuild);
  const [filesOpen, setFilesOpen] = useState(true);
  // Edits are counted so a build knows which ones it covered (see latex-autobuild).
  const [changeSeq, setChangeSeq] = useState(0);
  const [lastEditAt, setLastEditAt] = useState(0);
  const builtSeq = useRef(0);
  const [tick, setTick] = useState(0);

  const dirty = source !== null && draft !== source.saved;
  const running = activeJob(status);
  const finished = lastFinishedJob(status);
  const building = busy || running !== null;

  const openFile = useCallback(async (opened: LatexProject, fileId: string): Promise<boolean> => {
    const read = await projects.readProjectFile(vaultId, opened.projectId, fileId);
    if (!read.ok) {
      setError(read.error);
      return false;
    }
    setSource({ fileId, path: read.value.path, revision: read.value.revision, saved: read.value.text });
    setDraft(read.value.text);
    setChangeSeq(0);
    builtSeq.current = 0;
    return true;
  }, [projects, vaultId]);

  const reopen = useCallback(async (fileId: string | null): Promise<void> => {
    const opened = await projects.openProject(vaultId, projectId);
    if (!opened.ok) {
      setError(opened.error);
      return;
    }
    setProject(opened.value);
    await openFile(opened.value, fileId ?? opened.value.entryFileId);
  }, [projects, vaultId, projectId, openFile]);

  const refreshStatus = useCallback(async (): Promise<void> => {
    const outcome = await builds.status(vaultId, projectId);
    if (outcome.ok) setStatus(outcome.value);
  }, [builds, vaultId, projectId]);

  useEffect(() => {
    void reopen(null);
    void refreshStatus();
  }, [reopen, refreshStatus]);

  // Follow a running or waiting job until it ends.
  useEffect(() => {
    if (running === null) return;
    const timer = window.setInterval(() => void refreshStatus(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [running, refreshStatus]);

  // Load a newly published PDF; the previous one stays shown until then.
  const artifact = status?.artifact ?? null;
  useEffect(() => {
    if (artifact === null || artifact.artifactId === pdf?.artifactId) return;
    let cancelled = false;
    void builds.readArtifact(vaultId, projectId, artifact).then((outcome) => {
      if (cancelled) return;
      if (outcome.ok) setPdf({ artifactId: artifact.artifactId, bytes: outcome.value, label: artifact.label });
      else setError(outcome.error);
    });
    return () => { cancelled = true; };
  }, [artifact, pdf?.artifactId, builds, vaultId, projectId]);

  const pdfSource: PdfSource | null = useMemo(() => {
    if (pdf === null || project === null || artifact === null) return null;
    return { bytes: pdf.bytes, label: pdf.label, freshness: artifactFreshness(artifact, project.manifestHash) };
  }, [pdf, project, artifact]);

  const pdfOutdated = artifact !== null && project !== null && artifact.manifestHash !== project.manifestHash;
  const canLocate = artifact !== null && pdfSource !== null && artifact.synctex && !pdfOutdated;

  /** Source to PDF: scrolls the reader to the line and marks it. */
  async function locateInPdf(line: number): Promise<void> {
    if (project === null || source === null || artifact === null) return;
    if (!artifact.synctex) { setSyncNotice('latexWs.syncNone'); return; }
    if (pdfOutdated || dirty) { setSyncNotice('latexWs.syncStale'); return; }
    const spot = await builds.syncForward(vaultId, project.projectId, artifact, source.path, line);
    if (!spot.ok) { setSyncNotice(spot.error.messageKey === 'error.synctexStale' ? 'latexWs.syncStale' : 'latexWs.syncNone'); return; }
    if (spot.value === null) { setSyncNotice('latexWs.syncNoLine'); return; }
    setSyncNotice(null);
    jumpNonce.current += 1;
    const { page, x, y, width, height } = spot.value;
    setPdfJump({ page, nonce: jumpNonce.current, mark: { x, y, width, height } });
  }

  /** PDF to source: a Ctrl+click opens the line behind it. */
  async function locateInSource(page: number, x: number, y: number): Promise<void> {
    if (project === null || artifact === null) return;
    if (!artifact.synctex) { setSyncNotice('latexWs.syncNone'); return; }
    if (pdfOutdated || dirty) { setSyncNotice('latexWs.syncStale'); return; }
    const spot = await builds.syncInverse(vaultId, project.projectId, artifact, page, x, y);
    if (!spot.ok) { setSyncNotice(spot.error.messageKey === 'error.synctexStale' ? 'latexWs.syncStale' : 'latexWs.syncNone'); return; }
    const target = spot.value === null ? undefined : project.files.find((file) => file.path === spot.value?.file);
    if (spot.value === null || target === undefined) { setSyncNotice('latexWs.syncNoLine'); return; }
    setSyncNotice(null);
    pendingJump.current = spot.value.line;
    if (source?.fileId === target.fileId) setSource({ ...source });
    else await openFile(project, target.fileId);
  }

  // Put the caret on a diagnostic's line once its file is open.
  useEffect(() => {
    const line = pendingJump.current;
    if (line === null || source === null) return;
    pendingJump.current = null;
    setJump((previous) => ({ line, nonce: (previous?.nonce ?? 0) + 1 }));
  }, [source]);

  // Build on its own once typing pauses; never interrupts a running build.
  useEffect(() => {
    const step = nextAutoBuild({ enabled: autoBuild && editableNow(), changeSeq, builtSeq: builtSeq.current, lastEditAt, building }, Date.now());
    if (step.kind === 'wait') {
      const timer = window.setTimeout(() => setTick((value) => value + 1), step.ms);
      return () => window.clearTimeout(timer);
    }
    if (step.kind === 'build') void build();
    return undefined;
    // build reads the current draft through this render's closure.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoBuild, changeSeq, lastEditAt, building, tick]);

  /** Saves the open file; returns the manifest to build, or null. */
  async function save(): Promise<string | null> {
    if (project === null || source === null) return null;
    if (!dirty) return project.manifestHash;
    const text = draft;
    const outcome = await projects.saveProjectFile(vaultId, project.projectId, source.fileId, text, source.revision, newOperationId());
    if (!outcome.ok) {
      setError(outcome.error);
      return null;
    }
    setSource({ ...source, revision: outcome.value.revision, saved: text });
    setProject({
      ...project,
      manifestHash: outcome.value.manifestHash,
      files: project.files.map((file) => (file.fileId === source.fileId ? { ...file, revision: outcome.value.revision } : file)),
    });
    setError(null);
    return outcome.value.manifestHash;
  }

  /** F54-A03: saves first, so the ZIP holds what the editor shows. */
  async function exportZip(): Promise<void> {
    if (project === null || building) return;
    setBusy(true);
    setExportNotice(null);
    const manifest = await save();
    if (manifest !== null) {
      const outcome = await projects.exportSourceZip(vaultId, project.projectId, translator('latexWs.exportZip'));
      if (!outcome.ok) setError(outcome.error);
      else if (outcome.value) {
        setError(null);
        setExportNotice('latexWs.exportZipDone');
      }
    }
    setBusy(false);
  }

  async function savePdfCopy(): Promise<void> {
    if (project === null || artifact === null) return;
    setExportNotice(null);
    const outcome = await builds.savePdf(vaultId, project.projectId, translator('latexWs.savePdf'));
    if (!outcome.ok) setError(outcome.error);
    else if (outcome.value) {
      setError(null);
      setExportNotice('latexWs.savePdfDone');
    }
  }

  async function saveOnly(): Promise<void> {
    setBusy(true);
    await save();
    setBusy(false);
  }

  async function build(): Promise<void> {
    if (project === null || building) return;
    builtSeq.current = changeSeq;
    setBusy(true);
    const manifest = await save();
    if (manifest !== null) {
      const outcome = await builds.compile(vaultId, project.projectId, manifest, newOperationId());
      if (outcome.ok) {
        setError(null);
      } else {
        setError(outcome.error);
        // Changed on disk since it was read: show the current files.
        if (outcome.error.messageKey === 'error.buildManifestStale') await reopen(source?.fileId ?? null);
      }
      await refreshStatus();
    }
    setBusy(false);
  }

  async function cancel(): Promise<void> {
    if (running === null) return;
    const outcome = await builds.cancel(vaultId, running.jobId);
    if (!outcome.ok) setError(outcome.error);
    await refreshStatus();
  }

  async function jumpTo(diagnostic: TexDiagnostic): Promise<void> {
    if (project === null || finished?.report == null) return;
    const target = diagnosticTarget(diagnostic, finished.report, project);
    if (target === null) return;
    if (dirty && (await save()) === null) return;
    pendingJump.current = target.line;
    if (source?.fileId === target.fileId) {
      setSource({ ...source });
    } else {
      await openFile(project, target.fileId);
    }
  }

  /** Switches files, saving the open one first. */
  async function switchTo(fileId: string): Promise<void> {
    if (project === null) return;
    if (dirty && (await save()) === null) return;
    await openFile(project, fileId);
  }

  /** Closes after saving, so leaving never drops typing. */
  async function close(): Promise<void> {
    if (dirty && (await save()) === null) return;
    onClose();
  }

  function editableNow(): boolean {
    return source !== null && !(project?.readOnly ?? true) && (project?.files.find((file) => file.fileId === source.fileId)?.editable ?? false);
  }

  const errorKey: MessageKey | null = error === null ? null : buildErrorKey(error.messageKey);
  const report = finished?.report ?? null;
  const diagnostics = visibleDiagnostics(report);
  const problems = diagnostics.filter((diagnostic) => diagnostic.severity === 'error').length;
  const readOnly = project?.readOnly ?? true;
  const editable = editableNow();

  return (
    <section
      className="latex-ws"
      aria-labelledby="latex-ws-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        // An editor key the source pane already used, such as Esc closing its find box.
        if (event.nativeEvent.isComposing || event.defaultPrevented) return;
        if (event.key === 'Escape') {
          event.preventDefault();
          void close();
        }
      }}
    >
      <header className="latex-ws-head">
        <button type="button" className="latex-ws-icon" aria-label={translator(filesOpen ? 'latexWs.hideFiles' : 'latexWs.showFiles')}
          title={translator(filesOpen ? 'latexWs.hideFiles' : 'latexWs.showFiles')} onClick={() => setFilesOpen((open) => !open)}>
          {filesOpen ? <PanelLeftClose size={16} aria-hidden="true" /> : <PanelLeftOpen size={16} aria-hidden="true" />}
        </button>
        <h2 id="latex-ws-heading">{project === null ? translator('latexWs.title') : project.folder || translator('latexWs.title')}</h2>
        <span className="latex-ws-state" role="status">
          {running !== null ? translator(jobStateKey(running))
            : busy ? translator('latexWs.saving')
            : dirty ? translator(autoBuild ? 'latexWs.waiting' : 'latexWs.unsaved')
            : finished !== null ? translator(jobStateKey(finished)) : ''}
        </span>
        <div className="latex-ws-actions">
          {running !== null ? (
            <button type="button" className="secondary" onClick={() => void cancel()}>
              <Ban size={15} aria-hidden="true" /> {translator('latexWs.cancel')}
            </button>
          ) : null}
          <label className="latex-ws-auto" title={translator('latexWs.autoHint')}>
            <input type="checkbox" checked={autoBuild} onChange={(event) => {
              const next = event.currentTarget.checked;
              setAutoBuild(next);
              writeDevicePreferences({ latexAutoBuild: next });
            }} />
            {translator('latexWs.auto')}
          </label>
          <button type="button" className="latex-ws-icon" disabled={project === null || building}
            aria-label={translator('latexWs.exportZip')} title={translator('latexWs.exportZipHint')} onClick={() => void exportZip()}>
            <FileArchive size={16} aria-hidden="true" />
          </button>
          <button type="button" className="latex-ws-icon" disabled={project === null || artifact === null}
            aria-label={translator('latexWs.savePdf')} title={translator(artifact === null ? 'latexWs.savePdfNone' : 'latexWs.savePdf')} onClick={() => void savePdfCopy()}>
            <FileDown size={16} aria-hidden="true" />
          </button>
          <button type="button" className="primary" disabled={project === null || building} title={translator('latexWs.buildHint')} onClick={() => void build()}>
            <Hammer size={15} aria-hidden="true" /> {translator('latexWs.build')}
          </button>
          <button
            type="button"
            className="latex-ws-close"
            aria-label={translator('latexWs.close')}
            title={translator('latexWs.close')}
            onClick={() => void close()}
          >
            <X size={18} aria-hidden="true" />
          </button>
        </div>
      </header>

      {error !== null ? (
        <div className="latex-ws-error" role="alert">
          <p>{errorKey !== null ? translator(errorKey) : serviceErrorMessage(locale, error.code)}</p>
          {error.messageKey === 'error.latexEnvironmentNotReady' ? (
            <button type="button" className="secondary" onClick={onOpenEnvironment}>{translator('latexEnv.title')}</button>
          ) : null}
        </div>
      ) : null}

      <div className="latex-ws-body" data-files={filesOpen ? 'open' : 'closed'}>
        {filesOpen ? <nav className="latex-ws-files" aria-label={translator('latexWs.files')}>
          <ul>
            {(project?.files ?? []).map((file) => (
              <li key={file.fileId}>
                <button
                  type="button"
                  aria-current={file.fileId === source?.fileId ? 'true' : undefined}
                  disabled={!file.editable || building || file.fileId === source?.fileId}
                  title={file.path}
                  onClick={() => void switchTo(file.fileId)}
                >
                  <FileText size={14} aria-hidden="true" />
                  <span>{file.path}</span>
                  {file.fileId === project?.entryFileId ? <span className="latex-ws-entry">{translator('latexWs.entry')}</span> : null}
                </button>
              </li>
            ))}
          </ul>
        </nav> : null}

        <div className="latex-ws-source">
          <div className="latex-ws-path">
            <span>{source?.path ?? ''}</span>
            {readOnly && project !== null ? <span className="latex-ws-dirty">{translator('latexWs.readOnly')}</span> : null}
            {syncNotice !== null ? <span className="latex-ws-sync-note" role="status">{translator(syncNotice)}</span> : null}
            {exportNotice !== null ? <span className="latex-ws-sync-note" role="status">{translator(exportNotice)}</span> : null}
            <button type="button" className="secondary latex-ws-locate" disabled={!canLocate} title={translator(canLocate ? 'latexWs.locateHint' : artifact !== null && !artifact.synctex ? 'latexWs.syncNone' : 'latexWs.syncStale')}
              onClick={() => void locateInPdf(caretLine.current)}>
              {translator('latexWs.locate')}
            </button>
          </div>
          {source !== null ? (
            <LatexSourceEditor
              fileKey={source.fileId}
              value={draft}
              readOnly={!editable}
              label={source.path}
              translator={translator}
              findKeys={findKeys}
              onLocate={(line) => void locateInPdf(line)}
              onCaretLine={(line) => { caretLine.current = line; }}
              jump={jump}
              onChange={(text) => {
                setDraft(text);
                setChangeSeq((value) => value + 1);
                setLastEditAt(Date.now());
              }}
              onBuild={() => void build()}
              onSave={() => { if (editable && !busy) void saveOnly(); }}
            />
          ) : <div className="latex-ws-editor" />}

          {finished !== null || diagnostics.length > 0 ? <details className="latex-ws-diagnostics" aria-label={translator('latexWs.diagnostics')}>
            <summary data-problems={problems > 0 ? 'true' : undefined}>
              <ChevronRight className="latex-ws-chevron" size={14} aria-hidden="true" />
              {problems > 0 ? translator('latexWs.problems', { count: problems })
                : diagnostics.length > 0 ? translator('latexWs.notes', { count: diagnostics.length })
                : translator('latexWs.noProblems')}
            </summary>
            {finished !== null ? (
              <p className="latex-ws-result">
                {translator(jobStateKey(finished))}
                {jobReasonKey(finished) !== null ? ` ${translator(jobReasonKey(finished) as MessageKey)}` : ''}
                {finished.previousSession ? ` ${translator('latexWs.previousSession')}` : ''}
              </p>
            ) : null}
            {report !== null && report.missing.length > 0 ? (
              <p className="latex-ws-missing">
                {translator('latexWs.missing', { names: report.missing.map((item) => item.name).join(', ') })}
              </p>
            ) : null}
            {diagnostics.length > 0 ? (
              <ul>
                {diagnostics.map((diagnostic) => {
                  const target = project !== null && report !== null ? diagnosticTarget(diagnostic, report, project) : null;
                  const where = diagnostic.file !== null && diagnostic.line !== null
                    ? `${diagnostic.file}:${diagnostic.line}`
                    : diagnostic.line !== null ? translator('latexWs.line', { line: diagnostic.line }) : '';
                  return (
                    <li key={diagnostic.offset} data-severity={diagnostic.severity}>
                      <button type="button" disabled={target === null || building} onClick={() => void jumpTo(diagnostic)}>
                        <span className="latex-ws-where">{where}</span>
                        <span>{translator(diagnosticKey(diagnostic))}</span>
                      </button>
                      <pre>{diagnostic.raw}</pre>
                    </li>
                  );
                })}
              </ul>
            ) : null}
          </details> : null}
        </div>

        <div className="latex-ws-pdf" data-building={building ? 'true' : undefined}>
          {building && pdfSource !== null ? <span className="latex-ws-updating">{translator('latexWs.updating')}</span> : null}
          <PdfPane source={pdfSource} translator={translator} jump={pdfJump} onLocate={(page, x, y) => void locateInSource(page, x, y)} />
        </div>
      </div>
    </section>
  );
}

const DIAGNOSTIC_KEYS: Readonly<Record<TexDiagnostic['code'], MessageKey>> = {
  texError: 'latexWs.diag.texError',
  missingPackage: 'latexWs.diag.missing',
  missingFont: 'latexWs.diag.missing',
  undefinedControlSequence: 'latexWs.diag.undefined',
  mathMode: 'latexWs.diag.math',
  runawayArgument: 'latexWs.diag.runaway',
  boxWarning: 'latexWs.diag.box',
  unknownDiagnostic: 'latexWs.diag.other',
};

function diagnosticKey(diagnostic: TexDiagnostic): MessageKey {
  return diagnostic.severity === 'warning' && diagnostic.code === 'texError' ? 'latexWs.diag.warning' : DIAGNOSTIC_KEYS[diagnostic.code];
}
