import { Facet, StateEffect, StateField, type EditorState, type Extension, type Range, type Text } from '@codemirror/state';
import { Decoration, EditorView, WidgetType, type DecorationSet } from '@codemirror/view';
import { syntaxTree } from '@codemirror/language';

type SyntaxNode = ReturnType<ReturnType<typeof syntaxTree>['resolveInner']>;
import { splitFrontMatter, markdownFormulaSpans } from './markdown-render';
import type { MathSpan } from './math';
import { formulaKey, type FormulaResult } from './math-render';
import { isImageFileName } from './attachment-images';

/** How Live mode gets image bytes. Without one, images stay as written. */
export interface LiveImageHost {
  load(target: string): Promise<string | null>;
  failedText(target: string): string;
}
export const liveImageHost = Facet.define<LiveImageHost, LiveImageHost | null>({ combine: hosts => hosts[0] ?? null });

class ImageWidget extends WidgetType {
  constructor(readonly target: string, readonly alt: string) { super(); }
  override eq(other: ImageWidget): boolean { return this.target === other.target && this.alt === other.alt; }
  override toDOM(view: EditorView): HTMLElement {
    const element = document.createElement('span');
    element.className = 'live-image';
    const host = view.state.facet(liveImageHost);
    void host?.load(this.target).then((url) => {
      if (url === null) {
        element.classList.add('md-image-blocked');
        element.textContent = host.failedText(this.target);
      } else {
        const image = document.createElement('img');
        image.className = 'md-image';
        image.alt = this.alt;
        image.onload = () => view.requestMeasure();
        image.src = url;
        element.replaceChildren(image);
      }
      view.requestMeasure();
    });
    return element;
  }
  override ignoreEvent(): boolean { return false; }
}

const LOCAL_IMAGE = /^!\[((?:[^\]\\]|\\.)*)\]\(\s*(?:<([^>\n]*)>|([^\s)]*))/;

/** Alt text and target of `![alt](target)`; null for a remote or data image. */
export function liveImageTarget(written: string): { alt: string; target: string } | null {
  const match = LOCAL_IMAGE.exec(written);
  if (match === null) return null;
  const target = match[2] ?? match[3] ?? '';
  if (target === '' || /^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith('//')) return null;
  return { alt: match[1] ?? '', target };
}

export interface LiveFormulaResult extends FormulaResult { errorText?: string }
export const mathPreviewEffect = StateEffect.define<readonly LiveFormulaResult[]>();
export const mathResultsField = StateField.define<readonly LiveFormulaResult[]>({
  create: () => [],
  update(value, transaction) {
    for (const effect of transaction.effects) if (effect.is(mathPreviewEffect)) return effect.value;
    return transaction.docChanged ? [] : value;
  },
});

class FormulaWidget extends WidgetType {
  constructor(readonly html: string, readonly block: boolean, readonly tex: string) { super(); }
  override eq(other: FormulaWidget): boolean { return this.html === other.html && this.block === other.block && this.tex === other.tex; }
  override toDOM(): HTMLElement {
    const element = document.createElement(this.block ? 'div' : 'span');
    element.className = `md-math md-math-${this.block ? 'block' : 'inline'}`;
    element.title = this.tex;
    element.innerHTML = this.html;
    return element;
  }
  override ignoreEvent(): boolean { return false; }
}

class FormulaHintWidget extends WidgetType {
  constructor(readonly message: string) { super(); }
  override eq(other: FormulaHintWidget): boolean { return this.message === other.message; }
  override toDOM(): HTMLElement {
    const element = document.createElement('span');
    element.className = 'md-math-hint';
    element.dir = 'auto';
    element.textContent = this.message;
    return element;
  }
}

class BulletWidget extends WidgetType {
  override eq(): boolean { return true; }
  override toDOM(): HTMLElement {
    const element = document.createElement('span');
    element.className = 'live-bullet';
    element.setAttribute('aria-hidden', 'true');
    element.textContent = '•';
    return element;
  }
}

/** A task checkbox; `at` is the offset of the `[` so a click can rewrite the one character inside. */
class TaskWidget extends WidgetType {
  constructor(readonly checked: boolean, readonly at: number) { super(); }
  override eq(other: TaskWidget): boolean { return this.checked === other.checked && this.at === other.at; }
  override toDOM(): HTMLElement {
    const element = document.createElement('input');
    element.type = 'checkbox';
    element.className = 'live-task';
    element.checked = this.checked;
    element.dataset.taskAt = String(this.at);
    return element;
  }
  override ignoreEvent(): boolean { return false; }
}

// Text inside these is shown as written, so a `[[link]]` there stays plain.
const VERBATIM_NODES = new Set(['InlineCode', 'FencedCode', 'CodeBlock', 'CodeText', 'MarkdownComment', 'MarkdownCommentBlock']);
const WIKI_LINK = /(!?)\[\[([^[\]\n]+?)\]\]/g;

/** The note a `[[target#anchor|label]]` link points at: the part before `#` and `|`. */
export function wikiLinkTarget(inner: string): string {
  return inner.split('|')[0]!.split('#')[0]!.trim();
}

/** Flips `[ ]`/`[x]` at `at` (the offset of `[`); null when the text there is no task marker. */
export function toggleTaskChange(doc: Text, at: number): { from: number; to: number; insert: string } | null {
  if (at < 0 || at + 3 > doc.length) return null;
  const marker = doc.sliceString(at, at + 3);
  if (!/^\[[ xX]\]$/.test(marker)) return null;
  return { from: at + 1, to: at + 2, insert: marker[1] === ' ' ? 'x' : ' ' };
}

/**
 * Clicks on Live widgets: a checkbox rewrites its one marker character as an
 * ordinary undoable edit (refused when read-only or mid-composition), and
 * Ctrl/Cmd+click on a wiki link asks the host to open its target.
 */
export function liveInteractions(follow: (target: string) => void, composing: () => boolean, openLink?: (href: string) => boolean): Extension {
  return EditorView.domEventHandlers({
    mousedown(event, view) {
      const element = event.target instanceof Element ? event.target : null;
      const task = element?.closest('.live-task');
      if (task instanceof HTMLElement) {
        event.preventDefault();
        if (view.state.readOnly || composing()) return true;
        const change = toggleTaskChange(view.state.doc, Number(task.dataset.taskAt));
        if (change) view.dispatch({ changes: change, userEvent: 'input.toggle-task' });
        return true;
      }
      const written = element?.closest('[data-link-target]');
      if (written instanceof HTMLElement && (event.ctrlKey || event.metaKey) && event.button === 0 && openLink?.(written.dataset.linkTarget ?? '') === true) {
        event.preventDefault();
        return true;
      }
      const link = element?.closest('[data-wiki-target]');
      if (link instanceof HTMLElement && (event.ctrlKey || event.metaKey) && event.button === 0) {
        event.preventDefault();
        const target = link.dataset.wikiTarget ?? '';
        if (target.length > 0) follow(target);
        return true;
      }
      return false;
    },
    // The browser would flip the checkbox on its own; the document decides its state.
    click(event) {
      if (event.target instanceof Element && event.target.closest('.live-task')) event.preventDefault();
      return false;
    },
  });
}

const formulaSpansByDocument = new WeakMap<Text, MathSpan[]>();

export function livePreviewDecorations(state: EditorState): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  const source = state.doc.toString();
  const { body, frontMatter } = splitFrontMatter(source);
  const bodyStart = frontMatter === null ? 0 : source.length - body.length;
  if (bodyStart > 0) ranges.push(Decoration.replace({ liveHidden: true }).range(0, bodyStart));
  const active = (from: number, to: number) => state.selection.ranges.some(range => range.from <= to && range.to >= from);
  let formulas = formulaSpansByDocument.get(state.doc);
  if (!formulas) { formulas = markdownFormulaSpans(source); formulaSpansByDocument.set(state.doc, formulas); }
  const results = state.field(mathResultsField, false) ?? [];
  for (const span of formulas) {
    if (active(state.doc.lineAt(span.start).from, state.doc.lineAt(span.end).to)) continue;
    const result = results.find(item => formulaKey(item) === formulaKey(span));
    if (result?.html) {
      ranges.push(Decoration.replace({ widget: new FormulaWidget(result.html, span.delimiter === 'block', span.tex),
        block: span.delimiter === 'block', liveHidden: true }).range(span.start, span.end));
    } else if (result?.errorText) {
      ranges.push(Decoration.widget({ widget: new FormulaHintWidget(result.errorText), side: 1 }).range(span.end));
    }
  }
  syntaxTree(state).iterate({ enter(node) {
    if (node.to <= bodyStart) return false;
    if (node.from < bodyStart) return;
    if (formulas.some(span => node.from >= span.start && node.to <= span.end)) return false;
    const line = state.doc.lineAt(node.from);
    const heading = /^ATXHeading([1-6])$/.exec(node.name);
    if (heading) ranges.push(Decoration.line({ class: `live-heading live-h${heading[1]}` }).range(line.from));
    if (node.name === 'Blockquote') {
      for (let at = line.from; at <= node.to; at = state.doc.lineAt(at).to + 1) {
        ranges.push(Decoration.line({ class: 'live-quote' }).range(state.doc.lineAt(at).from));
      }
    }
    // Comments stay muted with their `%%` visible, even on the active line.
    if (node.name === 'MarkdownComment' || node.name === 'MarkdownCommentBlock') {
      ranges.push(Decoration.mark({ class: 'live-comment' }).range(node.from, node.to));
      return false;
    }
    if (active(line.from, state.doc.lineAt(node.to).to)) return;
    if (node.name === 'QuoteMark') {
      ranges.push(Decoration.replace({ liveHidden: true }).range(node.from, source[node.to] === ' ' ? node.to + 1 : node.to));
    } else if (node.name === 'HorizontalRule') {
      ranges.push(Decoration.line({ class: 'live-hr' }).range(line.from));
      ranges.push(Decoration.replace({ liveHidden: true }).range(node.from, node.to));
    } else if (node.name === 'ListMark' && node.node.parent?.parent?.name === 'BulletList') {
      // A task item shows only its checkbox, as in Obsidian; a plain item gets a bullet.
      if (/^ \[[ xX]\]/.test(source.slice(node.to, node.to + 4))) {
        ranges.push(Decoration.replace({ liveHidden: true }).range(node.from, node.to + 1));
      } else {
        ranges.push(Decoration.replace({ widget: new BulletWidget(), liveHidden: true }).range(node.from, node.to));
      }
    } else if (node.name === 'TaskMarker') {
      const checked = source[node.from + 1] !== ' ';
      ranges.push(Decoration.replace({ widget: new TaskWidget(checked, node.from), liveHidden: true }).range(node.from, node.to));
      const text = source[node.to] === ' ' ? node.to + 1 : node.to;
      if (checked && text < line.to) ranges.push(Decoration.mark({ class: 'live-task-done' }).range(text, line.to));
    } else if (node.name === 'HeaderMark') {
      ranges.push(Decoration.replace({ liveHidden: true }).range(node.from, Math.min(node.to + 1, line.to)));
    } else if (['EmphasisMark', 'StrikethroughMark', 'HighlightMark', 'CodeMark'].includes(node.name)) {
      ranges.push(Decoration.replace({ liveHidden: true }).range(node.from, node.to));
    } else if (['StrongEmphasis', 'Emphasis', 'Strikethrough', 'Highlight', 'InlineCode'].includes(node.name)) {
      ranges.push(Decoration.mark({ class: `live-${node.name.toLowerCase()}` }).range(node.from, node.to));
    } else if (node.name === 'Image') {
      const image = state.facet(liveImageHost) === null ? null : liveImageTarget(source.slice(node.from, node.to));
      if (image !== null) {
        ranges.push(Decoration.replace({ widget: new ImageWidget(image.target, image.alt), liveHidden: true }).range(node.from, node.to));
      }
      return false;
    } else if (node.name === 'Link') {
      const close = source.indexOf('](', node.from);
      if (close > node.from && close < node.to) {
        ranges.push(Decoration.replace({ liveHidden: true }).range(node.from, node.from + 1));
        const href = /^\s*(?:<([^>\n]*)>|([^\s)]*))/.exec(source.slice(close + 2, node.to - 1));
        ranges.push(Decoration.mark({ class: 'live-link', attributes: { 'data-link-target': href?.[1] ?? href?.[2] ?? '' } }).range(node.from + 1, close));
        ranges.push(Decoration.replace({ liveHidden: true }).range(close, node.to));
      }
      return false;
    }
  } });
  const tree = syntaxTree(state);
  for (const match of body.matchAll(WIKI_LINK)) {
    const from = bodyStart + match.index;
    const to = from + match[0].length;
    const embeddedImage = match[1] === '!' && isImageFileName(wikiLinkTarget(match[2]!)) && state.facet(liveImageHost) !== null;
    if ((match[1] === '!' && !embeddedImage) || active(from, to)) continue;
    if (formulas.some(span => from < span.end && to > span.start)) continue;
    let inCode = false;
    for (let node: SyntaxNode | null = tree.resolveInner(from + 2, 1); node; node = node.parent) {
      if (VERBATIM_NODES.has(node.name)) { inCode = true; break; }
    }
    if (inCode) continue;
    const inner = match[2]!;
    if (embeddedImage) {
      ranges.push(Decoration.replace({ widget: new ImageWidget(wikiLinkTarget(inner), ''), liveHidden: true }).range(from, to));
      continue;
    }
    const pipe = inner.indexOf('|');
    const labelFrom = pipe < 0 ? from + 2 : from + 3 + pipe;
    if (labelFrom >= to - 2) continue;
    ranges.push(Decoration.replace({ liveHidden: true }).range(from, labelFrom));
    ranges.push(Decoration.mark({ class: 'live-wikilink', attributes: { 'data-wiki-target': wikiLinkTarget(inner) } }).range(labelFrom, to - 2));
    ranges.push(Decoration.replace({ liveHidden: true }).range(to - 2, to));
  }
  return Decoration.set(ranges, true);
}

export const livePreviewField = StateField.define<DecorationSet>({
  create: livePreviewDecorations,
  update(value, transaction) {
    return transaction.docChanged || transaction.selection || transaction.effects.some(effect => effect.is(mathPreviewEffect))
      ? livePreviewDecorations(transaction.state) : value;
  },
  provide: field => [EditorView.decorations.from(field), EditorView.atomicRanges.of(view => view.state.field(field).update({ filter: (_from, _to, decoration) => decoration.spec.liveHidden === true }))],
});
