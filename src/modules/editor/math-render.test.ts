import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderFormula } from './math-render-core';
import { renderFormulas, type FormulaInput, type FormulaResult } from './math-render';

const formula: FormulaInput = { tex: '\\frac{a}{b}', delimiter: 'inline' };

describe('isolated KaTeX rendering', () => {
  it('renders fractions, matrices and accessible MathML', () => {
    const result = renderFormula(formula);
    expect(result.error).toBeNull();
    expect(result.html).toContain('class="katex"');
    expect(result.html).toContain('<math');
    const block = renderFormula({ tex: '\\begin{pmatrix}a&b\\\\c&d\\end{pmatrix}', delimiter: 'block' });
    expect(block.html).toContain('katex-display');
  });

  it('keeps parse failures as source and checks the code point limit', () => {
    expect(renderFormula({ ...formula, tex: '\\frac{' })).toMatchObject({ html: null, error: 'math.hint.invalid' });
    expect(renderFormula({ ...formula, tex: 'x'.repeat(8193) })).toMatchObject({ html: null, error: 'math.hint.tooLong' });
  });

  it('blocks external images, HTML commands and executable URLs', () => {
    for (const tex of [String.raw`\includegraphics{https://example.com/x.png}`,
      String.raw`\href{javascript:alert(1)}{click}`, String.raw`\htmlStyle{background:red}{x}`]) {
      const html = renderFormula({ ...formula, tex }).html ?? '';
      expect(html).not.toMatch(/<img\b|<a\b|\shref=|style="background:red/);
    }
  });

  it('does not share global macros and bounds recursive expansion', () => {
    renderFormula({ ...formula, tex: String.raw`\gdef\opennoteLeak{123}x` });
    expect(renderFormula({ ...formula, tex: String.raw`\opennoteLeak` }).html).toBeNull();
    expect(renderFormula({ ...formula, tex: String.raw`\def\a{\a}\a` }).html).toBeNull();
  });
});

class FakeWorker {
  static instances: FakeWorker[] = [];
  onmessage: ((event: MessageEvent<FormulaResult[]>) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  terminate = vi.fn();
  postMessage = vi.fn();
  constructor() { FakeWorker.instances.push(this); }
}

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); FakeWorker.instances = []; });

describe('worker lifecycle', () => {
  it('terminates the worker after receiving results', async () => {
    vi.stubGlobal('Worker', FakeWorker);
    const pending = renderFormulas([formula]);
    const worker = FakeWorker.instances[0]!;
    worker.onmessage?.({ data: [renderFormula(formula)] } as MessageEvent<FormulaResult[]>);
    expect((await pending).status).toBe('ready');
    expect(worker.terminate).toHaveBeenCalledOnce();
  });

  it('terminates a hung job at 500 ms and ignores its late reply', async () => {
    vi.useFakeTimers(); vi.stubGlobal('Worker', FakeWorker);
    const pending = renderFormulas([formula]);
    const worker = FakeWorker.instances[0]!;
    await vi.advanceTimersByTimeAsync(500);
    expect(await pending).toEqual({ status: 'timeout', formulas: [] });
    worker.onmessage?.({ data: [renderFormula(formula)] } as MessageEvent<FormulaResult[]>);
    expect(worker.terminate).toHaveBeenCalledOnce();
  });

  it('cancels the old worker without affecting another job', async () => {
    vi.stubGlobal('Worker', FakeWorker);
    const controller = new AbortController();
    const old = renderFormulas([formula], controller.signal);
    const next = renderFormulas([formula]);
    controller.abort();
    expect((await old).status).toBe('cancelled');
    expect(FakeWorker.instances[1]!.terminate).not.toHaveBeenCalled();
    FakeWorker.instances[1]!.onmessage?.({ data: [renderFormula(formula)] } as MessageEvent<FormulaResult[]>);
    expect((await next).status).toBe('ready');
  });

  it('degrades to source when workers are unavailable or return the wrong job', async () => {
    vi.stubGlobal('Worker', class { constructor() { throw new Error('unavailable'); } });
    expect((await renderFormulas([formula])).status).toBe('failed');
    vi.stubGlobal('Worker', FakeWorker);
    const pending = renderFormulas([formula]);
    FakeWorker.instances[0]!.onmessage?.({ data: [] } as unknown as MessageEvent<FormulaResult[]>);
    expect((await pending).status).toBe('failed');
  });

  it('skips empty and already cancelled work', async () => {
    vi.stubGlobal('Worker', FakeWorker);
    const controller = new AbortController(); controller.abort();
    expect((await renderFormulas([formula], controller.signal)).status).toBe('cancelled');
    expect((await renderFormulas([])).status).toBe('ready');
    expect(FakeWorker.instances).toHaveLength(0);
  });
});
