import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { LocaleContext, SUPPORTED_LOCALES, type SupportedLocaleId } from '../presentation-i18n';
import { ShortcutSettings } from './ShortcutSettings';
import { applyBinding, chordText, parseBindings, parseChord, resetBinding, type Chord, type BindableCommandId } from './shortcuts';

function render(bindings: Partial<Record<BindableCommandId, Chord>>, locale: SupportedLocaleId = 'en'): string {
  return renderToStaticMarkup(createElement(ShortcutSettings, {
    bindings,
    translator: new LocaleContext(locale).translator,
    onChange: () => undefined,
    onClose: () => undefined,
  }));
}

function row(html: string, title: string): string {
  const result = html.split('<li class="shortcut-row">').find((part) => part.startsWith(`<span class="shortcut-name">${title}</span>`));
  if (result === undefined) throw new Error(`Missing command row: ${title}`);
  return result.split('</li>')[0]!;
}

describe('shortcut settings rendered binding states', () => {
  it('shows the replaced command as unbound with a reset action instead of a default badge', () => {
    const bindings = applyBinding({}, 'workspace.commandPalette', parseChord('Ctrl+P')!, true).bindings;
    const html = render(bindings);
    const replaced = row(html, 'Quick open');
    expect(replaced).toContain('>Not bound</button>');
    expect(replaced).toContain('aria-label="Reset Quick open"');
    expect(replaced).not.toContain('shortcut-badge');
    expect(row(html, 'Command palette')).toContain('>Ctrl+P</button>');
  });

  it('renders the same binding state after text serialization and after confirmed reset', () => {
    const bindings = applyBinding({}, 'workspace.commandPalette', parseChord('Ctrl+P')!, true).bindings;
    const stored = Object.fromEntries(Object.entries(bindings).map(([id, binding]) => [id, chordText(binding!)]));
    expect(render(parseBindings(JSON.parse(JSON.stringify(stored))))).toBe(render(bindings));
    const cancelled = resetBinding(bindings, 'workspace.quickOpen');
    expect(render(cancelled.bindings)).toBe(render(bindings));
    const html = render(resetBinding(bindings, 'workspace.quickOpen', true).bindings);
    expect(row(html, 'Quick open')).toContain('>Ctrl+P</button>');
    expect(row(html, 'Quick open')).toContain('shortcut-badge');
    expect(row(html, 'Command palette')).toContain('>Not bound</button>');
  });

  it.each(SUPPORTED_LOCALES)('renders localized unbound text in $id', ({ id }) => {
    const text = new LocaleContext(id).t('shortcut.unbound');
    const escaped = renderToStaticMarkup(createElement('span', null, text)).slice(6, -7);
    expect(render({ 'workspace.quickOpen': [] }, id)).toContain(`>${escaped}</button>`);
  });
});
