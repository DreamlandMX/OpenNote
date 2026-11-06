import katex from 'katex';
import { checkFormula, katexOptions } from './math';
import type { FormulaInput, FormulaResult } from './math-render';

/** Called only in the worker; a fresh macro object prevents cross-formula state. */
export function renderFormula(input: FormulaInput): FormulaResult {
  const limit = checkFormula(input.tex);
  if (limit.kind !== 'ok') {
    const error = limit.kind === 'tooLong' ? 'math.hint.tooLong' :
      limit.kind === 'tooDeep' ? 'math.hint.tooDeep' : 'math.hint.empty';
    return { ...input, html: null, error };
  }
  try {
    const html = katex.renderToString(input.tex, { ...katexOptions(input.delimiter), macros: {} });
    // shortcut: KaTeX 0.19 encodes unsupported commands as red mtext; recheck on upgrade.
    if (html.includes('class="katex-error"') || html.includes('<mstyle mathcolor="#cc0000"><mtext>\\')) {
      const position = /at position (\d+)/.exec(html)?.[1];
      const offset = position ? Number(position) : html.includes('at end of input') ? input.tex.length + 1 : undefined;
      return { ...input, html: null, error: 'math.hint.invalid', ...(offset === undefined ? {} : { position: offset }) };
    }
    return { ...input, html, error: null };
  } catch {
    return { ...input, html: null, error: 'math.hint.invalid' };
  }
}
