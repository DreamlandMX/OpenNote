import { describe, expect, it } from 'vitest';
import {
  parseViewDefinition,
  serializeViewDefinition,
  viewNameFromPath,
  viewPathFromName,
  type ViewDefinition,
} from './view-definition';

const base: ViewDefinition = {
  formatVersion: 1,
  name: '读书清单',
  source: { folder: 'Books', recursive: false },
  viewType: 'board',
  filters: [{ key: 'status', op: 'equals', value: 'reading' }],
  sort: { key: 'title', direction: 'descending' },
  groupKey: 'status',
  dateKey: '',
  extra: {},
};

describe('view definitions', () => {
  it('round-trips every setting and Unicode', () => {
    const parsed = parseViewDefinition(serializeViewDefinition(base));
    expect(parsed).toEqual({ ok: true, readOnly: false, value: base });
  });

  it('keeps unknown fields, including nested ones in source', () => {
    const text = JSON.stringify({
      formatVersion: 1, viewType: 'table', future: { a: [1, 2] }, columns: ['x'],
      source: { folder: '', recursive: true, tag: 'later' },
    });
    const parsed = parseViewDefinition(text);
    if (!parsed.ok) throw new Error('parse');
    const again = JSON.parse(serializeViewDefinition(parsed.value)) as Record<string, unknown>;
    expect(again.future).toEqual({ a: [1, 2] });
    expect(again.columns).toEqual(['x']);
    expect(again.source).toEqual({ folder: '', recursive: true, tag: 'later' });
  });

  it('applies documented defaults for missing optional fields', () => {
    const parsed = parseViewDefinition('{"formatVersion":1}');
    expect(parsed.ok && parsed.value).toMatchObject({
      viewType: 'table', filters: [], sort: null, source: { folder: '', recursive: true },
    });
  });

  it('opens a newer format read-only instead of rewriting it', () => {
    const parsed = parseViewDefinition('{"formatVersion":2,"viewType":"table"}');
    expect(parsed.ok && parsed.readOnly).toBe(true);
  });

  it('refuses unknown view types, operators, bad JSON and bad shapes rather than guessing', () => {
    expect(parseViewDefinition('{"formatVersion":1,"viewType":"timeline"}')).toEqual({ ok: false, reason: 'shape' });
    expect(parseViewDefinition('{"formatVersion":1,"filters":[{"key":"a","op":"startsWith","value":""}]}'))
      .toEqual({ ok: false, reason: 'shape' });
    expect(parseViewDefinition('{"formatVersion":1,"sort":{"key":"a","direction":"up"}}')).toEqual({ ok: false, reason: 'shape' });
    expect(parseViewDefinition('not json')).toEqual({ ok: false, reason: 'json' });
    expect(parseViewDefinition('[]')).toEqual({ ok: false, reason: 'shape' });
    expect(parseViewDefinition('{"formatVersion":"1"}')).toEqual({ ok: false, reason: 'shape' });
  });

  it('builds safe file names', () => {
    expect(viewPathFromName('Views/Reading')).toBe('Views/Reading.onview.json');
    expect(viewPathFromName('x.onview.json')).toBe('x.onview.json');
    expect(viewPathFromName('../x')).toBeNull();
    expect(viewNameFromPath('Views/Reading.onview.json')).toBe('Reading');
  });
});
