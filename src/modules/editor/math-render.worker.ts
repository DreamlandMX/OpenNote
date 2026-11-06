import { renderFormula } from './math-render-core';
import type { FormulaInput } from './math-render';

self.onmessage = (event: MessageEvent<FormulaInput[]>) => {
  self.postMessage(event.data.map(renderFormula));
};
