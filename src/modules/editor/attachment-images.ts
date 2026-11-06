//! F19: where an attachment reference points, how it is written into a note, and
//! how its bytes reach an `<img>`.
//!
//! The webview only ever holds Blob URLs of bytes the host read through its path
//! rules. Remote images stay blocked: nothing here reaches the network.

import type { AttachmentImportItem, AttachmentInfo, NoteService } from './ports';

export const INLINE_ATTACHMENT_FOLDER = 'assets';
export const MAX_INLINE_ATTACHMENT_BYTES = 100 * 1024 * 1024;

const IMAGE_NAME = /\.(png|jpe?g|gif|webp|bmp|avif|svg)$/i;

export function isImageFileName(name: string): boolean {
  return IMAGE_NAME.test(name);
}

function isRemoteOrAbsolute(target: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith('//') || target.startsWith('/') || target.startsWith('\\');
}

function decodeTarget(target: string): string {
  try {
    return decodeURIComponent(target);
  } catch {
    return target;
  }
}

/** Joins and normalizes vault-relative parts; null when it leaves the vault. */
function normalize(parts: readonly string[]): string | null {
  const out: string[] = [];
  for (const part of parts) {
    if (part === '' || part === '.') continue;
    if (part === '..') {
      if (out.length === 0) return null;
      out.pop();
    } else {
      out.push(part);
    }
  }
  return out.length === 0 ? null : out.join('/');
}

function folderOf(notePath: string): string[] {
  const parts = notePath.split('/');
  parts.pop();
  return parts;
}

/**
 * Vault paths a reference may mean, most likely first: beside the note, then from
 * the vault root (older notes were written that way), then, for a bare
 * `![[name.png]]`, in the default attachment folder.
 */
export function attachmentCandidates(notePath: string, rawTarget: string): string[] {
  const unwrapped = rawTarget.trim().replace(/^<|>$/g, '').replace(/\\/g, '/');
  if (unwrapped === '' || isRemoteOrAbsolute(unwrapped)) return [];
  const target = decodeTarget(unwrapped.split(/[?#]/)[0] ?? '');
  const segments = target.split('/');
  // A note moved to another folder keeps its `../` links; its attachments did not move.
  const fromRoot = segments.filter((segment) => segment !== '..');
  const candidates = [
    normalize([...folderOf(notePath), ...segments]),
    normalize(fromRoot),
    segments.length === 1 ? normalize([INLINE_ATTACHMENT_FOLDER, ...segments]) : null,
  ];
  return [...new Set(candidates.filter((path): path is string => path !== null))];
}

/** `destination` as seen from the note's folder. */
export function relativeToNote(notePath: string, destination: string): string {
  const from = folderOf(notePath);
  const to = destination.split('/');
  let shared = 0;
  while (shared < from.length && shared < to.length - 1 && from[shared] === to[shared]) shared += 1;
  return [...from.slice(shared).map(() => '..'), ...to.slice(shared)].join('/');
}

/** Markdown for a stored file. Spaces and brackets need the `<…>` destination form. */
export function attachmentMarkdown(notePath: string, destination: string, image: boolean): string {
  const path = relativeToNote(notePath, destination);
  const written = /[\s()<>]/.test(path) ? `<${path}>` : path;
  if (image) return `![](${written})`;
  const label = (destination.split('/').pop() ?? destination).replace(/[[\]\\]/g, '\\$&');
  return `[${label}](${written})`;
}

/** A `.pdf` link's file and page, or null for anything else. */
export function pdfLinkTarget(href: string): { target: string; page: number | null } | null {
  const [file = '', fragment = ''] = href.trim().replace(/^<|>$/g, '').split('#');
  if (!/\.pdf$/i.test(decodeTarget(file)) || isRemoteOrAbsolute(file)) return null;
  const page = /^page=(\d+)$/.exec(fragment);
  return { target: file, page: page === null ? null : Number(page[1]) };
}

/** A citation: the selected text as a quote (context, not a PDF annotation), then a page link. */
export function pdfCitation(notePath: string, pdfPath: string, page: number, selected: string, label: string): string {
  const path = `${relativeToNote(notePath, pdfPath)}#page=${page}`;
  const written = /[\s()<>]/.test(path) ? `<${path}>` : path;
  const link = `[${label.replace(/[[\]\\]/g, '\\$&')}](${written})`;
  const quote = selected.trim().split(/\r?\n/).map((line) => `> ${line.trim()}`).join('\n');
  return selected.trim() === '' ? link : `${quote}\n\n${link}`;
}

/** The note-relative reference for an item the host reports as being on disk. */
export function itemMarkdown(notePath: string, item: AttachmentImportItem): string {
  return attachmentMarkdown(notePath, item.destination, item.reference.startsWith('!['));
}

/**
 * A name for a pasted file. Clipboard images all arrive as `image.png`, which
 * would make every paste a numbered copy of the same word.
 */
export function pastedFileName(name: string, type: string, now: Date): string {
  if (name !== '' && !/^image\.[a-z0-9]+$/i.test(name)) return name;
  const extension = type.startsWith('image/') ? type.slice(6).replace('jpeg', 'jpg').replace('svg+xml', 'svg') : 'bin';
  const pad = (value: number) => String(value).padStart(2, '0');
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  return `pasted-${stamp}.${extension}`;
}

/** `![[x.png]]` becomes `![](<x.png>)` so both image spellings render the same way. */
export function rewriteImageEmbeds(body: string, spans: readonly { from: number; to: number; target: string }[]): string {
  let out = body;
  for (const span of [...spans].sort((left, right) => right.from - left.from)) {
    if (isImageFileName(span.target)) out = `${out.slice(0, span.from)}![](<${span.target}>)${out.slice(span.to)}`;
  }
  return out;
}

export interface LoadedImage {
  url: string;
}

/** What the editor needs from the workspace to show and store attachments. */
export interface AttachmentHost {
  /** Resolves an image reference written in `notePath`; null when it cannot be shown. */
  loadImage(notePath: string, target: string): Promise<LoadedImage | null>;
  /** Stores one pasted or dropped file; the failure text is already localized. */
  storeFile(notePath: string, file: File): Promise<{ ok: true; markdown: string } | { ok: false; message: string }>;
  /** Shows a message the editor cannot attach to a control. */
  report(message: string): void;
}

/** The first place a reference exists, or null. Only "missing" moves on to the next place. */
export async function findAttachment(
  service: Pick<NoteService, 'attachmentInfo'>,
  notePath: string,
  target: string,
): Promise<AttachmentInfo | null> {
  for (const candidate of attachmentCandidates(notePath, target)) {
    const info = await service.attachmentInfo(candidate);
    if (info.ok) return info.value;
    if (info.error.code !== 'OBJECT_NOT_FOUND') return null;
  }
  return null;
}

/** Keeps decoded image URLs by content hash, dropping the oldest past the limit. */
export function createImageLoader(service: Pick<NoteService, 'attachmentInfo' | 'readAttachment'>, limit = 64) {
  const byHash = new Map<string, LoadedImage>();
  async function load(notePath: string, target: string): Promise<LoadedImage | null> {
    const info = await findAttachment(service, notePath, target);
    if (info === null || info.mediaType === null || !info.mediaType.startsWith('image/')) return null;
    const known = byHash.get(info.sha256);
    if (known !== undefined) {
      byHash.delete(info.sha256);
      byHash.set(info.sha256, known);
      return known;
    }
    const bytes = await service.readAttachment(info);
    if (!bytes.ok) return null;
    const image = { url: URL.createObjectURL(new Blob([bytes.value as BlobPart], { type: info.mediaType })) };
    byHash.set(info.sha256, image);
    if (byHash.size > limit) {
      const oldest = byHash.entries().next().value;
      if (oldest !== undefined) {
        byHash.delete(oldest[0]);
        URL.revokeObjectURL(oldest[1].url);
      }
    }
    return image;
  }
  function dispose(): void {
    for (const image of byHash.values()) URL.revokeObjectURL(image.url);
    byHash.clear();
  }
  return { load, dispose };
}
