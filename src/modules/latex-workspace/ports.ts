import type {
  CompileRequestOutcome,
  JobState,
  LatexMetadata,
  LatexTemplateId,
  ProjectInspection,
  ScaffoldPlan,
  ServiceOutcome,
  TexBuildReport,
} from '../editor';

/**
 * F45 LaTeX project boundary. The frontend never submits a disk path: a picked
 * folder is a `selectionId`, and files are addressed by the host's `fileId`.
 * Sources are saved against the revision they were read at; a change made
 * elsewhere is a VERSION_CONFLICT and is never overwritten.
 */

export interface ChosenLatexFolder {
  selectionId: string;
  /** Vault-relative folder for display; empty for the vault root. */
  folder: string;
}

export interface LatexProjectFile {
  fileId: string;
  /** Relative to the project folder. */
  path: string;
  revision: number;
  /** Whether a source save may write this file (`.tex/.bib/.sty/.cls/.bst`). */
  editable: boolean;
}

/** How an interrupted save was settled when the project was opened. */
export type LatexRecovery = 'nothing' | 'completed' | 'abandoned' | 'external';

export interface LatexProject {
  projectId: string;
  folder: string;
  entryFileId: string;
  files: readonly LatexProjectFile[];
  recovery: LatexRecovery;
  readOnly: boolean;
  /** Identity of the whole input; a build request names the one it was shown. */
  manifestHash: string;
}

export interface LatexProjectSource {
  projectId: string;
  fileId: string;
  path: string;
  text: string;
  revision: number;
}

export interface LatexSaveResult {
  fileId: string;
  revision: number;
  /** False when nothing had to be written (a retry, or unchanged text). */
  written: boolean;
  /** The project's manifest after this save. */
  manifestHash: string;
}

export interface LatexProjectSummary {
  projectId: string;
  folder: string | null;
  entryPath: string | null;
  /** False when this build cannot read the record; it can still be removed. */
  readable: boolean;
}

export interface LatexCreateOutcome {
  plan: ScaffoldPlan;
  /** Present only when every file was written and the project registered. */
  project: LatexProject | null;
}

export interface LatexProjectService {
  listProjects(vaultId: string): Promise<ServiceOutcome<readonly LatexProjectSummary[]>>;
  /** Removes the registration only; the project's files are never deleted. */
  unregisterProject(vaultId: string, projectId: string, operationId: string): Promise<ServiceOutcome<null>>;
  /** Writes a template into a new folder (never over an existing file) and registers it. */
  createProject(vaultId: string, request: { templateId: LatexTemplateId; metadata: LatexMetadata; destination: string }, operationId: string): Promise<ServiceOutcome<LatexCreateOutcome>>;
  /** Native picker inside the vault; `null` when cancelled. */
  chooseProjectFolder(vaultId: string, dialogTitle: string): Promise<ServiceOutcome<ChosenLatexFolder | null>>;
  inspectProject(vaultId: string, selectionId: string): Promise<ServiceOutcome<ProjectInspection>>;
  /** `entryPath` is one of the inspection's candidates; the operation id becomes the project id. */
  registerProject(vaultId: string, selectionId: string, entryPath: string, operationId: string): Promise<ServiceOutcome<LatexProject>>;
  openProject(vaultId: string, projectId: string): Promise<ServiceOutcome<LatexProject>>;
  readProjectFile(vaultId: string, projectId: string, fileId: string): Promise<ServiceOutcome<LatexProjectSource>>;
  saveProjectFile(vaultId: string, projectId: string, fileId: string, text: string, expectedRevision: number, operationId: string): Promise<ServiceOutcome<LatexSaveResult>>;
  /** F54-A03: the whole project folder as a ZIP, saved where the user picks. `false` when cancelled. */
  exportSourceZip(vaultId: string, projectId: string, dialogTitle: string): Promise<ServiceOutcome<boolean>>;
}

/** What a preparation would download; shown before the user agrees. */
export interface LatexEnvironmentPlan {
  engineName: string;
  engineVersion: string;
  engineLicense: string;
  engineHost: string;
  engineDownloadBytes: number;
  resourceHost: string;
  resourceEstimatedBytes: number;
  resourceMaxBytes: number;
  prepareSeconds: number;
}

export type LatexEnvironmentState =
  | { kind: 'notPrepared' }
  | { kind: 'ready'; totalBytes: number; fileCount: number }
  /** Present but failed verification; treated exactly like not prepared. */
  | { kind: 'invalid'; reasonKey: string };

export type LatexPrepareStage = 'downloading' | 'verifying' | 'fetchingResources' | 'checkingOffline' | 'activating';

export interface LatexEnvironmentStatus {
  environment: LatexEnvironmentState;
  preparing: {
    operationId: string;
    progress: { stage: LatexPrepareStage; bytes: number; totalBytes: number | null; step: number; steps: number };
  } | null;
  last: { operationId: string; succeeded: boolean; reasonKey: string | null } | null;
}

export interface LatexCleanupReport {
  revoked: number;
  profilesDeleted: number;
  runtimeRemoved: boolean;
  errors: readonly string[];
}

/**
 * The local engine environment. Nothing is downloaded until prepare is
 * called with consent: true after the plan was shown to the user.
 */
export interface LatexEnvironmentService {
  plan(): Promise<ServiceOutcome<LatexEnvironmentPlan>>;
  status(): Promise<ServiceOutcome<LatexEnvironmentStatus>>;
  prepare(consent: boolean, operationId: string): Promise<ServiceOutcome<string>>;
  cancel(): Promise<ServiceOutcome<null>>;
  remove(): Promise<ServiceOutcome<LatexCleanupReport>>;
}

/** One build job as the host observed it. */
export interface LatexBuildJob {
  /** The live job id, or the operation id of a job from an earlier session. */
  jobId: string;
  operationId: string;
  state: JobState;
  reasonKey: string | null;
  manifestHash: string;
  artifactId: string | null;
  previousSession: boolean;
  report: TexBuildReport | null;
}

/** The published, checked PDF (without its bytes). */
export interface LatexArtifact {
  artifactId: string;
  jobId: string;
  manifestHash: string;
  label: string;
  bytes: number;
  sha256: string;
  pages: number;
  /** Whether this PDF has a source mapping to jump with. */
  synctex: boolean;
  builtAt: number;
}

/** A spot on a PDF page, in PDF points from the page's top left. */
export interface PdfSpot {
  page: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** A source line, by project-relative path. */
export interface SourceSpot {
  file: string;
  line: number;
}

export interface LatexBuildStatus {
  /** Newest first. */
  jobs: readonly LatexBuildJob[];
  artifact: LatexArtifact | null;
}

/**
 * F51/F52 real builds. The host takes the snapshot, runs the engine offline,
 * checks the PDF and publishes it; the frontend only asks, cancels and reads.
 */
export interface LatexBuildService {
  compile(vaultId: string, projectId: string, expectedManifestHash: string, operationId: string): Promise<ServiceOutcome<CompileRequestOutcome>>;
  cancel(vaultId: string, jobId: string): Promise<ServiceOutcome<JobState>>;
  status(vaultId: string, projectId: string): Promise<ServiceOutcome<LatexBuildStatus>>;
  /** The artifact's bytes, read in bounded chunks and checked against its hash. */
  /** F52: where a source line landed. Null when it produced nothing on the page. */
  syncForward(vaultId: string, projectId: string, artifact: LatexArtifact, path: string, line: number): Promise<ServiceOutcome<PdfSpot | null>>;
  /** F52: the source behind a point on a page. Refused if the source has since changed. */
  syncInverse(vaultId: string, projectId: string, artifact: LatexArtifact, page: number, x: number, y: number): Promise<ServiceOutcome<SourceSpot | null>>;
  readArtifact(vaultId: string, projectId: string, artifact: LatexArtifact): Promise<ServiceOutcome<Uint8Array>>;
  /** A copy of the current PDF, saved where the user picks. `false` when cancelled. */
  savePdf(vaultId: string, projectId: string, dialogTitle: string): Promise<ServiceOutcome<boolean>>;
}
