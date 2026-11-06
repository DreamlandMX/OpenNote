import type { TexBuildReport, TexDiagnostic } from '../editor';
import type { MessageKey } from '../presentation-i18n';
import type { LatexArtifact, LatexBuildJob, LatexBuildStatus, LatexProject } from './ports';
import type { PdfFreshness } from './pdf-view';

/** A PDF built from another manifest than the project's current one is outdated. */
export function artifactFreshness(artifact: LatexArtifact, manifestHash: string): PdfFreshness {
  return artifact.manifestHash === manifestHash ? 'current' : 'outdated';
}

/** The job still running or waiting, if any (newest first). */
export function activeJob(status: LatexBuildStatus | null): LatexBuildJob | null {
  return status?.jobs.find((job) => job.state === 'running' || job.state === 'queued' || job.state === 'preparing') ?? null;
}

/** The newest finished job; its report is what the diagnostics show. */
export function lastFinishedJob(status: LatexBuildStatus | null): LatexBuildJob | null {
  return status?.jobs.find((job) => job.state !== 'running' && job.state !== 'queued' && job.state !== 'preparing') ?? null;
}

const STATE_KEYS: Readonly<Record<LatexBuildJob['state'], MessageKey>> = {
  preparing: 'latexWs.state.running',
  queued: 'latexWs.state.queued',
  running: 'latexWs.state.running',
  succeeded: 'latexWs.state.succeeded',
  failed: 'latexWs.state.failed',
  cancelled: 'latexWs.state.cancelled',
  timedOut: 'latexWs.state.timedOut',
  interrupted: 'latexWs.state.interrupted',
};

export function jobStateKey(job: LatexBuildJob): MessageKey {
  return STATE_KEYS[job.state];
}

const REASON_KEYS: Readonly<Record<string, MessageKey>> = {
  'build.failed': 'latexWs.reason.failed',
  'build.noPdf': 'latexWs.reason.noPdf',
  'build.tooLarge': 'latexWs.reason.tooLarge',
  'build.invalidPdf': 'latexWs.reason.invalidPdf',
  'build.sandbox': 'latexWs.reason.sandbox',
  'build.environmentMissing': 'latexWs.reason.environmentMissing',
  'build.io': 'latexWs.reason.io',
  'build.interrupted': 'latexWs.reason.interrupted',
  'build.superseded': 'latexWs.reason.superseded',
};

/** Why a job ended without a PDF; an unknown key gets the generic caption. */
export function jobReasonKey(job: LatexBuildJob): MessageKey | null {
  if (job.state === 'succeeded' || job.state === 'cancelled' || job.state === 'timedOut') return null;
  if (job.reasonKey === null) return null;
  return REASON_KEYS[job.reasonKey] ?? 'latexWs.reason.unknown';
}

const ERROR_KEYS: Readonly<Record<string, MessageKey>> = {
  'error.latexEnvironmentNotReady': 'latexWs.error.environment',
  'error.latexPrepareRunning': 'latexWs.error.preparing',
  'error.buildManifestStale': 'latexWs.error.stale',
  'error.projectFileChanged': 'latexWs.error.fileChanged',
  'error.latexBuildRunning': 'latexWs.error.building',
  'error.buildArtifactChanged': 'latexWs.error.artifactChanged',
  'error.buildArtifactMissing': 'latexWs.error.artifactChanged',
  'error.latexProjectsUnavailable': 'project.error.unavailable',
  'error.exportTargetExists': 'latexWs.error.targetExists',
  'error.latexZipTooLarge': 'latexWs.error.zipTooLarge',
};

/** A specific caption for host errors whose generic code would mislead. */
export function buildErrorKey(messageKey: string): MessageKey | null {
  return ERROR_KEYS[messageKey] ?? null;
}

/** Where a diagnostic may jump to, or `null`. */
export interface DiagnosticTarget {
  fileId: string;
  line: number;
}

/**
 * A diagnostic is located only when the engine named both a file of this
 * project and a line, and the report was produced from the manifest that is
 * open now; otherwise the line could point at text that moved (F53-R03).
 */
export function diagnosticTarget(
  diagnostic: TexDiagnostic,
  report: TexBuildReport,
  project: LatexProject,
): DiagnosticTarget | null {
  if (report.sourceHash !== project.manifestHash) return null;
  if (diagnostic.file === null || diagnostic.line === null || diagnostic.line < 1) return null;
  const file = project.files.find((candidate) => candidate.path === diagnostic.file && candidate.editable);
  return file === undefined ? null : { fileId: file.fileId, line: diagnostic.line };
}

/** Character offset of the start of 1-based `line`, clamped to the text. */
export function lineOffset(text: string, line: number): number {
  let offset = 0;
  for (let current = 1; current < line; current += 1) {
    const next = text.indexOf('\n', offset);
    if (next < 0) return text.length;
    offset = next + 1;
  }
  return offset;
}

/** Errors first, then warnings; informational lines are left to the raw log. */
export function visibleDiagnostics(report: TexBuildReport | null): readonly TexDiagnostic[] {
  if (report === null) return [];
  const rank = { error: 0, warning: 1, info: 2 } as const;
  return report.diagnostics
    .filter((diagnostic) => diagnostic.severity !== 'info')
    .slice()
    .sort((left, right) => rank[left.severity] - rank[right.severity] || left.offset - right.offset);
}
