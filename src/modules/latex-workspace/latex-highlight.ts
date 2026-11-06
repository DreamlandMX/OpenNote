import { HighlightStyle, StreamLanguage, StringStream, syntaxHighlighting, type StreamParser } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import type { Extension } from '@codemirror/state';

interface LatexState {
  /** Inside `$…$` or `$$…$$`, which may span lines. */
  math: false | '$' | '$$';
}

/**
 * A small LaTeX tokenizer for colouring only: commands, comments, math and
 * braces. It does not parse the document, so a malformed file still colours
 * and never blocks typing.
 */
export const latexParser: StreamParser<LatexState> = {
  name: 'latex',
  startState: () => ({ math: false }),
  copyState: (state) => ({ ...state }),
  token(stream, state) {
    // Commands first, so `\%` and `\$` are escapes rather than comment or math.
    if (stream.match(/^\\(?:[A-Za-z@]+\*?|.)/)) return state.math ? 'string' : 'keyword';
    if (stream.peek() === '%') {
      stream.skipToEnd();
      return 'comment';
    }
    if (stream.match('$$')) {
      state.math = state.math === '$$' ? false : state.math === false ? '$$' : state.math;
      return 'string';
    }
    if (stream.eat('$')) {
      state.math = state.math === '$' ? false : state.math === false ? '$' : state.math;
      return 'string';
    }
    if (stream.eat(/[{}[\]]/)) return 'bracket';
    if (state.math) {
      stream.match(/^[^\\$%{}[\]]+/) || stream.next();
      return 'string';
    }
    stream.match(/^[^\\$%{}[\]]+/) || stream.next();
    return null;
  },
};

const latexStyle = HighlightStyle.define([
  { tag: tags.keyword, color: 'var(--latex-command, var(--accent))' },
  { tag: tags.comment, color: 'var(--secondary)', fontStyle: 'italic' },
  { tag: tags.string, color: 'var(--latex-math, var(--danger))' },
  { tag: tags.bracket, color: 'var(--secondary)' },
]);

/** The editor extensions that colour LaTeX source. */
export function latexHighlighting(): Extension[] {
  return [StreamLanguage.define(latexParser), syntaxHighlighting(latexStyle)];
}

/** Splits lines into styled pieces; tests use it to check the colouring rules. */
export function latexTokens(text: string): { text: string; style: string | null }[] {
  const state = latexParser.startState!(2);
  const out: { text: string; style: string | null }[] = [];
  for (const line of text.split('\n')) {
    const stream = new StringStream(line, 2, 2);
    while (!stream.eol()) {
      const style = latexParser.token(stream, state);
      out.push({ text: stream.current(), style });
      stream.start = stream.pos;
    }
  }
  return out;
}
