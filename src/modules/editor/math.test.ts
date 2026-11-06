import { describe, expect, it } from 'vitest';
import {
  budgetFallback,
  checkFormula,
  codePointLength,
  exceedsBudget,
  findFormulas,
  isDangerousHtml,
  isSafeUrl,
  katexOptions,
  MAX_EXPAND,
  MAX_FORMULA_CODE_POINTS,
  MAX_SIZE_EM,
  prepareFormulas,
  RENDER_BUDGET_MS,
  skipRegions,
} from './math';

function texOf(source: string): string[] {
  return findFormulas(source).map((span) => span.tex);
}

describe('money versus formula', () => {
  it('treats two prices as text, not as a formula', () => {
    // F06-R02: `$5 and $10` must not swallow the words between the dollars.
    expect(texOf('costs $5 and $10 today')).toEqual([]);
  });

  it('does not swallow prices when a real formula follows on the same line', () => {
    expect(texOf('Pay $5 and $10; value $x$.')).toEqual(['x']);
    expect(texOf('$2+3$ is math, $4 and $5 are prices.')).toEqual(['2+3']);
    expect(texOf('$x$2 and $y$')).toEqual(['y']);
    expect(texOf('$x\ry$')).toEqual([]);
  });

  it('still finds a real inline formula', () => {
    expect(texOf('the value $x + y$ here')).toEqual(['x + y']);
  });

  it('requires a non-space after the opener and before the closer', () => {
    // A space right after `$` is the shape of a price, not a formula.
    expect(texOf('$ x$')).toEqual([]);
    // A space before the closer is likewise refused.
    expect(texOf('$x $')).toEqual([]);
    expect(texOf('$x$')).toEqual(['x']);
  });

  it('does not let an inline formula cross a newline', () => {
    // Otherwise one stray `$` would consume many lines.
    expect(texOf('a $x\n and $y')).toEqual([]);
  });

  it('finds a block formula and trims its body', () => {
    const spans = findFormulas('$$\n a = b\n$$');
    expect(spans).toHaveLength(1);
    expect(spans[0]?.delimiter).toBe('block');
    expect(spans[0]?.tex).toBe('a = b');
  });

  it('leaves an unclosed $$ as text', () => {
    expect(texOf('$$ a = b')).toEqual([]);
  });

  it('requires block delimiters on their own lines and rejects a digit after an inline closer', () => {
    expect(texOf('cost $x$2')).toEqual([]);
    expect(texOf('before $$x$$ after')).toEqual([]);
    expect(texOf('$$\nx + y\n$$')).toEqual(['x + y']);
    expect(texOf('$$\nx\n$$ trailing')).toEqual([]);
  });

  it('distinguishes odd and even backslash escapes', () => {
    expect(texOf(String.raw`\$x$`)).toEqual([]);
    expect(texOf(String.raw`\\$x$`)).toEqual(['x']);
  });
});

describe('code and escapes', () => {
  it('ignores a $ inside an inline code span', () => {
    // F06-A02: code is code.
    expect(texOf('use `$x$` literally')).toEqual([]);
  });

  it('ignores a $ inside a fenced block', () => {
    expect(texOf('```\n$x$\n```\n')).toEqual([]);
  });

  it('ignores a $ after the escape that makes it literal', () => {
    expect(texOf('a \\$x\\$ b')).toEqual([]);
  });

  it('finds a formula after a fence ends', () => {
    expect(texOf('```\ncode\n```\n\n$x$')).toEqual(['x']);
  });

  it('masks the rest of the note for an unclosed fence', () => {
    // An unterminated fence is still code as far as a reader is concerned.
    const regions = skipRegions('```\n$x$\n');
    expect(regions.some((region) => region.kind === 'fence')).toBe(true);
  });

  it('reports the exact offsets of a formula', () => {
    const spans = findFormulas('ab $x$ cd');
    expect(spans[0]?.start).toBe(3);
    expect(spans[0]?.end).toBe(6);
  });
});

describe('limits', () => {
  it('counts code points rather than UTF-16 units', () => {
    expect(codePointLength('😀')).toBe(1);
    expect(codePointLength('ab')).toBe(2);
  });

  it('accepts a normal formula', () => {
    expect(checkFormula('a + b')).toEqual({ kind: 'ok' });
  });

  it('refuses an empty formula', () => {
    expect(checkFormula('   ')).toEqual({ kind: 'empty' });
  });

  it('refuses one over the code point limit', () => {
    // F06-R09: over the limit is shown as source, not rendered.
    const limit = checkFormula('a'.repeat(MAX_FORMULA_CODE_POINTS + 1));
    expect(limit.kind).toBe('tooLong');
  });

  it('accepts exactly the limit', () => {
    expect(checkFormula('a'.repeat(MAX_FORMULA_CODE_POINTS)).kind).toBe('ok');
  });

  it('refuses a formula deeper than the expansion limit', () => {
    const deep = '{'.repeat(MAX_EXPAND + 1);
    expect(checkFormula(deep).kind).toBe('tooDeep');
  });

  it('ignores a brace that is escaped', () => {
    // `\{` is a literal brace, not a group.
    const escaped = '\\{' + 'x'.repeat(10);
    expect(checkFormula(escaped).kind).toBe('ok');
  });
});

describe('preparation for display', () => {
  it('marks a normal formula renderable', () => {
    const views = prepareFormulas('$x$');
    expect(views[0]?.renderable).toBe(true);
    expect(views[0]?.hint).toBeNull();
  });

  it('marks an over-long formula as source with a hint', () => {
    // Never dropped: dropping loses the author's text.
    const views = prepareFormulas(`$${'a'.repeat(MAX_FORMULA_CODE_POINTS + 1)}$`);
    expect(views[0]?.renderable).toBe(false);
    expect(views[0]?.hint).toBe('math.hint.tooLong');
  });

  it('marks an empty formula with its own hint', () => {
    const views = prepareFormulas('$$\n \n$$');
    expect(views[0]?.renderable).toBe(false);
    expect(views[0]?.hint).toBe('math.hint.empty');
  });
});

describe('katex options', () => {
  it('never trusts input and never throws on a parse error', () => {
    // F06-R03: trust false is what disables \href and \url injection.
    const options = katexOptions('inline');
    expect(options.trust).toBe(false);
    expect(options.throwOnError).toBe(false);
    expect(options.strict).toBe('warn');
    expect(options.maxExpand).toBe(MAX_EXPAND);
    expect(options.maxSize).toBe(MAX_SIZE_EM);
    expect(options.displayMode).toBe(false);
  });

  it('sets display mode for a block formula', () => {
    expect(katexOptions('block').displayMode).toBe(true);
  });

  it('scopes macros to one formula so they cannot leak into another note', () => {
    expect(katexOptions('inline').globalGroup).toBe(true);
  });
});

describe('safety', () => {
  it('allows http, https and mailto only', () => {
    expect(isSafeUrl('https://example.com')).toBe(true);
    expect(isSafeUrl('http://example.com')).toBe(true);
    expect(isSafeUrl('mailto:a@b.c')).toBe(true);
    expect(isSafeUrl('relative/path.md')).toBe(true);
  });

  it('refuses a javascript or data URL', () => {
    // A javascript: URL in a note would be executable content.
    expect(isSafeUrl('javascript:alert(1)')).toBe(false);
    expect(isSafeUrl('data:text/html,<script>')).toBe(false);
    expect(isSafeUrl('')).toBe(false);
    // A protocol-relative URL can leave the app's origin.
    expect(isSafeUrl('//evil.example')).toBe(false);
  });

  it('detects dangerous pasted HTML', () => {
    expect(isDangerousHtml('<script>alert(1)</script>')).toBe(true);
    expect(isDangerousHtml('<img onerror="x">')).toBe(true);
    expect(isDangerousHtml('<iframe src="x">')).toBe(true);
    expect(isDangerousHtml('plain text')).toBe(false);
    expect(isDangerousHtml('[link](https://example.com)')).toBe(false);
  });
});

describe('render budget', () => {
  it('reports exceeding the budget', () => {
    expect(exceedsBudget(0, RENDER_BUDGET_MS)).toBe(false);
    expect(exceedsBudget(0, RENDER_BUDGET_MS + 1)).toBe(true);
  });

  it('falls back to source for the remaining formulas', () => {
    // A partially rendered note that looks complete would be a lie.
    const views = prepareFormulas('$a$ $b$ $c$');
    const fallback = budgetFallback(views, 1);
    expect(fallback.render).toHaveLength(1);
    expect(fallback.asSource).toHaveLength(2);
    expect(fallback.notice).toBe('math.budgetExceeded');
  });
});
