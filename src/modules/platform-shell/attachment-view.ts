import { itemMarkdown, type AttachmentImportItem, type AttachmentIssue, type AttachmentPlan, type PlannedAttachment } from '../editor';
import type { Translator } from '../presentation-i18n';

/**
 * S17 attachment import view.
 *
 * The plan comes from M07. This module decides what a reader is told before any
 * bytes are stored, and it protects three distinctions the rules depend on:
 *
 * * an unknown type is **not** an error: it is imported and handed to the system,
 *   so it must not be shown as refused (F19-R02);
 * * a duplicate is **not** a failure either: nothing needs storing, which is a
 *   different fact from "rejected";
 * * an over-limit file is imported locally and only marked unsynced, so the two
 *   kinds of "problem" must read differently (F19-R04).
 */

export type IssueKey =
  | 'attachment.issue.emptyName'
  | 'attachment.issue.unsafePath'
  | 'attachment.issue.tooLarge'
  | 'attachment.issue.empty'
  | 'attachment.issue.duplicate'
  | 'attachment.issue.renamed';

/** Whether an issue stops the import. */
export function isBlocking(issue: AttachmentIssue): boolean {
  switch (issue.kind) {
    case 'emptyName':
    case 'unsafePath':
    case 'tooLarge':
      return true;
    case 'empty':
    case 'duplicateContent':
    case 'renamedForCollision':
    default:
      // An empty file is still a file, a duplicate needs no new bytes, and a
      // collision is resolved by keeping both files under different names.
      return false;
  }
}

export function issueKey(issue: AttachmentIssue): IssueKey {
  switch (issue.kind) {
    case 'emptyName':
      return 'attachment.issue.emptyName';
    case 'unsafePath':
      return 'attachment.issue.unsafePath';
    case 'tooLarge':
      return 'attachment.issue.tooLarge';
    case 'empty':
      return 'attachment.issue.empty';
    case 'renamedForCollision':
      return 'attachment.issue.renamed';
    case 'duplicateContent':
    default:
      return 'attachment.issue.duplicate';
  }
}

export function issueParams(issue: AttachmentIssue): Record<string, string | number> {
  switch (issue.kind) {
    case 'unsafePath':
      return { path: issue.path };
    case 'tooLarge':
      return { size: issue.size, limit: issue.limit };
    case 'empty':
      return { path: issue.path };
    case 'duplicateContent':
      return { existing: issue.existing };
    case 'renamedForCollision':
      return { from: issue.from, to: issue.to };
    case 'emptyName':
    default:
      return {};
  }
}

export function issueText(issue: AttachmentIssue, translator: Translator): string {
  return translator(issueKey(issue), issueParams(issue));
}

/** True when this attachment would be stored. */
export function willStore(attachment: PlannedAttachment): boolean {
  return attachment.storesBytes && !attachment.issues.some(isBlocking);
}

/** Rows grouped so a reader sees what happens to each file. */
export function partitionPlan(plan: AttachmentPlan): {
  storing: PlannedAttachment[];
  duplicates: PlannedAttachment[];
  refused: PlannedAttachment[];
} {
  const storing: PlannedAttachment[] = [];
  const duplicates: PlannedAttachment[] = [];
  const refused: PlannedAttachment[] = [];
  for (const attachment of plan.attachments) {
    if (attachment.issues.some(isBlocking)) {
      refused.push(attachment);
    } else if (!attachment.storesBytes) {
      duplicates.push(attachment);
    } else {
      storing.push(attachment);
    }
  }
  return { storing, duplicates, refused };
}

/** The label a kind carries, so a reader knows how it will be opened. */
export function kindKey(kind: PlannedAttachment['kind']):
  | 'attachment.kind.image'
  | 'attachment.kind.media'
  | 'attachment.kind.document'
  | 'attachment.kind.binary'
  | 'attachment.kind.unknown' {
  switch (kind) {
    case 'image':
      return 'attachment.kind.image';
    case 'media':
      return 'attachment.kind.media';
    case 'document':
      return 'attachment.kind.document';
    case 'binary':
      return 'attachment.kind.binary';
    case 'unknown':
    default:
      return 'attachment.kind.unknown';
  }
}

/** The label a preview carries. */
export function previewKey(preview: PlannedAttachment['preview']):
  | 'attachment.preview.inline'
  | 'attachment.preview.systemOpen'
  | 'attachment.preview.download' {
  switch (preview) {
    case 'inline':
      return 'attachment.preview.inline';
    case 'systemOpen':
      return 'attachment.preview.systemOpen';
    case 'download':
    default:
      return 'attachment.preview.download';
  }
}

/** A size a person can read without counting digits. */
export function formatBytes(bytes: number, translator: Translator): string {
  if (bytes < 1024) {
    return translator('attachment.bytes', { count: bytes });
  }
  if (bytes < 1024 * 1024) {
    return translator('attachment.kib', { value: (bytes / 1024).toFixed(1) });
  }
  if (bytes < 1024 * 1024 * 1024) {
    return translator('attachment.mib', { value: (bytes / (1024 * 1024)).toFixed(1) });
  }
  return translator('attachment.gib', { value: (bytes / (1024 * 1024 * 1024)).toFixed(1) });
}

/** The one-line verdict for the panel header. */
export function planVerdict(plan: AttachmentPlan | null):
  | 'attachment.verdict.empty'
  | 'attachment.verdict.ready'
  | 'attachment.verdict.nothingNew'
  | 'attachment.verdict.blocked' {
  if (plan === null) {
    return 'attachment.verdict.empty';
  }
  const { storing, refused } = partitionPlan(plan);
  if (storing.length === 0 && refused.length === 0) {
    // Everything was already present, which is not a failure.
    return 'attachment.verdict.nothingNew';
  }
  return storing.length > 0 ? 'attachment.verdict.ready' : 'attachment.verdict.blocked';
}

/** The parsing of a pasted file list, since this build has no file picker. */
export interface ParsedFile {
  name: string;
  size: number;
  sha256: string;
}

/**
 * Parses a pasted listing: `name`, `size`, `sha256` per line, separated by `|`.
 *
 * The hash is required because deduplication depends on it; a line without one is
 * rejected rather than guessed, since a wrong hash would skip a real write.
 */
export function parseFileList(input: string): { files: ParsedFile[]; rejected: number } {
  const files: ParsedFile[] = [];
  let rejected = 0;
  for (const raw of input.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === '' || line.startsWith('#')) {
      continue;
    }
    const parts = line.split('|').map((part) => part.trim());
    if (parts.length < 3) {
      rejected += 1;
      continue;
    }
    // `parts` has at least three entries, but indexing still needs narrowing.
    const name = parts[0] ?? '';
    const size = Number(parts[1]);
    const sha256 = parts[2] ?? '';
    if (name === '' || !Number.isFinite(size) || size < 0 || sha256 === '') {
      rejected += 1;
      continue;
    }
    files.push({ name, size, sha256 });
  }
  return { files, rejected };
}

/** Whether a hash looks like a SHA-256 in hex. */
export function looksLikeSha256(value: string): boolean {
  return /^[0-9a-fA-F]{64}$/.test(value);
}

/** True when the item's file exists in the vault and may be referenced. */
export function isOnDisk(item: AttachmentImportItem): boolean {
  return (item.status === 'stored' || item.status === 'alreadyPresent') && item.reference.length > 0;
}

/**
 * The text inserted into a note for an import: one reference per line, only for
 * files that exist on disk, de-duplicated (two picks of the same bytes share a
 * destination). Refused or failed files are never referenced.
 */
export function insertableReferences(items: readonly AttachmentImportItem[], notePath?: string): string {
  const seen = new Set<string>();
  const lines: string[] = [];
  for (const item of items) {
    if (!isOnDisk(item) || seen.has(item.destination)) continue;
    seen.add(item.destination);
    lines.push(notePath === undefined ? item.reference : itemMarkdown(notePath, item));
  }
  return lines.join('\n');
}

export type AttachmentReasonKey =
  | 'attachment.reason.occupied'
  | 'attachment.reason.changed'
  | 'attachment.reason.source'
  | 'attachment.reason.unsafe'
  | 'attachment.reason.tooLarge'
  | 'attachment.reason.name'
  | 'attachment.reason.diskFull'
  | 'attachment.reason.readOnly'
  | 'attachment.reason.other';

/** Maps a host error/issue key to a short user-facing reason. */
export function attachmentReasonKey(errorKey: string | null): AttachmentReasonKey | null {
  if (errorKey === null || errorKey.length === 0) return null;
  switch (errorKey) {
    case 'error.attachmentPathOccupied':
      return 'attachment.reason.occupied';
    case 'error.attachmentSourceChanged':
      return 'attachment.reason.changed';
    case 'error.attachmentSourceInvalid':
    case 'error.exportSourceMissing':
      return 'attachment.reason.source';
    case 'error.attachmentUnsafePath':
    case 'error.attachmentPathReparse':
    case 'error.attachmentIsNote':
    case 'error.pathTraversal':
    case 'error.reservedName':
    case 'error.reservedDir':
    case 'error.pathAds':
    case 'error.pathIllegalChar':
    case 'error.trailingDotOrSpace':
    case 'error.absolutePath':
      return 'attachment.reason.unsafe';
    case 'error.attachmentTooLarge':
      return 'attachment.reason.tooLarge';
    case 'error.attachmentEmptyName':
    case 'error.nameTooLong':
      return 'attachment.reason.name';
    case 'error.diskFull':
      return 'attachment.reason.diskFull';
    case 'error.vaultReadOnly':
    case 'error.schemaReadOnly':
      return 'attachment.reason.readOnly';
    default:
      return 'attachment.reason.other';
  }
}

export function attachmentStatusKey(
  status: AttachmentImportItem['status'],
): 'attachment.result.stored' | 'attachment.result.alreadyPresent' | 'attachment.result.refused' | 'attachment.result.failed' {
  return `attachment.result.${status}`;
}
