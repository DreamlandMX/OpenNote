import MarkdownIt from 'markdown-it';
import type { Token, RendererRule } from 'markdown-it';
import type { StateInline, StateBlock } from 'markdown-it';
import { scanEmbeds } from './embed-render';
import { prepareFormulas, type FormulaView, type MathSpan } from './math';
import { formulaKey, type FormulaResult, type FormulaNotice, type FormulaInput } from './math-render';

export interface SafeMarkdownCaptions {
  frontmatterLabel: string;
  blockedImageRemote: string;
  blockedImageLocal: (source: string) => string;
  /** Accessible label of the jump-to-source button on an embedded block. */
  openEmbeddedSource: (path: string) => string;
  mathHint?: (key: FormulaNotice, position?: number) => string;
  /** Local images become slots the caller fills; without it they stay as captions. */
  localImageSlots?: boolean;
}

interface RenderEnv {
  captions?: SafeMarkdownCaptions;
  formulas?: readonly FormulaResult[];
}

/** Formula tokens participate in inline parsing, preserving surrounding markup. */
function mathPlugin(instance: InstanceType<typeof MarkdownIt>): void {
  const scans = new WeakMap<StateInline, FormulaView[]>();
  instance.inline.ruler.before('escape', 'opennote_math', (state: StateInline, silent: boolean) => {
    if (state.src[state.pos] !== '$') return false;
    let views = scans.get(state);
    if (!views) { views = prepareFormulas(state.src); scans.set(state, views); }
    const view = views.find(item => item.span.start === state.pos && item.span.delimiter === 'inline');
    if (!view) return false;
    if (!silent) {
      const token = state.push('opennote_math_inline', '', 0);
      token.content = view.span.tex;
      token.meta = { view, raw: state.src.slice(view.span.start, view.span.end) };
    }
    state.pos = view.span.end;
    return true;
  });
  instance.block.ruler.before('fence', 'opennote_math_block', (state: StateBlock, start: number, end: number, silent: boolean) => {
    if (state.sCount[start]! - state.blkIndent >= 4) return false;
    const offset = state.bMarks[start]! + state.tShift[start]!;
    if (!state.src.startsWith('$$', offset)) return false;
    const source = state.getLines(start, end, state.blkIndent, false).trimStart();
    const view = prepareFormulas(source)[0];
    if (!view || view.span.start !== 0 || view.span.delimiter !== 'block') return false;
    if (silent) return true;
    const lines = source.slice(0, view.span.end).split('\n').length;
    const token = state.push('opennote_math_block', 'div', 0);
    token.block = true;
    token.content = view.span.tex;
    token.meta = { view, raw: source.slice(0, view.span.end) };
    token.map = [start, start + lines];
    state.line = start + lines;
    return true;
  }, { alt: ['paragraph', 'reference', 'blockquote', 'list'] });
  const render: RendererRule = (tokens, index, _options, env): string => {
    const settings = (env ?? {}) as RenderEnv;
    const meta = tokens[index]!.meta as { view: FormulaView; raw: string };
    const { view, raw } = meta;
    const result = settings.formulas?.find(item => formulaKey(item) === formulaKey(view.span));
    const hint = result?.error ?? view.hint;
    const label = hint ? settings.captions?.mathHint?.(hint, result?.position) ?? hint : null;
    const classes = `md-math md-math-${view.span.delimiter}${result?.html ? '' : ' md-math-source'}`;
    const tag = view.span.delimiter === 'block' ? 'div' : 'span';
    return `<${tag} class="${classes}" data-tex="${escapeHtml(view.span.tex)}">` +
      (result?.html ?? escapeHtml(raw)) +
      (label ? `<span class="md-math-hint" dir="auto">${escapeHtml(label)}</span>` : '') + `</${tag}>`;
  };
  instance.renderer.rules.opennote_math_inline = render;
  instance.renderer.rules.opennote_math_block = render;
}

const EQUALS = 0x3d;

/**
 * `==text==` renders as `<mark>`. The delimiters pair up like GFM
 * strikethrough, so a highlight can hold or sit inside emphasis.
 */
function highlightPlugin(instance: InstanceType<typeof MarkdownIt>): void {
  instance.inline.ruler.before('emphasis', 'opennote_highlight', (state: StateInline, silent: boolean) => {
    if (silent || state.src.charCodeAt(state.pos) !== EQUALS) return false;
    const scanned = state.scanDelims(state.pos, true);
    let length = scanned.length;
    if (length < 2) return false;
    if (length % 2) {
      state.push('text', '', 0).content = '=';
      length -= 1;
    }
    for (let index = 0; index < length; index += 2) {
      state.push('text', '', 0).content = '==';
      state.delimiters.push({
        marker: EQUALS, length: 0, token: state.tokens.length - 1, end: -1,
        open: scanned.can_open, close: scanned.can_close,
      });
    }
    state.pos += scanned.length;
    return true;
  });
  const pair = (state: StateInline, delimiters: StateInline['delimiters']): void => {
    for (const start of delimiters) {
      if (start.marker !== EQUALS || start.end === -1) continue;
      const open = state.tokens[start.token]!;
      const close = state.tokens[delimiters[start.end]!.token]!;
      Object.assign(open, { type: 'mark_open', tag: 'mark', nesting: 1, markup: '==', content: '' });
      Object.assign(close, { type: 'mark_close', tag: 'mark', nesting: -1, markup: '==', content: '' });
    }
  };
  instance.inline.ruler2.before('emphasis', 'opennote_highlight', (state: StateInline) => {
    pair(state, state.delimiters);
    for (const meta of state.tokens_meta) if (meta?.delimiters) pair(state, meta.delimiters);
    return false;
  });
}

/**
 * Obsidian `%%comments%%` are left out of the rendered note. Inline, a comment
 * ends at the next `%%` in the same paragraph. A line that opens with `%%` and
 * does not close on it starts a block running to the next line holding `%%`,
 * or to the end of the note; like the Live parser in `markdown-language.ts`, it
 * does not interrupt a paragraph.
 */
function commentPlugin(instance: InstanceType<typeof MarkdownIt>): void {
  instance.inline.ruler.before('escape', 'opennote_comment', (state: StateInline) => {
    if (!state.src.startsWith('%%', state.pos)) return false;
    const close = state.src.indexOf('%%', state.pos + 2);
    if (close < 0 || close + 2 > state.posMax) return false;
    state.pos = close + 2;
    return true;
  });
  instance.block.ruler.before('fence', 'opennote_comment_block', (state: StateBlock, start: number, end: number, silent: boolean) => {
    if (state.sCount[start]! - state.blkIndent >= 4) return false;
    const first = state.bMarks[start]! + state.tShift[start]!;
    if (!state.src.startsWith('%%', first)) return false;
    const firstLine = state.src.slice(first + 2, state.eMarks[start]);
    const sameLine = firstLine.indexOf('%%');
    if (sameLine >= 0 && firstLine.slice(sameLine + 2).trim() !== '') return false;
    if (silent) return true;
    let line = start + 1;
    if (sameLine < 0) {
      while (line < end && !state.src.slice(state.bMarks[line]!, state.eMarks[line]).includes('%%')) line += 1;
      line = Math.min(line + 1, end);
    }
    state.line = line;
    return true;
  }, { alt: ['reference', 'blockquote', 'list'] });
}

const md = new MarkdownIt({
  html: false,
  linkify: false,
  typographer: false,
  breaks: false,
}).use(mathPlugin).use(highlightPlugin).use(commentPlugin);

const FALLBACK_CAPTIONS: SafeMarkdownCaptions = {
  frontmatterLabel: 'Frontmatter',
  blockedImageRemote: 'Remote image blocked',
  blockedImageLocal: (source: string) => `Image ${source} blocked`,
  openEmbeddedSource: (path: string) => `Open source: ${path}`,
};

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

const BOM = '\uFEFF';

// The closing marker must sit unindented at the start of its line (YAML only
// allows a block-scalar line at column 0 to end the document); an indented
// `---`/`...` inside a literal scalar stays part of the frontmatter text.
const FRONTMATTER_PATTERN = /^---\r?\n[\s\S]*?\r?\n(?:---|\.\.\.)(?:\r?\n|$)/;

export function splitFrontMatter(source: string): { frontMatter: string | null; body: string } {
  const withoutBom = source.startsWith(BOM) ? source.slice(BOM.length) : source;
  const match = withoutBom.match(FRONTMATTER_PATTERN);
  if (!match) {
    return { frontMatter: null, body: withoutBom };
  }
  return { frontMatter: match[0].replace(/\r?\n$/, ''), body: withoutBom.slice(match[0].length) };
}

function isRemoteImageSource(src: string): boolean {
  return /^([a-z][a-z0-9+.-]*:)?\/\//i.test(src) || /^(https?:)/i.test(src);
}

md.renderer.rules.image = (tokens, idx, _options, env) => {
  const token: Token | undefined = tokens[idx];
  const rawSrc = token?.attrGet('src') ?? '';
  const src = typeof rawSrc === 'string' ? rawSrc : String(rawSrc);
  const altText = (token?.content || '').trim();
  const captions = (env as RenderEnv | undefined)?.captions ?? FALLBACK_CAPTIONS;
  const caption = isRemoteImageSource(src)
    ? captions.blockedImageRemote
    : captions.blockedImageLocal(src);
  const detail = altText.length > 0 ? ` ${escapeHtml(altText)}` : '';
  if (captions.localImageSlots === true && !isRemoteImageSource(src) && !/^[a-z][a-z0-9+.-]*:/i.test(src)) {
    // Empty until the caller has the bytes, so a loading image does not flash an error.
    return `<span class="md-image-slot" data-attachment-src="${escapeHtml(src)}" data-image-alt="${escapeHtml(altText)}" data-image-failed="${escapeHtml(caption)}"></span>`;
  }
  return `<span class="md-image-blocked" role="img" aria-label="${escapeHtml(caption)}${detail}">[${escapeHtml(caption)}${detail}]</span>`;
};

export function renderSafeMarkdown(captions: SafeMarkdownCaptions, source: string, formulas: readonly FormulaResult[] = []): string {
  const { frontMatter, body } = splitFrontMatter(source);
  const parts: string[] = [];
  if (frontMatter !== null) {
    parts.push(frontMatterHtml(captions, frontMatter));
  }
  parts.push(md.render(body, { captions, formulas }));
  return parts.join('\n');
}

/** One resolved embed, as the read overlay needs it. */
export interface RenderedEmbed {
  /** The `![[…]]` text this replaces; matched back to its span by exact text. */
  raw: string;
  /** Expanded body when it resolved, otherwise an empty string. */
  content: string | null;
  /** Already-localized placeholder text when it did not resolve. */
  placeholder: string | null;
  /** Note path for a jump affordance, when known. */
  targetPath: string | null;
  /** Target identity for the jump button; `null` renders a plain label. */
  targetId: string | null;
  /** Embeds found inside `content`, resolved for this exact chain. */
  children: readonly RenderedEmbed[];
}

/**
 * Rendered embed nesting levels. The backend refuses a fourth level, so this is
 * a belt-and-braces stop: a host that wrongly returned content again still can
 * not make the renderer recurse past it.
 */
export const EMBED_RENDER_DEPTH_LIMIT = 3;

function frontMatterHtml(captions: SafeMarkdownCaptions, frontMatter: string): string {
  const label = `<p class="md-frontmatter-label">${escapeHtml(captions.frontmatterLabel)}</p>`;
  const inner = frontMatter
    .replace(/^---\r?\n/, '')
    .replace(/\r?\n(?:---|\.\.\.)(?:\r?\n)?$/, '')
    .split(/\r?\n/)
    .filter(line => !/^opennote-id: [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\s*$/i.test(line))
    .join('\n');
  if (inner.trim() === '') return '';
  return `<div class="md-frontmatter">${label}<pre><code>${escapeHtml(inner)}</code></pre></div>`;
}

/**
 * The source row above embedded content.
 *
 * F11 asks for a visible source and an entry point to open it, without editing
 * the copy in place. When the target identity is known the row is a real
 * button, so it is reachable with Tab and activated with Enter; a broken link
 * has no identity to open and keeps a plain, non-actionable label.
 */
function embedSourceRow(
  captions: SafeMarkdownCaptions,
  embed: RenderedEmbed,
  span: { target: string },
): string {
  const path = embed.targetPath ?? span.target;
  const label = escapeHtml(captions.openEmbeddedSource(path));
  if (embed.targetId === null) {
    return `<p class="md-embed-source">${label}</p>`;
  }
  return (
    '<p class="md-embed-source">' +
    `<button type="button" class="md-embed-open" data-embed-open="${escapeHtml(embed.targetId)}">` +
    `${label}</button></p>`
  );
}

/**
 * Renders one embed and, recursively, the embeds inside its content.
 *
 * `depth` is the level this embed occupies: 1 for an embed of the open note.
 * At the limit the content is rendered without further expansion, which can
 * only happen if the host answered `ok` past the chain limit; a real
 * depth-exceeded answer already arrives as a placeholder with no content.
 */
function renderEmbedBlock(
  captions: SafeMarkdownCaptions,
  span: { target: string },
  embed: RenderedEmbed,
  depth: number,
  formulas: readonly FormulaResult[],
): string {
  const path = escapeHtml(embed.targetPath ?? '');
  const written = `<code>${escapeHtml(embed.raw)}</code>`;
  if (embed.content === null) {
    const label = embed.placeholder ?? embed.raw;
    return (
      `<div class="md-embed-placeholder" data-embed-path="${path}">` +
      embedSourceRow(captions, embed, span) +
      `<p class="md-embed-message">${escapeHtml(label)} ${written}</p>` +
      '</div>'
    );
  }
  const { frontMatter, body } = splitFrontMatter(embed.content);
  const prefix = frontMatter === null ? '' : frontMatterHtml(captions, frontMatter);
  const inner =
    depth >= EMBED_RENDER_DEPTH_LIMIT
      ? md.render(body, { captions, formulas })
      : renderEmbedBody(captions, body, embed.children, depth, formulas);
  return (
    `<section class="md-embed" data-embed-path="${path}">` +
    embedSourceRow(captions, embed, span) +
    prefix +
    inner +
    '</section>'
  );
}

/**
 * Renders a body that may contain embeds.
 *
 * `embeds` are the children of the enclosing embed (or the resolved top-level
 * embeds of the note for the outermost call); a span without a matching entry
 * is left as written, because expanding it was refused or not attempted.
 * `depth` is the level of the enclosing content: 0 for the open note.
 */
function renderEmbedBody(
  captions: SafeMarkdownCaptions,
  body: string,
  embeds: readonly RenderedEmbed[],
  depth: number,
  formulas: readonly FormulaResult[],
): string {
  const byRaw = new Map<string, RenderedEmbed>();
  for (const embed of embeds) {
    byRaw.set(embed.raw, embed);
  }
  const html: string[] = [];
  let cursor = 0;
  for (const span of scanEmbeds(body)) {
    if (span.from > cursor) {
      html.push(md.render(body.slice(cursor, span.from), { captions, formulas }));
    }
    const embed = byRaw.get(span.raw);
    if (embed === undefined) {
      html.push(md.render(span.raw, { captions, formulas }));
    } else {
      html.push(renderEmbedBlock(captions, span, embed, depth + 1, formulas));
    }
    cursor = span.to;
  }
  if (cursor < body.length) {
    html.push(md.render(body.slice(cursor), { captions, formulas }));
  }
  return html.join('\n');
}

/**
 * Renders a note body with inline embeds inlined.
 *
 * Embeds are replaced by a block before markdown rendering, so an embedded body
 * is shown as a nested section rather than as raw `![[…]]` syntax. A failed
 * embed renders its placeholder text, which keeps a cycle or a deleted anchor
 * readable instead of showing an empty hole (F11-R02, F11-A03).
 *
 * The replacement is done on the *source* rather than on rendered HTML, so the
 * embedded content goes through the same safe renderer (no HTML, no remote
 * images) instead of being trusted.
 */
export function renderWithEmbeds(
  captions: SafeMarkdownCaptions,
  source: string,
  embeds: readonly RenderedEmbed[],
  formulas: readonly FormulaResult[] = [],
): string {
  const { frontMatter, body } = splitFrontMatter(source);
  const parts: string[] = [];
  if (frontMatter !== null) {
    parts.push(frontMatterHtml(captions, frontMatter));
  }
  parts.push(renderEmbedBody(captions, body, embeds, 0, formulas));
  return parts.join('\n');
}

/** Collect only tokens the Markdown parser displays, excluding code and metadata. */
export function markdownFormulas(source: string): FormulaInput[] {
  const inputs = new Map<string, FormulaInput>();
  function collect(tokens: Token[]): void {
    for (const token of tokens) {
      if (token.type === 'opennote_math_inline' || token.type === 'opennote_math_block') {
        const input: FormulaInput = { tex: token.content, delimiter: token.type.endsWith('_block') ? 'block' : 'inline' };
        inputs.set(formulaKey(input), input);
      }
      if (token.children) collect(token.children);
    }
  }
  collect(md.parse(splitFrontMatter(source).body, {}));
  return [...inputs.values()];
}

/** Map parsed tokens back to source, including quote/list prefixes and code exclusions. */
export function markdownFormulaSpans(source: string): MathSpan[] {
  const body = splitFrontMatter(source).body;
  const bodyStart = source.length - body.length;
  const lines = body.split('\n');
  const starts: number[] = [];
  let offset = bodyStart;
  for (const line of lines) { starts.push(offset); offset += line.length + 1; }
  const spans: MathSpan[] = [];
  for (const token of md.parse(body, {})) {
    if (!token.map) continue;
    if (token.type === 'opennote_math_block') {
      const [first, after] = token.map;
      const start = starts[first]! + lines[first]!.indexOf('$$');
      const end = starts[after - 1]! + lines[after - 1]!.lastIndexOf('$$') + 2;
      spans.push({ tex: token.content, delimiter: 'block', start, end });
    }
    if (token.type !== 'inline' || !token.children) continue;
    const contentLines = token.content.split('\n');
    for (const child of token.children) {
      if (child.type !== 'opennote_math_inline') continue;
      const { view, raw } = child.meta as { view: FormulaView; raw: string };
      const before = token.content.slice(0, view.span.start);
      const line = before.split('\n').length - 1;
      const originalLine = token.map[0] + line;
      const prefix = lines[originalLine]?.indexOf(contentLines[line]!) ?? -1;
      if (prefix < 0) continue;
      const start = starts[originalLine]! + prefix + view.span.start - (before.lastIndexOf('\n') + 1);
      spans.push({ tex: child.content, delimiter: 'inline', start, end: start + raw.length });
    }
  }
  return spans;
}
