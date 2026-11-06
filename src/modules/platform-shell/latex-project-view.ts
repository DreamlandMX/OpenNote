import type { ProjectInspection } from '../editor';
import type { ServiceError } from '../domain';
import type { LatexEnvironmentStatus } from '../latex-workspace/ports';
import type { MessageKey } from '../presentation-i18n';
import { isBlocking } from './project-inspect-view';

/**
 * The entry a registration may use, or `null`.
 *
 * The user's choice must be one of the inspected `.tex` candidates. An
 * ambiguous entry is resolved by that choice, never guessed; any other
 * blocking finding (a shell escape, a reference leaving the project, an unsafe
 * root) keeps the project from being registered at all.
 */
export function registrableEntry(inspection: ProjectInspection | null, chosen: string | null): string | null {
  if (inspection === null || chosen === null) return null;
  if (!inspection.entryCandidates.some((candidate) => candidate.path === chosen)) return null;
  const blocked = inspection.findings.some(
    (finding) => isBlocking(finding) && finding.kind !== 'entryAmbiguous',
  );
  return blocked ? null : chosen;
}

const PROJECT_ERROR_KEYS: Readonly<Record<string, MessageKey>> = {
  'error.projectAlreadyRegistered': 'project.error.alreadyRegistered',
  'error.latexFolderOutsideVault': 'project.error.outsideVault',
  'error.projectReparse': 'project.error.reparse',
  'error.projectTooManyFiles': 'project.error.tooLarge',
  'error.projectFileTooLarge': 'project.error.tooLarge',
  'error.projectTooLarge': 'project.error.tooLarge',
  'error.latexProjectsUnavailable': 'project.error.unavailable',
};

/** A specific caption for project errors whose generic code would mislead. */
export function projectErrorKey(error: ServiceError): MessageKey | null {
  return PROJECT_ERROR_KEYS[error.messageKey] ?? null;
}

/** What the LaTeX hub says about the local environment; `undefined` while checking. */
export function environmentStepKey(status: LatexEnvironmentStatus | null | undefined): MessageKey {
  if (status === undefined) return 'project.env.checking';
  if (status === null) return 'project.env.unknown';
  if (status.preparing !== null) return 'project.env.preparing';
  return status.environment.kind === 'ready' ? 'project.env.ready' : 'project.env.missing';
}
