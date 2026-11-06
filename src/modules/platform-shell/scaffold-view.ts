import type {
  DocumentLanguage,
  LatexMetadata,
  MetadataError,
  PaperSize,
  ScaffoldIssue,
  ScaffoldPlan,
} from '../editor';

/**
 * S81 LaTeX project wizard presentation.
 *
 * The plan and the escaping live in M13; this module only decides what the wizard
 * shows. The rule it owns: the document language is chosen once and is never
 * replaced by the interface language afterwards (F46-R07).
 */

export interface MetadataIssue {
  field: 'title' | 'author' | 'date' | 'general';
  key:
    | 'wizard.issue.titleEmpty'
    | 'wizard.issue.titleTooLong'
    | 'wizard.issue.authorTooLong'
    | 'wizard.issue.invalidDate';
}

export function metadataIssue(error: MetadataError): MetadataIssue {
  switch (error.kind) {
    case 'titleEmpty':
      return { field: 'title', key: 'wizard.issue.titleEmpty' };
    case 'titleTooLong':
      return { field: 'title', key: 'wizard.issue.titleTooLong' };
    case 'authorTooLong':
      return { field: 'author', key: 'wizard.issue.authorTooLong' };
    case 'invalidDate':
    default:
      return { field: 'date', key: 'wizard.issue.invalidDate' };
  }
}

/** Issue keys for plan-level problems. */
export function issueKey(issue: ScaffoldIssue):
  | 'wizard.issue.collision'
  | 'wizard.issue.unsafeDestination'
  | 'wizard.issue.unknownTemplate'
  | MetadataIssue['key'] {
  switch (issue.kind) {
    case 'collision':
      return 'wizard.issue.collision';
    case 'unsafeDestination':
      return 'wizard.issue.unsafeDestination';
    case 'unknownTemplate':
      return 'wizard.issue.unknownTemplate';
    case 'invalidMetadata':
    default:
      // Say which field is wrong rather than "metadata is invalid".
      return metadataIssue(issue.error).key;
  }
}

export function issueParams(issue: ScaffoldIssue): Record<string, string> {
  switch (issue.kind) {
    case 'collision':
      return { path: issue.path };
    case 'unsafeDestination':
      return { destination: issue.destination };
    case 'unknownTemplate':
      return { templateId: issue.templateId };
    case 'invalidMetadata':
      return {};
    default:
      return {};
  }
}

/**
 * The language the wizard should start with.
 *
 * A stored choice always wins: switching the interface later must not replace a
 * language the user already picked (F46-R07).
 */
export function initialLanguage(
  uiLocale: string,
  stored: DocumentLanguage | null,
): DocumentLanguage {
  if (stored !== null) {
    return stored;
  }
  return uiLocale === 'zh-CN' || uiLocale === 'zh-TW' ? 'zhCn' : 'en';
}

/** True when the plan may be executed: no issues and at least one file. */
export function canCreate(plan: ScaffoldPlan): boolean {
  return plan.issues.length === 0 && plan.files.length > 0;
}

/** Files listed before the rest are collapsed. */
export const PREVIEW_LIMIT = 10;

export function previewFiles(plan: ScaffoldPlan): { shown: string[]; hidden: number } {
  const paths = plan.files.map((file) => file.path);
  if (paths.length <= PREVIEW_LIMIT) {
    return { shown: paths, hidden: 0 };
  }
  return { shown: paths.slice(0, PREVIEW_LIMIT), hidden: paths.length - PREVIEW_LIMIT };
}

/** A short summary line for the confirmation step. */
export function planSummary(plan: ScaffoldPlan): {
  files: number;
  requires: number;
  executable: boolean;
  entry: string;
} {
  return {
    files: plan.files.length,
    requires: plan.requires.length,
    executable: canCreate(plan),
    entry: plan.entryFile,
  };
}

/** A default destination from the title, so the wizard starts somewhere sane. */
export function defaultDestination(title: string): string {
  const cleaned = title
    .trim()
    .replace(/[/\\:*?"<>|]/g, '-')
    .replace(/\s+/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '')
    .toLowerCase();
  return cleaned === '' ? 'latex-project' : cleaned;
}

/** The paper sizes offered, in display order. */
export function paperOptions(): { value: PaperSize; key: 'wizard.paper.a4' | 'wizard.paper.letter' }[] {
  return [
    { value: 'a4', key: 'wizard.paper.a4' },
    { value: 'letter', key: 'wizard.paper.letter' },
  ];
}

/** A metadata object with the wizard's defaults. */
export function defaultMetadata(
  language: DocumentLanguage,
  date: string,
): LatexMetadata {
  return {
    title: '',
    author: '',
    date,
    language,
    // A4 is the documented default; choosing Letter only changes the paper.
    paper: 'a4',
  };
}
