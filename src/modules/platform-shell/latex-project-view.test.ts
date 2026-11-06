import { describe, expect, it } from 'vitest';
import type { ProjectInspection } from '../editor';
import type { ServiceError } from '../domain';
import type { LatexEnvironmentStatus } from '../latex-workspace/ports';
import { environmentStepKey, projectErrorKey, registrableEntry } from './latex-project-view';

function inspection(overrides: Partial<ProjectInspection> = {}): ProjectInspection {
  return {
    root: 'thesis',
    entryCandidates: [
      { path: 'main.tex', score: 100, reasonKey: 'project.entryDeclaresClass' },
      { path: 'alt.tex', score: 100, reasonKey: 'project.entryDeclaresClass' },
    ],
    entry: null,
    files: ['main.tex', 'alt.tex'],
    dependencies: [],
    findings: [{ kind: 'entryAmbiguous', candidates: ['main.tex', 'alt.tex'] }],
    inspectable: false,
    ...overrides,
  };
}

function error(messageKey: string): ServiceError {
  return { code: 'PATH_COLLISION', messageKey, params: {}, retryable: false, operationId: null, retryAfterSeconds: null };
}

describe('registrableEntry', () => {
  it('lets the user resolve an ambiguous entry instead of guessing one', () => {
    expect(registrableEntry(inspection(), null)).toBeNull();
    expect(registrableEntry(inspection(), 'alt.tex')).toBe('alt.tex');
  });

  it('only accepts an inspected candidate', () => {
    expect(registrableEntry(inspection(), 'other.tex')).toBeNull();
    expect(registrableEntry(null, 'main.tex')).toBeNull();
  });

  it('refuses a project with a safety finding even when an entry is chosen', () => {
    for (const finding of [
      { kind: 'shellEscape', from: 'main.tex' },
      { kind: 'unsafeReference', reference: '../x', from: 'main.tex' },
    ] as const) {
      expect(registrableEntry(inspection({ findings: [finding] }), 'main.tex')).toBeNull();
    }
  });

  it('keeps reported-only findings from blocking registration', () => {
    const reported = inspection({
      entry: 'main.tex',
      findings: [{ kind: 'missingReference', reference: 'fig.png', from: 'main.tex' }],
    });
    expect(registrableEntry(reported, 'main.tex')).toBe('main.tex');
  });
});

describe('projectErrorKey', () => {
  it('names a duplicate registration instead of a generic path collision', () => {
    expect(projectErrorKey(error('error.projectAlreadyRegistered'))).toBe('project.error.alreadyRegistered');
    expect(projectErrorKey(error('error.somethingElse'))).toBeNull();
  });
});

describe('environment step', () => {
  const idle = { preparing: null, last: null };
  it('reads checking, unknown, preparing, ready and missing', () => {
    expect(environmentStepKey(undefined)).toBe('project.env.checking');
    expect(environmentStepKey(null)).toBe('project.env.unknown');
    const ready: LatexEnvironmentStatus = { ...idle, environment: { kind: 'ready', totalBytes: 1, fileCount: 1 } };
    expect(environmentStepKey(ready)).toBe('project.env.ready');
    expect(environmentStepKey({ ...idle, environment: { kind: 'notPrepared' } })).toBe('project.env.missing');
    // A failed verification is not ready, whatever it holds.
    expect(environmentStepKey({ ...idle, environment: { kind: 'invalid', reasonKey: 'x' } })).toBe('project.env.missing');
    const preparing: LatexEnvironmentStatus = {
      ...ready,
      preparing: { operationId: 'op', progress: { stage: 'downloading', bytes: 0, totalBytes: null, step: 1, steps: 5 } },
    };
    expect(environmentStepKey(preparing)).toBe('project.env.preparing');
  });
});
