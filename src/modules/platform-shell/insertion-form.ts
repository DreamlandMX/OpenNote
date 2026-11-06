import type {
  BibEntry,
  Bibliography,
  InsertParameters,
  InsertionIssue,
  InsertionPlan,
} from '../editor';

/**
 * S73/S74 insertion and citation form logic.
 *
 * The plan itself comes from M13; this module only decides how a form is filled
 * and how its result is explained. The rule it owns: an issue is always shown with
 * the field it belongs to, so a user can fix the form instead of guessing which
 * input was rejected.
 */

export interface FieldIssue {
  field: 'body' | 'path' | 'width' | 'label' | 'rows' | 'advanced' | 'title' | 'level' | 'general';
  key: string;
  params: Record<string, string | number>;
}

/** Maps a plan issue to the form field that caused it. */
export function fieldIssue(issue: InsertionIssue): FieldIssue {
  switch (issue.kind) {
    case 'missing':
      return {
        field:
          issue.field === 'body'
            ? 'body'
            : issue.field === 'path'
              ? 'path'
              : issue.field === 'title'
                ? 'title'
                : 'general',
        key: 'insert.issue.missing',
        params: { field: issue.field },
      };
    case 'invalidLabel':
      return { field: 'label', key: 'insert.issue.invalidLabel', params: { label: issue.label } };
    case 'duplicateLabel':
      return {
        field: 'label',
        key: 'insert.issue.duplicateLabel',
        params: { label: issue.label, count: issue.existing.length },
      };
    case 'unsafeImagePath':
      return { field: 'path', key: 'insert.issue.unsafePath', params: { path: issue.path } };
    case 'invalidWidth':
      return { field: 'width', key: 'insert.issue.invalidWidth', params: { value: issue.value } };
    case 'malformedTable':
      return { field: 'rows', key: 'insert.issue.malformedTable', params: { detail: issue.detail } };
    case 'invalidLevel':
      return { field: 'level', key: 'insert.issue.invalidLevel', params: { level: issue.level } };
    case 'unknownLabel':
      return {
        field: 'label',
        key: 'insert.issue.unknownLabel',
        params: { label: issue.label },
      };
    default:
      return { field: 'general', key: 'insert.issue.unknown', params: {} };
  }
}

/** The issues that belong to one field. */
export function issuesFor(
  plan: InsertionPlan | null,
  field: FieldIssue['field'],
): FieldIssue[] {
  if (plan === null) {
    return [];
  }
  return plan.issues.map(fieldIssue).filter((issue) => issue.field === field);
}

/** True when the plan may be inserted. */
export function canInsert(plan: InsertionPlan | null): boolean {
  return plan !== null && plan.issues.length === 0 && plan.fragment.trim() !== '';
}

/**
 * True when the source no longer matches the text the plan was previewed on.
 *
 * F48 §7 STALE_INSERT_POINT: label uniqueness and package presence are computed
 * from the source, so a plan previewed before an edit must be previewed again
 * rather than inserted with stale checks.
 */
export function isInsertionStale(previewSource: string | null, source: string): boolean {
  return previewSource !== null && previewSource !== source;
}

/**
 * True when the form still holds the values the plan was previewed with.
 *
 * §8 keeps preview and execution separate. Applying a plan after changing a field
 * would insert the previewed values while the form shows different ones, so the
 * plan must be bound to the parameters that produced it.
 */
export function sameInsertParameters(
  previewed: InsertParameters | null,
  current: InsertParameters,
): boolean {
  return previewed !== null && JSON.stringify(previewed) === JSON.stringify(current);
}

/**
 * The dependency lines a user must add, with the exact text.
 *
 * F48-R03 forbids silently editing the preamble. The lines are shown first and
 * only applied after the caller confirms them.
 */
export function missingDependencyLines(plan: InsertionPlan | null): string[] {
  if (plan === null) {
    return [];
  }
  return plan.dependencies.filter((entry) => !entry.present).map((entry) => entry.line);
}

/** A text change over the previewed source. */
export interface InsertionEdit {
  start: number;
  end: number;
  text: string;
}

/**
 * The point where confirmed dependency lines may be added, if one is safe.
 *
 * A package line belongs before `\begin{document}`. The edit is offered only when
 * that command starts its own line, so the lines cannot land inside a command or a
 * comment; otherwise the plan is still shown but the user adds them manually.
 */
export function dependencyInsertion(
  source: string,
  lines: readonly string[],
): InsertionEdit | null {
  const unique = [...new Set(lines.map((line) => line.trim()).filter((line) => line !== ''))];
  if (unique.length === 0) {
    return null;
  }
  const begin = source.indexOf('\\begin{document}');
  if (begin < 0) {
    return null;
  }
  const lineStart = source.lastIndexOf('\n', begin) + 1;
  if (!/^[ \t]*$/.test(source.slice(lineStart, begin))) {
    return null;
  }
  const newline = source.includes('\r\n') ? '\r\n' : '\n';
  return { start: lineStart, end: lineStart, text: `${unique.join(newline)}${newline}` };
}

/**
 * True when a zero-width insertion would split the selection it shares a point
 * with. Two changes cannot both claim the same offset.
 */
export function insertionPointConflicts(point: number, from: number, to: number): boolean {
  if (from === to) {
    return point === from;
  }
  return point >= from && point < to;
}

/**
 * Every edit for one insertion, returned together so the editor applies them in
 * one transaction (F48-R04). Null means the confirmed package lines cannot be
 * placed safely; nothing must be inserted in that case.
 */
export function insertionEdits(
  source: string,
  fragment: string,
  range: { from: number; to: number },
  dependencies: readonly string[],
  includeDependencies: boolean,
): InsertionEdit[] | null {
  const edits: InsertionEdit[] = [{ start: range.from, end: range.to, text: fragment }];
  if (!includeDependencies) {
    return edits;
  }
  const dependencyEdit = dependencyInsertion(source, dependencies);
  if (
    dependencyEdit === null ||
    insertionPointConflicts(dependencyEdit.start, range.from, range.to)
  ) {
    return null;
  }
  edits.push(dependencyEdit);
  return edits;
}

/** Parses the plain-text table input: one row per line, `|` between cells. */
export function parseTableInput(input: string): string[][] {
  return input
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== '')
    .map((line) => line.split('|').map((cell) => cell.trim()));
}

/** A default width in the documented range. */
export const DEFAULT_IMAGE_WIDTH = 0.8;

/** Suggested label prefixes for each kind (§4). */
export function labelPrefix(kind: InsertParameters['kind']): string {
  switch (kind) {
    case 'equation':
      return 'eq:';
    case 'image':
      return 'fig:';
    case 'table':
      return 'tab:';
    default:
      return '';
  }
}

/** A label suggestion that is unlikely to collide with an existing one. */
export function suggestLabel(kind: InsertParameters['kind'], existing: readonly string[]): string {
  const prefix = labelPrefix(kind);
  if (prefix === '') {
    return '';
  }
  for (let index = 1; index < 1000; index += 1) {
    const candidate = `${prefix}${index}`;
    if (!existing.includes(candidate)) {
      return candidate;
    }
  }
  return '';
}

/**
 * A citation entry as the list shows it.
 *
 * F49 §8 requires the author and title to be announced, so the label is built from
 * the entry itself rather than from the key alone.
 */
export interface CitationItem {
  /** Position in `bibliography.entries`, so a row maps back to its exact entry. */
  entryIndex: number;
  citeKey: string;
  /** Parser byte offset, shown so two duplicate rows can be told apart. */
  offset: number;
  label: string;
  /** True when the key is used more than once, so citing it is ambiguous. */
  duplicate: boolean;
  unknownType: boolean;
}

export function citationItems(bibliography: Bibliography): CitationItem[] {
  return bibliography.entries.map((entry: BibEntry, entryIndex: number) => {
    const author = entry.fields.find((field) => field.name === 'author')?.value ?? '';
    const title = entry.fields.find((field) => field.name === 'title')?.value ?? '';
    const label = [author, title].filter((part) => part !== '').join(' — ') || entry.citeKey;
    return {
      entryIndex,
      citeKey: entry.citeKey,
      offset: entry.offset,
      label,
      duplicate: bibliography.duplicates.some((duplicate) => duplicate.citeKey === entry.citeKey),
      unknownType: entry.unknownType,
    };
  });
}

/** Entries that may be cited without ambiguity. */
export function citableItems(bibliography: Bibliography): CitationItem[] {
  return citationItems(bibliography).filter((item) => !item.duplicate);
}

/** Filters the citation list by key, author or title. */
export function filterCitations(
  items: readonly CitationItem[],
  query: string,
): CitationItem[] {
  const trimmed = query.trim().toLowerCase();
  if (trimmed === '') {
    return [...items];
  }
  return items.filter(
    (item) =>
      item.citeKey.toLowerCase().includes(trimmed) ||
      item.label.toLowerCase().includes(trimmed),
  );
}

/** A short summary of what the bibliography parser reported. */
export function bibliographySummary(bibliography: Bibliography): {
  entries: number;
  duplicates: number;
  errors: number;
  unknownTypes: number;
} {
  return {
    entries: bibliography.entries.length,
    duplicates: bibliography.duplicates.length,
    errors: bibliography.errors.length,
    unknownTypes: bibliography.entries.filter((entry) => entry.unknownType).length,
  };
}
