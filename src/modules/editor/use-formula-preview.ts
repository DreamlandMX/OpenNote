import { useEffect, useState } from 'react';
import { renderFormulas, type FormulaInput, type FormulaResult } from './math-render';

const EMPTY: readonly FormulaResult[] = [];

/** Scope includes note identity; old results never appear in a different buffer. */
export function useFormulaPreview(inputs: readonly FormulaInput[], scope: string): readonly FormulaResult[] {
  const key = JSON.stringify([scope, inputs]);
  const [completed, setCompleted] = useState<{ key: string; formulas: readonly FormulaResult[] } | null>(null);
  useEffect(() => {
    if (inputs.length === 0) return;
    const controller = new AbortController();
    void renderFormulas(inputs, controller.signal).then(batch => {
      if (controller.signal.aborted || batch.status === 'cancelled') return;
      const formulas: readonly FormulaResult[] = batch.status === 'ready' ? batch.formulas : inputs.map(input => ({
        ...input, html: null, error: batch.status === 'timeout' ? 'math.hint.timeout' : 'math.hint.failed',
      }));
      setCompleted({ key, formulas });
    });
    return () => controller.abort();
    // The serialized key represents every input, including its owning buffer.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return completed?.key === key ? completed.formulas : EMPTY;
}
