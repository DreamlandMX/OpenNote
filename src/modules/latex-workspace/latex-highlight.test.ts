import { describe, expect, it } from 'vitest';
import { latexTokens } from './latex-highlight';

const styled = (text: string) => latexTokens(text).filter((token) => token.style !== null);

describe('LaTeX colouring', () => {
  it('marks commands, braces and comments', () => {
    expect(styled('\\section{Intro} % note')).toEqual([
      { text: '\\section', style: 'keyword' },
      { text: '{', style: 'bracket' },
      { text: '}', style: 'bracket' },
      { text: '% note', style: 'comment' },
    ]);
  });

  it('treats escaped percent and dollar as commands, not comment or math', () => {
    expect(styled('50\\% of \\$5')).toEqual([
      { text: '\\%', style: 'keyword' },
      { text: '\\$', style: 'keyword' },
    ]);
  });

  it('colours inline and display math, across lines', () => {
    const tokens = latexTokens('a $x^2$ b\n$$\n\\int f\n$$ c');
    const math = tokens.filter((token) => token.style === 'string').map((token) => token.text).join('');
    expect(math).toBe('$x^2$$$\\int f$$');
    expect(tokens.find((token) => token.text.includes(' c'))?.style).toBeNull();
  });
});
