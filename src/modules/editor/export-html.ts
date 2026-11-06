//! A single note as one self-contained HTML document: images and formula fonts are
//! embedded, so it opens offline and on another machine. The body comes from the
//! same sanitizing renderer as the reading view, and a strict policy on the page
//! means nothing in it can run or reach the network.

import type { NoteService } from './ports';
import { findAttachment } from './attachment-images';
import { markdownFormulas, renderSafeMarkdown, type SafeMarkdownCaptions } from './markdown-render';
import { renderFormulas, type FormulaResult } from './math-render';
import { rewriteImageEmbeds } from './attachment-images';
import { scanEmbeds } from './embed-render';

const SLOT = /<span class="md-image-slot" data-attachment-src="([^"]*)" data-image-alt="([^"]*)" data-image-failed="([^"]*)"><\/span>/g;

function unescapeAttribute(value: string): string {
  return value.replace(/&quot;/g, '"').replace(/&gt;/g, '>').replace(/&lt;/g, '<').replace(/&amp;/g, '&');
}

/** Puts each image in, or its caption when the file cannot be shown. */
export async function fillImageSlots(html: string, dataUrl: (target: string) => Promise<string | null>): Promise<string> {
  const slots = [...html.matchAll(SLOT)];
  const urls = await Promise.all(slots.map((slot) => dataUrl(unescapeAttribute(slot[1] ?? ''))));
  let index = 0;
  return html.replace(SLOT, (_whole, _source, alt: string, failed: string) => {
    const url = urls[index++];
    return url === null || url === undefined
      ? `<span class="md-image-blocked">[${failed}]</span>`
      : `<img class="md-image" src="${url}" alt="${alt}">`;
  });
}

function base64(bytes: Uint8Array): string {
  let binary = '';
  const step = 0x8000;
  for (let at = 0; at < bytes.length; at += step) binary += String.fromCharCode(...bytes.subarray(at, at + step));
  return btoa(binary);
}

/** Readable on paper and on screen, independent of the app's theme. */
const NOTE_STYLES = `
:root { color-scheme: light; }
body { margin: 0 auto; max-width: 46rem; padding: 2.5rem 1.5rem; color: #1f2328; background: #fff; font: 16px/1.7 -apple-system, "Segoe UI", "Microsoft YaHei", "PingFang SC", sans-serif; }
h1, h2, h3, h4 { line-height: 1.3; margin: 1.6em 0 .6em; }
img { max-width: 100%; height: auto; }
pre { overflow-x: auto; padding: .8em 1em; border-radius: 6px; background: #f4f5f7; }
code { font-family: Consolas, "Courier New", monospace; font-size: .92em; }
blockquote { margin: 1em 0; padding-inline-start: 1em; border-inline-start: 3px solid #d0d4da; color: #57606a; }
table { border-collapse: collapse; } th, td { padding: .35em .7em; border: 1px solid #d0d4da; }
a { color: #0b6bcb; }
mark { background: #fff1a8; }
.md-image-blocked, .md-frontmatter-label { color: #57606a; font-size: 12px; }
.md-frontmatter { margin-bottom: 1.5em; padding: .6em .9em; border: 1px solid #d0d4da; border-radius: 6px; }
.md-math-block { margin: 1em 0; text-align: center; overflow-x: auto; }
`;

function escapeText(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

export function noteDocument(title: string, body: string, formulaStyles: string, lang: string): string {
  const policy = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; base-uri 'none'; form-action 'none'";
  return `<!doctype html>
<html lang="${escapeText(lang).replace(/"/g, '')}">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${policy}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeText(title)}</title>
<style>${NOTE_STYLES}${formulaStyles}</style>
</head>
<body>
${body}
</body>
</html>
`;
}

export function noteTitle(path: string): string {
  return (path.split('/').pop() ?? path).replace(/\.(md|markdown)$/i, '');
}

/**
 * The formula stylesheet as it is loaded in the app, with its fonts embedded. The
 * bundle already holds them, so nothing is downloaded.
 */
async function formulaStyles(): Promise<string> {
  const rules: string[] = [];
  for (const sheet of Array.from(document.styleSheets)) {
    for (const rule of Array.from(sheet.cssRules)) {
      if (rule instanceof CSSFontFaceRule && rule.cssText.includes('KaTeX')) {
        const font = /url\("?([^")]+\.woff2)"?\)/.exec(rule.cssText)?.[1];
        if (font === undefined) continue;
        const response = await fetch(new URL(font, sheet.href ?? document.baseURI)).catch(() => null);
        if (response === null || !response.ok) continue;
        const data = base64(new Uint8Array(await response.arrayBuffer()));
        rules.push(rule.cssText.replace(/src:[^;]+;/, `src: url("data:font/woff2;base64,${data}") format("woff2");`));
      } else if (rule.cssText.includes('.katex')) {
        rules.push(rule.cssText);
      }
    }
  }
  return rules.join('\n');
}

export interface RenderedNote {
  body: string;
  styles: string;
}

/** Renders a note's current text for export. Note embeds keep their written `![[…]]`. */
export async function renderNoteForExport(
  service: Pick<NoteService, 'attachmentInfo' | 'readAttachment'>,
  notePath: string,
  source: string,
  captions: Omit<SafeMarkdownCaptions, 'localImageSlots'>,
): Promise<RenderedNote> {
  const prepared = rewriteImageEmbeds(source, scanEmbeds(source));
  const inputs = markdownFormulas(prepared);
  let formulas: readonly FormulaResult[] = [];
  if (inputs.length > 0) {
    const batch = await renderFormulas(inputs);
    if (batch.status === 'ready') formulas = batch.formulas;
  }
  const html = renderSafeMarkdown({ ...captions, localImageSlots: true }, prepared, formulas);
  const body = await fillImageSlots(html, async (target) => {
    const info = await findAttachment(service, notePath, target);
    if (info === null || info.mediaType === null || !info.mediaType.startsWith('image/')) return null;
    const bytes = await service.readAttachment(info);
    return bytes.ok ? `data:${info.mediaType};base64,${base64(bytes.value)}` : null;
  });
  return { body, styles: formulas.length > 0 ? await formulaStyles() : '' };
}

/**
 * Prints the rendered note through the system dialog, where "Microsoft Print to
 * PDF" saves a PDF. Only the note is on the page while the dialog is open.
 */
export async function printRenderedNote(title: string, rendered: RenderedNote): Promise<void> {
  const holder = document.createElement('div');
  holder.id = 'print-root';
  holder.innerHTML = rendered.body;
  const style = document.createElement('style');
  style.textContent = `@media screen { #print-root { display: none; } }
@media print { body > :not(#print-root) { display: none !important; } #print-root { display: block; } ${NOTE_STYLES} }`;
  document.body.append(holder, style);
  await Promise.all(Array.from(holder.querySelectorAll('img')).map((image) => image.decode().catch(() => undefined)));
  const previousTitle = document.title;
  document.title = title; // The default file name in the PDF save dialog.
  const finished = new Promise<void>((resolve) => window.addEventListener('afterprint', () => resolve(), { once: true }));
  window.print();
  await finished;
  document.title = previousTitle;
  holder.remove();
  style.remove();
}
