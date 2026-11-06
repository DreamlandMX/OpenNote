import type { ProjectFinding, ProjectInspection } from '../editor';
import type { Translator } from '../presentation-i18n';

/**
 * S76 project inspection view logic.
 *
 * The inspection itself runs in M13 from a file list the caller supplies. This
 * module owns what the reader is told, and one rule matters most: a finding is
 * never softened. A shell escape, an unsafe reference or an ambiguous entry set
 * each means "do not build", so the panel says that instead of showing a green
 * summary with a footnote.
 */

export type FindingKey =
  | 'project.finding.entryAmbiguous'
  | 'project.finding.noEntry'
  | 'project.finding.unsafeReference'
  | 'project.finding.missingReference'
  | 'project.finding.unknownDependency'
  | 'project.finding.shellEscape'
  | 'project.finding.configNotExecuted'
  | 'project.finding.unsafeRoot'
  | 'project.finding.movedFromRecord';

export function findingKey(finding: ProjectFinding): FindingKey {
  switch (finding.kind) {
    case 'entryAmbiguous':
      return 'project.finding.entryAmbiguous';
    case 'noEntry':
      return 'project.finding.noEntry';
    case 'unsafeReference':
      return 'project.finding.unsafeReference';
    case 'missingReference':
      return 'project.finding.missingReference';
    case 'unknownDependency':
      return 'project.finding.unknownDependency';
    case 'shellEscape':
      return 'project.finding.shellEscape';
    case 'configNotExecuted':
      return 'project.finding.configNotExecuted';
    case 'unsafeRoot':
      return 'project.finding.unsafeRoot';
    case 'movedFromRecord':
    default:
      return 'project.finding.movedFromRecord';
  }
}

export function findingParams(finding: ProjectFinding): Record<string, string | number> {
  switch (finding.kind) {
    case 'entryAmbiguous':
      return { count: finding.candidates.length, candidates: finding.candidates.join(', ') };
    case 'unsafeReference':
    case 'missingReference':
      return { reference: finding.reference, from: finding.from };
    case 'unknownDependency':
      return { name: finding.name };
    case 'shellEscape':
      return { from: finding.from };
    case 'configNotExecuted':
      return { path: finding.path };
    case 'unsafeRoot':
      return { path: finding.path };
    case 'movedFromRecord':
      return { recorded: finding.recorded, found: finding.found };
    case 'noEntry':
    default:
      return {};
  }
}

/** Resolves a finding to a sentence. */
export function findingText(finding: ProjectFinding, translator: Translator): string {
  return translator(findingKey(finding), findingParams(finding));
}

/**
 * Whether a finding blocks a build.
 *
 * `configNotExecuted` and `missingReference` are reported but not blocking here:
 * the first is informational, and a missing reference is a fact about the project
 * that the user may know is generated later. Everything else stops a build.
 */
export function isBlocking(finding: ProjectFinding): boolean {
  return !(finding.kind === 'configNotExecuted' || finding.kind === 'missingReference');
}

/** Findings split so blocking ones are read first. */
export function partitionFindings(findings: readonly ProjectFinding[]): {
  blocking: ProjectFinding[];
  warnings: ProjectFinding[];
} {
  return {
    blocking: findings.filter(isBlocking),
    warnings: findings.filter((finding) => !isBlocking(finding)),
  };
}

/** A one-line verdict for the panel header. */
export function verdictKey(inspection: ProjectInspection | null):
  | 'project.verdict.ready'
  | 'project.verdict.blocked'
  | 'project.verdict.unknown' {
  if (inspection === null) {
    return 'project.verdict.unknown';
  }
  return inspection.inspectable ? 'project.verdict.ready' : 'project.verdict.blocked';
}

/** Dependencies this build does not recognise. */
export function unknownDependencies(inspection: ProjectInspection): string[] {
  return inspection.dependencies
    .filter((dependency) => !dependency.known)
    .map((dependency) => dependency.name);
}

/**
 * Parses a pasted file listing.
 *
 * Each line is `path`, and an optional ` = text` suffix carries the file's content
 * so references can be resolved. A bare path is still useful: it proves the file
 * exists, which is what "missing reference" needs.
 */
export function parseFileListing(input: string): { path: string; text: string | null }[] {
  const entries: { path: string; text: string | null }[] = [];
  for (const raw of input.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === '' || line.startsWith('#')) {
      continue;
    }
    const separator = line.indexOf(' = ');
    if (separator === -1) {
      entries.push({ path: line, text: null });
      continue;
    }
    const path = line.slice(0, separator).trim();
    const text = line.slice(separator + 3);
    if (path !== '') {
      entries.push({ path, text });
    }
  }
  return entries;
}

/** The files the parser found, for a count in the header. */
export function fileCount(inspection: ProjectInspection): number {
  return inspection.files.length;
}

/** Entry candidates ordered as the picker shows them (highest score first). */
export function rankedCandidates(inspection: ProjectInspection): {
  path: string;
  score: number;
  reasonKey: string;
}[] {
  return [...inspection.entryCandidates].sort((left, right) => {
    if (right.score !== left.score) {
      return right.score - left.score;
    }
    return left.path.localeCompare(right.path);
  });
}

/** True when more than one candidate could plausibly be the entry. */
export function hasAmbiguity(inspection: ProjectInspection): boolean {
  return inspection.findings.some((finding) => finding.kind === 'entryAmbiguous');
}
