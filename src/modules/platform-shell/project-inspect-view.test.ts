import { describe, expect, it } from 'vitest';
import type { ProjectInspection } from '../editor';
import type { Translator } from '../presentation-i18n';
import {
  fileCount,
  findingKey,
  findingParams,
  findingText,
  hasAmbiguity,
  isBlocking,
  partitionFindings,
  parseFileListing,
  rankedCandidates,
  unknownDependencies,
  verdictKey,
} from './project-inspect-view';

const translator: Translator = ((key: string) => key) as Translator;

function inspection(overrides: Partial<ProjectInspection> = {}): ProjectInspection {
  return {
    root: 'proj',
    entryCandidates: [
      { path: 'main.tex', score: 100, reasonKey: 'project.entryDeclaresClass' },
      { path: 'intro.tex', score: 10, reasonKey: 'project.entryPlainTex' },
    ],
    entry: 'main.tex',
    files: ['main.tex', 'intro.tex'],
    dependencies: [
      { kind: 'usepackage', name: 'graphicx', known: true },
      { kind: 'usepackage', name: 'mypackage', known: false },
    ],
    findings: [],
    inspectable: true,
    ...overrides,
  };
}

describe('finding mapping', () => {
  it('maps each finding kind to its message key', () => {
    expect(findingKey({ kind: 'noEntry' })).toBe('project.finding.noEntry');
    expect(findingKey({ kind: 'entryAmbiguous', candidates: ['a'] })).toBe(
      'project.finding.entryAmbiguous',
    );
    expect(
      findingKey({ kind: 'unsafeReference', reference: 'x', from: 'main.tex' }),
    ).toBe('project.finding.unsafeReference');
    expect(findingKey({ kind: 'shellEscape', from: 'main.tex' })).toBe(
      'project.finding.shellEscape',
    );
    expect(findingKey({ kind: 'configNotExecuted', path: '.latexmkrc' })).toBe(
      'project.finding.configNotExecuted',
    );
  });

  it('carries the parameters each message needs', () => {
    expect(
      findingParams({ kind: 'unsafeReference', reference: '../x', from: 'main.tex' }),
    ).toEqual({ reference: '../x', from: 'main.tex' });
    expect(findingParams({ kind: 'entryAmbiguous', candidates: ['a.tex', 'b.tex'] })).toEqual({
      count: 2,
      candidates: 'a.tex, b.tex',
    });
    expect(findingParams({ kind: 'noEntry' })).toEqual({});
    expect(findingParams({ kind: 'movedFromRecord', recorded: 'a', found: 'b' })).toEqual({
      recorded: 'a',
      found: 'b',
    });
  });

  it('resolves a finding to a sentence', () => {
    expect(findingText({ kind: 'shellEscape', from: 'main.tex' }, translator)).toBe(
      'project.finding.shellEscape',
    );
  });
});

describe('blocking classification', () => {
  it('treats a shell escape, unsafe reference and ambiguity as blocking', () => {
    expect(isBlocking({ kind: 'shellEscape', from: 'main.tex' })).toBe(true);
    expect(isBlocking({ kind: 'unsafeReference', reference: 'x', from: 'y' })).toBe(true);
    expect(isBlocking({ kind: 'entryAmbiguous', candidates: [] })).toBe(true);
    expect(isBlocking({ kind: 'noEntry' })).toBe(true);
    expect(isBlocking({ kind: 'unsafeRoot', path: '../x' })).toBe(true);
  });

  it('keeps an informational finding and a missing reference out of the blocking list', () => {
    // A latexmkrc is reported, not fatal; a missing reference may be generated later.
    expect(isBlocking({ kind: 'configNotExecuted', path: '.latexmkrc' })).toBe(false);
    expect(isBlocking({ kind: 'missingReference', reference: 'gone', from: 'main.tex' })).toBe(
      false,
    );
  });

  it('partitions findings so blocking ones read first', () => {
    const findings = [
      { kind: 'configNotExecuted' as const, path: '.latexmkrc' },
      { kind: 'shellEscape' as const, from: 'main.tex' },
      { kind: 'missingReference' as const, reference: 'gone', from: 'main.tex' },
    ];
    const split = partitionFindings(findings);
    expect(split.blocking).toHaveLength(1);
    expect(split.warnings).toHaveLength(2);
  });
});

describe('verdict and summary', () => {
  it('reports unknown before an inspection ran', () => {
    expect(verdictKey(null)).toBe('project.verdict.unknown');
  });

  it('reports ready only when the inspection is not blocked', () => {
    expect(verdictKey(inspection())).toBe('project.verdict.ready');
    expect(verdictKey(inspection({ inspectable: false }))).toBe('project.verdict.blocked');
  });

  it('lists unknown dependencies and counts files', () => {
    expect(unknownDependencies(inspection())).toEqual(['mypackage']);
    expect(fileCount(inspection())).toBe(2);
  });

  it('ranks candidates by score then path', () => {
    const ranked = rankedCandidates(inspection());
    expect(ranked[0]?.path).toBe('main.tex');
    expect(ranked[1]?.path).toBe('intro.tex');
  });

  it('detects an ambiguous entry set', () => {
    expect(hasAmbiguity(inspection())).toBe(false);
    expect(
      hasAmbiguity(
        inspection({ findings: [{ kind: 'entryAmbiguous', candidates: ['a', 'b'] }] }),
      ),
    ).toBe(true);
  });
});

describe('file listing parsing', () => {
  it('reads a bare path as an existing file without text', () => {
    expect(parseFileListing('main.tex\nintro.tex\n')).toEqual([
      { path: 'main.tex', text: null },
      { path: 'intro.tex', text: null },
    ]);
  });

  it('reads an optional text payload so references can be resolved', () => {
    const parsed = parseFileListing('main.tex = \\documentclass{article}\n');
    expect(parsed).toEqual([
      { path: 'main.tex', text: '\\documentclass{article}' },
    ]);
  });

  it('ignores blank lines and comments', () => {
    expect(parseFileListing('\n\n# a note\nmain.tex\n')).toEqual([
      { path: 'main.tex', text: null },
    ]);
  });

  it('returns nothing for empty input', () => {
    expect(parseFileListing('')).toEqual([]);
  });
});
