import { markdown } from '@codemirror/lang-markdown';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import type { Extension } from '@codemirror/state';
import { Tag, tags as t } from '@lezer/highlight';
import { GFM, type MarkdownConfig } from '@lezer/markdown';
import { livePreviewField, mathResultsField } from './live-preview';

const highlightTag = Tag.define();
const PUNCTUATION = /[\p{S}\p{P}]/u;
const EQUALS = 61;
const PERCENT = 37;
// Delimiters pair by identity, so every `==` must share this object.
const HIGHLIGHT_DELIMITER = { resolve: 'Highlight', mark: 'HighlightMark' };

/** `==text==`, delimited like GFM strikethrough so it nests with emphasis. */
export const Highlight: MarkdownConfig = {
  defineNodes: [
    { name: 'Highlight', style: { 'Highlight/...': highlightTag } },
    { name: 'HighlightMark', style: t.processingInstruction },
  ],
  parseInline: [{
    name: 'Highlight',
    parse(cx, next, pos) {
      if (next !== EQUALS || cx.char(pos + 1) !== EQUALS || cx.char(pos + 2) === EQUALS) return -1;
      const before = cx.slice(pos - 1, pos);
      const after = cx.slice(pos + 2, pos + 3);
      const spaceBefore = /\s|^$/.test(before);
      const spaceAfter = /\s|^$/.test(after);
      const punctBefore = PUNCTUATION.test(before);
      const punctAfter = PUNCTUATION.test(after);
      return cx.addDelimiter(HIGHLIGHT_DELIMITER, pos, pos + 2,
        !spaceAfter && (!punctAfter || spaceBefore || punctBefore),
        !spaceBefore && (!punctBefore || spaceAfter || punctAfter));
    },
    after: 'Emphasis',
  }],
};

/**
 * Obsidian comments. Inline, `%%text%%` runs to the next `%%` in the same
 * paragraph and its content is not parsed. A line that opens with `%%` and
 * does not close on that line starts a block that ends at the next line holding
 * `%%`, or at the end of the note, like an unclosed code fence.
 */
export const Comment: MarkdownConfig = {
  defineNodes: [
    { name: 'MarkdownComment', style: { 'MarkdownComment/...': t.comment } },
    { name: 'MarkdownCommentBlock', block: true, style: { 'MarkdownCommentBlock/...': t.comment } },
    { name: 'MarkdownCommentMark', style: t.processingInstruction },
  ],
  parseInline: [{
    name: 'MarkdownComment',
    parse(cx, next, pos) {
      if (next !== PERCENT || cx.char(pos + 1) !== PERCENT) return -1;
      const close = cx.slice(pos + 2, cx.end).indexOf('%%');
      if (close < 0) return -1;
      const end = pos + 2 + close + 2;
      return cx.addElement(cx.elt('MarkdownComment', pos, end,
        [cx.elt('MarkdownCommentMark', pos, pos + 2), cx.elt('MarkdownCommentMark', end - 2, end)]));
    },
    before: 'Emphasis',
  }],
  parseBlock: [{
    name: 'MarkdownCommentBlock',
    parse(cx, line) {
      if (line.next !== PERCENT || line.text.charCodeAt(line.pos + 1) !== PERCENT) return false;
      const start = cx.lineStart + line.pos;
      const sameLine = line.text.indexOf('%%', line.pos + 2);
      // `%%a%% more text` is an inline comment in a paragraph.
      if (sameLine >= 0 && line.text.slice(sameLine + 2).trim() !== '') return false;
      const marks = [cx.elt('MarkdownCommentMark', start, start + 2)];
      let end = cx.lineStart + sameLine + 2;
      let closed = sameLine >= 0;
      if (!closed) {
        end = cx.lineStart + line.text.length;
        while (cx.nextLine()) {
          const close = line.text.indexOf('%%');
          if (close >= 0) { end = cx.lineStart + close + 2; closed = true; break; }
          end = cx.lineStart + line.text.length;
        }
      }
      if (closed) marks.push(cx.elt('MarkdownCommentMark', end - 2, end));
      cx.nextLine();
      cx.addElement(cx.elt('MarkdownCommentBlock', start, end, marks));
      return true;
    },
    before: 'LinkReference',
  }],
};

export const markdownHighlightStyle: HighlightStyle = HighlightStyle.define([
  { tag: [t.heading1, t.heading2, t.heading3, t.heading4, t.heading5, t.heading6], color: 'var(--text)', fontWeight: '700' },
  { tag: t.strong, fontWeight: '700', color: 'var(--text)' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
  { tag: [t.link, t.url], color: 'var(--accent)', textDecoration: 'underline' },
  { tag: t.monospace, color: 'var(--text)', fontFamily: 'var(--font-mono)' },
  { tag: [t.processingInstruction, t.meta], color: 'var(--secondary)' },
  { tag: t.quote, color: 'var(--secondary)' },
  { tag: t.list, color: 'var(--text)' },
  { tag: t.comment, color: 'var(--secondary)' },
  { tag: t.labelName, color: 'var(--accent)' },
  { tag: highlightTag, backgroundColor: 'var(--highlight)' },
]);

export function liveMarkdownSyntaxExtensions(): readonly Extension[] {
  return [markdown({ extensions: [GFM, Highlight, Comment] }), syntaxHighlighting(markdownHighlightStyle), mathResultsField, livePreviewField];
}
