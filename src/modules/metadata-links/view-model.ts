//! Pure view-model for the metadata panels (F08 tags, F10 backlinks/link
//! issues, F11 embeds). Keeping the shaping here means the React component stays
//! a thin renderer and every rule below is testable without a DOM.

import type {
  Backlink,
  BacklinkReport,
  EmbedExpansion,
  LinkIssue,
  LinkReport,
  NoteMetadataReport,
  TagEntry,
  TagReport,
  UnreadableNote,
} from '../editor/ports';

/** A row the panel can render without further logic. */
export interface MetadataRow {
  /** Stable key for React lists. */
  key: string;
  /** Primary text (tag, path or target). */
  label: string;
  /** Secondary text; empty when there is nothing to add. */
  detail: string;
  /** Byte offset for "jump to reference", when the row has one. */
  offset: number | null;
  /** Note id to open, when the row points at a note. */
  noteId: string | null;
}

export interface MetadataSection {
  /** Message key for the section heading. */
  titleKey: string;
  rows: readonly MetadataRow[];
  /** True when the section is intentionally empty (show the empty text). */
  empty: boolean;
}

/**
 * Tags sorted by name, with the note count as detail. Ordering is applied here
 * so the panel never depends on the host's ordering.
 */
export function tagRows(report: TagReport): MetadataRow[] {
  return [...report.tags]
    .sort((left, right) => left.tag.localeCompare(right.tag))
    .map((entry: TagEntry) => ({
      key: `tag:${entry.tag}`,
      label: entry.tag,
      detail: String(entry.notePaths.length),
      offset: null,
      noteId: entry.noteIds[0] ?? null,
    }));
}

/**
 * Backlinks grouped by source note, in path order. Several links from one note
 * collapse into one row so a heavily linked note does not flood the panel; the
 * detail carries how many references there are.
 */
export function backlinkRows(report: BacklinkReport): MetadataRow[] {
  const byNote = new Map<string, Backlink[]>();
  for (const link of report.backlinks) {
    const existing = byNote.get(link.fromNoteId);
    if (existing) {
      existing.push(link);
    } else {
      byNote.set(link.fromNoteId, [link]);
    }
  }
  return [...byNote.entries()]
    .flatMap(([noteId, links]) => {
      const first = links[0];
      if (first === undefined) {
        return [];
      }
      const anchors = links.filter(link => link.anchor !== null).length;
      return [{
        key: `backlink:${noteId}`,
        label: first.fromPath,
        detail: anchors > 0 ? `${links.length} (${anchors})` : String(links.length),
        offset: first.offset,
        noteId,
      }];
    })
    .sort((left, right) => left.label.localeCompare(right.label));
}

/**
 * Link problems, with broken targets before missing anchors so the more
 * serious state is never hidden below a cosmetic one. Ambiguity keeps its
 * candidate paths, which is what F10-A02 requires the user to choose from.
 * A written alias stays visible next to the target so "keep my label" and
 * "the target moved" are distinguishable (F10-A03).
 */
export function linkIssueRows(report: LinkReport): MetadataRow[] {
  const broken = report.broken.map((issue: LinkIssue) => ({
    key: `broken:${issue.fromNoteId}:${issue.offset}`,
    label: issue.target.length > 0 ? issue.target : `#${issue.anchor ?? ''}`,
    detail: issueDetail(issue, issue.fromPath),
    offset: issue.offset,
    noteId: issue.fromNoteId,
  }));
  const missing = report.missingAnchors.map((issue: LinkIssue) => ({
    key: `anchor:${issue.fromNoteId}:${issue.offset}`,
    label: `#${issue.anchor ?? ''}`,
    detail: issueDetail(issue, issue.fromPath),
    offset: issue.offset,
    noteId: issue.fromNoteId,
  }));
  return [...broken, ...missing];
}

/**
 * Alias, candidates and source path share one detail line. The alias is only
 * repeated when it differs from the target; otherwise the row would say the
 * same thing twice.
 */
function issueDetail(issue: LinkIssue, fallback: string): string {
  const parts: string[] = [];
  if (issue.label !== null && issue.label !== issue.target) {
    parts.push(issue.label);
  }
  if (issue.candidates.length > 0) {
    parts.push(issue.candidates.join(', '));
  } else if (fallback.length > 0) {
    parts.push(fallback);
  }
  return parts.join(' · ');
}

/** Notes the index could not read: shown as a warning, never as broken links. */
export function unreadableRows(notes: readonly UnreadableNote[]): MetadataRow[] {
  return notes.map(note => ({
    key: `unreadable:${note.noteId}`,
    label: note.relativePath,
    detail: note.messageKey,
    offset: null,
    noteId: note.noteId,
  }));
}

/**
 * The embed placeholder text for a non-ok expansion. Returning text (not just a
 * flag) is what keeps a cycle or a deleted anchor readable instead of blank
 * (F11-R02, F11-A03). The note path is included so the user can navigate.
 */
export function embedPlaceholder(expansion: EmbedExpansion): string | null {
  const path = expansion.targetPath ?? '';
  switch (expansion.status) {
    case 'ok':
      return null;
    case 'brokenLink':
      return `embed.broken|${path}`;
    case 'ambiguousTarget':
      return `embed.ambiguous|${path}`;
    case 'anchorMissing':
      return `embed.anchorMissing|${path}`;
    case 'cycle':
      return `embed.cycle|${expansion.chain.join(' > ')}`;
    case 'depthExceeded':
      return `embed.depth|${path}`;
    default:
      return `embed.broken|${path}`;
  }
}

/**
 * What the panel can show for the current selection. The vault-wide tabs work
 * with no note open; the per-note tabs need one, so the panel disables them
 * instead of showing a misleading empty state (S18: no note is not "no
 * backlinks").
 */
export function tabAvailability(noteId: string | null): {
  tags: boolean;
  backlinks: boolean;
  issues: boolean;
} {
  return { tags: true, backlinks: noteId !== null, issues: true };
}

/** Sections for the note panel, in a fixed order the UI can rely on. */
export function noteSections(report: NoteMetadataReport): MetadataSection[] {  const headingRows = report.headings.map(heading => ({
    key: `heading:${heading.offset}`,
    label: `${'#'.repeat(heading.level)} ${heading.text}`,
    detail: heading.slug,
    offset: heading.offset,
    noteId: report.noteId,
  }));
  const tagRowsForNote = report.tags.map(occurrence => ({
    key: `tag:${occurrence.offset}`,
    label: occurrence.tag,
    detail: occurrence.source,
    offset: occurrence.offset,
    noteId: report.noteId,
  }));
  const linkRows = report.links.map((parsed, index) => ({
    key: `link:${index}`,
    label: parsed.link.target.length > 0 ? parsed.link.target : `#${parsed.link.anchor ?? ''}`,
    detail: parsed.resolved.status,
    offset: parsed.link.offset,
    noteId: parsed.resolved.targetId,
  }));
  return [
    { titleKey: 'metadata.headings', rows: headingRows, empty: headingRows.length === 0 },
    { titleKey: 'metadata.tags', rows: tagRowsForNote, empty: tagRowsForNote.length === 0 },
    { titleKey: 'metadata.links', rows: linkRows, empty: linkRows.length === 0 },
    {
      titleKey: 'metadata.backlinks',
      rows: backlinkRows({
        noteId: report.noteId,
        backlinks: report.backlinks,
        unreadable: [],
      }),
      empty: report.backlinks.length === 0,
    },
  ];
}
