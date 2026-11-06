import { RENDER_BUDGET_MS, type MathDelimiter } from './math';

export interface FormulaInput {
  tex: string;
  delimiter: MathDelimiter;
}

export type FormulaNotice = 'math.hint.tooLong' | 'math.hint.tooDeep' | 'math.hint.empty' |
  'math.hint.invalid' | 'math.hint.timeout' | 'math.hint.failed';

export interface FormulaResult extends FormulaInput {
  html: string | null;
  error: FormulaNotice | null;
  position?: number;
}

export function formulaKey(input: FormulaInput): string {
  return `${input.delimiter}:${input.tex}`;
}

export interface FormulaBatch {
  status: 'ready' | 'timeout' | 'failed' | 'cancelled';
  formulas: readonly FormulaResult[];
}

/** Each job owns its worker: termination stops even a single expensive render. */
export function renderFormulas(formulas: readonly FormulaInput[], signal?: AbortSignal): Promise<FormulaBatch> {
  if (signal?.aborted) return Promise.resolve({ status: 'cancelled', formulas: [] });
  if (formulas.length === 0) return Promise.resolve({ status: 'ready', formulas: [] });
  return new Promise(resolve => {
    let worker: Worker;
    try {
      worker = new Worker(new URL('./math-render.worker.ts', import.meta.url), { type: 'module' });
    } catch {
      resolve({ status: 'failed', formulas: [] });
      return;
    }
    let finished = false;
    const finish = (result: FormulaBatch): void => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      signal?.removeEventListener('abort', abort);
      worker.terminate();
      resolve(result);
    };
    const abort = (): void => finish({ status: 'cancelled', formulas: [] });
    const timer = setTimeout(() => finish({ status: 'timeout', formulas: [] }), RENDER_BUDGET_MS);
    signal?.addEventListener('abort', abort, { once: true });
    worker.onmessage = (event: MessageEvent<FormulaResult[]>) => {
      const results = event.data;
      if (!Array.isArray(results) || results.length !== formulas.length || results.some((result, index) =>
        result === null || typeof result !== 'object' || result.tex !== formulas[index]?.tex ||
        result.delimiter !== formulas[index]?.delimiter || (result.html !== null && typeof result.html !== 'string'))) {
        finish({ status: 'failed', formulas: [] });
        return;
      }
      finish({ status: 'ready', formulas: results });
    };
    worker.onerror = event => { event.preventDefault(); finish({ status: 'failed', formulas: [] }); };
    try { worker.postMessage(formulas); } catch { finish({ status: 'failed', formulas: [] }); }
  });
}
