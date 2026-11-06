import { describe, expect, it } from 'vitest';
import type { TexBuildReport, TexDiagnostic } from '../editor';
import type { LatexBuildJob, LatexBuildStatus, LatexProject } from './ports';
import {
  activeJob,
  artifactFreshness,
  buildErrorKey,
  diagnosticTarget,
  jobReasonKey,
  lastFinishedJob,
  lineOffset,
  visibleDiagnostics,
} from './latex-build-view';

const project: LatexProject = {
  projectId: 'p',
  folder: 'thesis',
  entryFileId: 'f1',
  files: [
    { fileId: 'f1', path: 'main.tex', revision: 1, editable: true },
    { fileId: 'f2', path: 'chapters/one.tex', revision: 1, editable: true },
    { fileId: 'f3', path: 'fig.png', revision: 1, editable: false },
  ],
  recovery: 'nothing',
  readOnly: false,
  manifestHash: 'm2',
};

function job(state: LatexBuildJob['state'], overrides: Partial<LatexBuildJob> = {}): LatexBuildJob {
  return { jobId: state, operationId: 'op', state, reasonKey: null, manifestHash: 'm', artifactId: null, previousSession: false, report: null, ...overrides };
}

function diagnostic(overrides: Partial<TexDiagnostic> = {}): TexDiagnostic {
  return { severity: 'error', code: 'undefinedControlSequence', raw: '! Undefined control sequence.', messageKey: 'x', file: 'chapters/one.tex', line: 3, offset: 0, ...overrides };
}

function report(sourceHash: string, diagnostics: TexDiagnostic[] = []): TexBuildReport {
  return { diagnostics, unparsed: [], missing: [], truncated: false, exitCode: 1, succeeded: false, verdict: 'failed', jobId: 'j', sourceHash };
}

describe('latex build view', () => {
  it('marks a PDF built from another manifest as outdated', () => {
    const artifact = { artifactId: 'a', jobId: 'j', manifestHash: 'm1', label: 'main.pdf', bytes: 1, sha256: 's', pages: 1, synctex: true, builtAt: 0 };
    expect(artifactFreshness(artifact, 'm1')).toBe('current');
    expect(artifactFreshness(artifact, 'm2')).toBe('outdated');
  });

  it('separates the job in progress from the last finished one', () => {
    const status: LatexBuildStatus = { jobs: [job('queued'), job('running'), job('failed')], artifact: null };
    expect(activeJob(status)?.state).toBe('queued');
    expect(lastFinishedJob(status)?.state).toBe('failed');
    expect(activeJob({ jobs: [job('succeeded')], artifact: null })).toBeNull();
    expect(activeJob(null)).toBeNull();
  });

  it('locates a diagnostic only in a project file, with a line, from the open manifest', () => {
    expect(diagnosticTarget(diagnostic(), report('m2'), project)).toEqual({ fileId: 'f2', line: 3 });
    // Built from an older manifest: the line may have moved (F53-R03).
    expect(diagnosticTarget(diagnostic(), report('m1'), project)).toBeNull();
    // The engine named no file: never guessed.
    expect(diagnosticTarget(diagnostic({ file: null }), report('m2'), project)).toBeNull();
    expect(diagnosticTarget(diagnostic({ file: 'article.cls' }), report('m2'), project)).toBeNull();
    expect(diagnosticTarget(diagnostic({ file: 'fig.png' }), report('m2'), project)).toBeNull();
    expect(diagnosticTarget(diagnostic({ line: null }), report('m2'), project)).toBeNull();
  });

  it('finds the start of a line and clamps past the end', () => {
    expect(lineOffset('a\nbb\nccc', 1)).toBe(0);
    expect(lineOffset('a\nbb\nccc', 3)).toBe(5);
    expect(lineOffset('a\nbb', 9)).toBe(4);
  });

  it('lists errors before warnings and leaves informational lines out', () => {
    const shown = visibleDiagnostics(report('m', [
      diagnostic({ severity: 'warning', code: 'boxWarning', offset: 1 }),
      diagnostic({ severity: 'info', code: 'unknownDiagnostic', offset: 2 }),
      diagnostic({ severity: 'error', offset: 9 }),
    ]));
    expect(shown.map((item) => item.severity)).toEqual(['error', 'warning']);
    expect(visibleDiagnostics(null)).toEqual([]);
  });

  it('never shows a raw or unknown host key', () => {
    expect(jobReasonKey(job('failed', { reasonKey: 'build.invalidPdf' }))).toBe('latexWs.reason.invalidPdf');
    expect(jobReasonKey(job('failed', { reasonKey: 'build.somethingNew' }))).toBe('latexWs.reason.unknown');
    expect(jobReasonKey(job('cancelled', { reasonKey: 'build.cancelled' }))).toBeNull();
    expect(buildErrorKey('error.latexEnvironmentNotReady')).toBe('latexWs.error.environment');
    expect(buildErrorKey('error.other')).toBeNull();
  });
});
