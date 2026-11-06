import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { LocaleContext, SUPPORTED_LOCALES } from '../presentation-i18n';
import { EquationDialog, formulaErrorText } from './EquationDialog';

describe('formula insertion dialog', () => {
  it.each(SUPPORTED_LOCALES)('renders source and labeled controls in $id', ({ id }) => {
    const translator = new LocaleContext(id).translator;
    const html = renderToStaticMarkup(createElement(EquationDialog, {
      initialSource: 'x + <img>', stale: false, translator, onInsert: () => undefined, onClose: () => undefined,
    }));
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toContain('x + &lt;img&gt;');
    expect(html).not.toContain('<img>');
    expect(html).toContain('type="checkbox"');
    expect(html).toContain(translator('math.source'));
  });

  it('disables insertion when the snapshot is stale or the formula is over the limit', () => {
    const translator = new LocaleContext('en').translator;
    for (const [initialSource, stale] of [['x', true], ['x'.repeat(8193), false], ['', false]] as const) {
      const html = renderToStaticMarkup(createElement(EquationDialog, {
        initialSource, stale, translator, onInsert: () => undefined, onClose: () => undefined,
      }));
      expect(html).toContain('type="submit" class="primary" disabled=""');
      if (stale) expect(html).toContain(translator('math.stale'));
    }
  });

  it('localizes parse positions without inserting renderer diagnostics as HTML', () => {
    expect(formulaErrorText({ tex: '<img>', delimiter: 'inline', html: null, error: 'math.hint.invalid', position: 4 },
      new LocaleContext('en').translator)).toContain('Error at position 4.');
  });
});
