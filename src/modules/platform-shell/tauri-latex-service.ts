import { invoke } from '@tauri-apps/api/core';
import type { CompileRequestOutcome, JobState, ProjectInspection, ServiceOutcome } from '../editor';
import type {
  ChosenLatexFolder,
  LatexArtifact,
  LatexBuildService,
  LatexBuildStatus,
  LatexCleanupReport,
  LatexCreateOutcome,
  LatexEnvironmentPlan,
  LatexEnvironmentService,
  LatexEnvironmentStatus,
  LatexProject,
  LatexProjectService,
  LatexProjectSource,
  LatexSaveResult,
  LatexProjectSummary,
} from '../latex-workspace/ports';
import { serviceErrorFromThrown } from './port';

function call<T>(command: string, args: Record<string, unknown>): Promise<ServiceOutcome<T>> {
  return invoke<T>(command, args).then(
    (value) => ({ ok: true as const, value }),
    (thrown: unknown) => ({ ok: false as const, error: serviceErrorFromThrown(thrown) }),
  );
}

/** The only Tauri adapter for F45 LaTeX projects. */
export function createTauriLatexService(): LatexProjectService {
  return {
    listProjects(vaultId) {
      return call<readonly LatexProjectSummary[]>('list_latex_projects', { vaultId });
    },
    unregisterProject(vaultId, projectId, operationId) {
      return call<null>('unregister_latex_project', { vaultId, projectId, operationId });
    },
    createProject(vaultId, request, operationId) {
      return call<LatexCreateOutcome>('create_latex_project', { vaultId, ...request, operationId });
    },
    chooseProjectFolder(vaultId, dialogTitle) {
      return call<ChosenLatexFolder | null>('select_latex_project_folder', { vaultId, dialogTitle });
    },
    inspectProject(vaultId, selectionId) {
      return call<ProjectInspection>('inspect_latex_folder', { vaultId, selectionId });
    },
    registerProject(vaultId, selectionId, entryPath, operationId) {
      return call<LatexProject>('register_latex_project', { vaultId, selectionId, entryPath, operationId });
    },
    openProject(vaultId, projectId) {
      return call<LatexProject>('open_latex_project', { vaultId, projectId });
    },
    readProjectFile(vaultId, projectId, fileId) {
      return call<LatexProjectSource>('read_latex_project_file', { vaultId, projectId, fileId });
    },
    saveProjectFile(vaultId, projectId, fileId, text, expectedRevision, operationId) {
      return call<LatexSaveResult>('save_latex_project_file', {
        vaultId,
        projectId,
        fileId,
        text,
        expectedRevision,
        operationId,
      });
    },
    exportSourceZip(vaultId, projectId, dialogTitle) {
      return call<boolean>('export_latex_source_zip', { vaultId, projectId, dialogTitle });
    },
  };
}

const failure = <T>(code: 'NOT_IMPLEMENTED' | 'VERSION_CONFLICT', messageKey: string): ServiceOutcome<T> => ({
  ok: false as const,
  error: { code, messageKey, params: {}, retryable: false, operationId: null, retryAfterSeconds: null },
});

const unavailable = <T>(): Promise<ServiceOutcome<T>> =>
  Promise.resolve(failure<T>('NOT_IMPLEMENTED', 'error.latexProjectsUnavailable'));

/** Used outside the desktop host (browser preview, tests): every call fails visibly. */
export const disconnectedLatexService: LatexProjectService = {
  listProjects: () => unavailable(),
  unregisterProject: () => unavailable(),
  createProject: () => unavailable(),
  chooseProjectFolder: () => unavailable(),
  inspectProject: () => unavailable(),
  registerProject: () => unavailable(),
  openProject: () => unavailable(),
  readProjectFile: () => unavailable(),
  saveProjectFile: () => unavailable(),
  exportSourceZip: () => unavailable(),
};

export function resolveLatexService(): LatexProjectService {
  const internals = (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  return internals === undefined ? disconnectedLatexService : createTauriLatexService();
}
/** The only Tauri adapter for the LaTeX environment. */
export function createTauriLatexEnvironment(): LatexEnvironmentService {
  return {
    plan: () => call<LatexEnvironmentPlan>('latex_environment_plan', {}),
    status: () => call<LatexEnvironmentStatus>('latex_environment_status', {}),
    prepare: (consent, operationId) => call<string>('prepare_latex_environment', { consent, operationId }),
    cancel: () => call<null>('cancel_latex_preparation', {}),
    remove: () => call<LatexCleanupReport>('remove_latex_environment', {}),
  };
}

export const disconnectedLatexEnvironment: LatexEnvironmentService = {
  plan: () => unavailable(),
  status: () => unavailable(),
  prepare: () => unavailable(),
  cancel: () => unavailable(),
  remove: () => unavailable(),
};

export function resolveLatexEnvironment(): LatexEnvironmentService {
  const internals = (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  return internals === undefined ? disconnectedLatexEnvironment : createTauriLatexEnvironment();
}

/** Chunk size for reading a PDF; the host never returns more per call. */
export const ARTIFACT_CHUNK_BYTES = 8 * 1024 * 1024;

async function sha256Hex(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

/** The only Tauri adapter for F51/F52 builds and their PDFs. */
export function createTauriLatexBuilds(): LatexBuildService {
  return {
    compile: (vaultId, projectId, expectedManifestHash, operationId) =>
      call<CompileRequestOutcome>('compile_latex_project', { vaultId, projectId, expectedManifestHash, operationId }),
    cancel: (vaultId, jobId) => call<JobState>('cancel_latex_compile', { vaultId, jobId }),
    status: (vaultId, projectId) => call<LatexBuildStatus>('latex_build_status', { vaultId, projectId }),
    syncForward(vaultId, projectId, artifact, path, line) {
      return call('synctex_forward', { vaultId, projectId, artifactId: artifact.artifactId, expectedManifestHash: artifact.manifestHash, path, line });
    },
    syncInverse(vaultId, projectId, artifact, page, x, y) {
      return call('synctex_inverse', { vaultId, projectId, artifactId: artifact.artifactId, expectedManifestHash: artifact.manifestHash, page, x, y });
    },
    async readArtifact(vaultId, projectId, artifact: LatexArtifact) {
      const bytes = new Uint8Array(artifact.bytes);
      for (let offset = 0; offset < artifact.bytes; ) {
        const length = Math.min(ARTIFACT_CHUNK_BYTES, artifact.bytes - offset);
        const chunk = await call<ArrayBuffer>('read_latex_artifact', {
          vaultId, projectId, artifactId: artifact.artifactId, offset, length,
        });
        if (!chunk.ok) return chunk;
        const part = new Uint8Array(chunk.value);
        // An empty or oversized chunk means the file is not the recorded one.
        if (part.byteLength === 0 || part.byteLength > length) return failure('VERSION_CONFLICT', 'error.buildArtifactChanged');
        bytes.set(part, offset);
        offset += part.byteLength;
      }
      // The assembled bytes must be the ones the host checked and published.
      if (await sha256Hex(bytes) !== artifact.sha256) return failure('VERSION_CONFLICT', 'error.buildArtifactChanged');
      return { ok: true as const, value: bytes };
    },
    savePdf: (vaultId, projectId, dialogTitle) => call<boolean>('save_latex_pdf', { vaultId, projectId, dialogTitle }),
  };
}

export const disconnectedLatexBuilds: LatexBuildService = {
  compile: () => unavailable(),
  cancel: () => unavailable(),
  status: () => unavailable(),
  syncForward: () => unavailable(),
  syncInverse: () => unavailable(),
  readArtifact: () => unavailable(),
  savePdf: () => unavailable(),
};

export function resolveLatexBuilds(): LatexBuildService {
  const internals = (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  return internals === undefined ? disconnectedLatexBuilds : createTauriLatexBuilds();
}
