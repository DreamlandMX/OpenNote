import { useEffect, useId, useRef, useState } from 'react';
import { X } from 'lucide-react';
import type { Translator } from '../presentation-i18n';
import { checkFormula } from './math';
import type { FormulaResult } from './math-render';
import { useFormulaPreview } from './use-formula-preview';

export function formulaErrorText(result: FormulaResult, translator: Translator): string {
  if (!result.error) return '';
  return translator(result.error) + (result.position === undefined ? '' : ` ${translator('math.hint.position', { position: result.position })}`);
}

interface Props {
  initialSource: string;
  stale: boolean;
  translator: Translator;
  onInsert(tex: string, display: boolean): void;
  onClose(): void;
}

export function EquationDialog({ initialSource, stale, translator, onInsert, onClose }: Props) {
  const [source, setSource] = useState(initialSource);
  const [display, setDisplay] = useState(false);
  const input = { tex: display ? source.trim() : source.replace(/[\r\n]+/g, ' ').trim(), delimiter: display ? 'block' as const : 'inline' as const };
  const formulas = useFormulaPreview([input], 'equation-dialog');
  const result = formulas[0];
  const id = useId();
  const field = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const opener = document.activeElement;
    field.current?.focus();
    return () => { if (opener instanceof HTMLElement && opener.isConnected) opener.focus(); };
  }, []);
  return <div className="equation-shade" onKeyDown={event => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose(); }
    if (event.key === 'Tab') {
      const controls = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('textarea, input, button:not([disabled])'));
      const index = controls.indexOf(document.activeElement as HTMLElement);
      if ((event.shiftKey && index <= 0) || (!event.shiftKey && index === controls.length - 1)) {
        event.preventDefault(); controls[event.shiftKey ? controls.length - 1 : 0]?.focus();
      }
    }
  }}>
    <form className="equation-dialog" role="dialog" aria-modal="true" aria-labelledby={`${id}-title`} onSubmit={event => {
      event.preventDefault(); if (!stale && checkFormula(input.tex).kind === 'ok') onInsert(input.tex, display);
    }}>
      <header className="equation-heading">
        <h2 id={`${id}-title`}>{translator('insert.equation')}</h2>
        <button type="button" className="secondary" aria-label={translator('properties.cancel')} onClick={onClose}><X size={18} aria-hidden="true" /></button>
      </header>
      <label htmlFor={`${id}-source`}>{translator('math.source')}</label>
      <textarea ref={field} id={`${id}-source`} value={source} onChange={event => setSource(event.target.value)} dir="ltr"
        aria-describedby={`${id}-message`} aria-invalid={checkFormula(input.tex).kind !== 'ok' || result?.error === 'math.hint.invalid'} />
      <label className="equation-display"><input type="checkbox" checked={display} onChange={event => setDisplay(event.target.checked)} />{translator('math.display')}</label>
      <span className="equation-preview-label">{translator('math.preview')}</span>
      <div className="equation-preview" aria-label={translator('math.preview')}>
        {result?.html ? <div dangerouslySetInnerHTML={{ __html: result.html }} /> : <pre>{input.tex}</pre>}
        <p id={`${id}-message`} role="status">{result?.error ? formulaErrorText(result, translator) : ''}</p>
      </div>
      {stale ? <p role="alert">{translator('math.stale')}</p> : null}
      <div className="equation-actions">
        <button type="button" className="secondary" onClick={onClose}>{translator('properties.cancel')}</button>
        <button type="submit" className="primary" disabled={stale || checkFormula(input.tex).kind !== 'ok'}>{translator('math.insert')}</button>
      </div>
    </form>
  </div>;
}
