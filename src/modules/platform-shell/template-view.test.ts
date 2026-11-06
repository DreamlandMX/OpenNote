import { describe, expect, it, vi } from 'vitest';
import type { TemplateExpansion, TemplateRow } from '../editor';
import {
  insertable,
  insertAtSelection,
  needsAttention,
  noteNameFor,
  summarizeWarnings,
  templateBody,
  toChoices,
  variablesFor,
  currentTemplatePreview,
  observeTemplatePreview,
  sameTemplateTarget,
  applyTemplateInsert,
  type TemplatePreviewSnapshot,
  type TemplateInsertTarget,
} from './template-view';

const template: TemplateRow = {
  noteId: 'n1',
  relativePath: 'templates/weekly.md',
  title: 'weekly',
  body: '---\ntemplate: true\n---\n# Weekly\n\nReview {{date}}\n',
};

describe('template bodies', () => {
  it('strips the frontmatter marker so it is never inserted', () => {
    expect(templateBody(template.body)).toBe('# Weekly\n\nReview {{date}}\n');
  });

  it('handles CRLF, a missing marker and a trailing marker', () => {
    expect(templateBody('---\r\ntemplate: true\r\n---\r\nbody')).toBe('body');
    expect(templateBody('body only')).toBe('body only');
    expect(templateBody('---\ntemplate: true\n...\nbody')).toBe('body');
  });

  it('does not strip a horizontal rule in the middle of a note', () => {
    const body = '\n---\nnot frontmatter\n';
    expect(templateBody(body)).toBe(body);
  });
});

describe('template choices', () => {
  it('sorts by title and summarizes the first prose line', () => {
    const choices = toChoices([
      { ...template, noteId: 'b', title: 'beta', body: '---\ntemplate: true\n---\n# Heading\n\nProse\n' },
      { ...template, noteId: 'a', title: 'alpha' },
    ]);
    expect(choices.map((choice) => choice.title)).toEqual(['alpha', 'beta']);
    expect(choices[0]?.summary).toBe('Review {{date}}');
    // A leading heading is not a summary; the prose after it is.
    expect(choices[1]?.summary).toBe('Prose');
  });

  it('gives an empty summary when there is no prose', () => {
    const choices = toChoices([{ ...template, body: '---\ntemplate: true\n---\n' }]);
    expect(choices[0]?.summary).toBe('');
  });
});

describe('expansion warnings', () => {
  it('deduplicates an unknown variable reported twice', () => {
    const summary = summarizeWarnings([
      { kind: 'unknownVariable', name: 'mood' },
      { kind: 'unknownVariable', name: 'mood' },
      { kind: 'unknownVariable', name: 'alpha' },
      { kind: 'unterminated' },
    ]);
    expect(summary).toEqual({ unknown: ['alpha', 'mood'], unterminated: true });
  });

  it('reports nothing for a clean expansion', () => {
    expect(summarizeWarnings([])).toEqual({ unknown: [], unterminated: false });
  });
});

describe('insertable fragments', () => {
  it('uses the cursor marker when present', () => {
    expect(insertable({ fragment: 'before  after', warnings: [], cursorOffset: 7 })).toEqual({
      text: 'before  after',
      caret: 7,
    });
  });

  it('falls back to the end when there is no marker', () => {
    expect(insertable({ fragment: 'text', warnings: [], cursorOffset: null })).toEqual({
      text: 'text',
      caret: 4,
    });
  });

  it('clamps a malformed cursor offset instead of trusting it', () => {
    expect(insertable({ fragment: 'abc', warnings: [], cursorOffset: 99 }).caret).toBe(3);
    expect(insertable({ fragment: 'abc', warnings: [], cursorOffset: -4 }).caret).toBe(0);
    const nan = { fragment: 'abc', warnings: [], cursorOffset: Number.NaN } as TemplateExpansion;
    expect(nan.cursorOffset).toBeNaN();
    expect(insertable(nan).caret).toBe(3);
  });

  it('converts a host byte cursor after CJK and emoji into editor positions', () => {
    expect(insertable({ fragment: '京都😀 X', warnings: [], cursorOffset: 10 })).toEqual({ text: '京都😀 X', caret: 4 });
  });

  it('normalizes CRLF for the editor without shifting the cursor marker', () => {
    expect(insertable({ fragment: '京都\r\n😀Z', warnings: [], cursorOffset: 12 })).toEqual({ text: '京都\n😀Z', caret: 5 });
  });

  it('needs attention exactly when a warning exists', () => {
    expect(needsAttention({ fragment: '', warnings: [], cursorOffset: null })).toBe(false);
    expect(
      needsAttention({
        fragment: '',
        warnings: [{ kind: 'unknownVariable', name: 'x' }],
        cursorOffset: null,
      }),
    ).toBe(true);
  });
});

describe('note naming and variables', () => {
  it('sanitizes the same characters the host refuses', () => {
    expect(noteNameFor('Weekly review')).toBe('Weekly review');
    expect(noteNameFor('a/b:c*d?e')).toBe('a-b-c-d-e');
    expect(noteNameFor('  trailing.  ')).toBe('trailing');
    expect(noteNameFor('   ')).toBe('Untitled');
    expect(noteNameFor('京都')).toBe('京都');
  });

  it('builds variables from the chosen title and local date', () => {
    expect(variablesFor('T', '2026-10-20', '09:30')).toEqual({
      title: 'T',
      date: '2026-10-20',
      time: '09:30',
    });
    expect(variablesFor('T', '2026-10-20', null)).toEqual({
      title: 'T',
      date: '2026-10-20',
      time: null,
    });
  });
});

describe('inserting into a document', () => {
  it('replaces the selection and places the caret relative to the insert', () => {
    const result = insertAtSelection('before SELECTED after', { from: 7, to: 15 }, 'X', 1);
    expect(result.source).toBe('before X after');
    expect(result.caret).toBe(8);
  });

  it('appends at the caret when there is no selection', () => {
    expect(insertAtSelection('abc', { from: 3, to: 3 }, 'Z', 1)).toEqual({
      source: 'abcZ',
      caret: 4,
    });
  });

  it('appends at the end when there is no selection information', () => {
    expect(insertAtSelection('abc', null, 'Z', 0)).toEqual({ source: 'abcZ', caret: 3 });
  });

  it('clamps a selection outside the document instead of throwing', () => {
    expect(insertAtSelection('abc', { from: -5, to: 99 }, 'Z', 1)).toEqual({
      source: 'Z',
      caret: 1,
    });
  });

  it('clamps a caret that points past the fragment', () => {
    const result = insertAtSelection('', null, 'frag', 99);
    expect(result.source).toBe('frag');
    expect(result.caret).toBe(4);
  });
});

describe('preview identity and lifetime', () => {
  const choice = toChoices([template])[0]!;
  const variables = variablesFor('Title', '2026-10-25', null);
  const service = { expandTemplate: vi.fn(async () => ({ ok: true as const, value: { fragment: 'reviewed', warnings: [], cursorOffset: null } })) };
  const outcome = { ok: true as const, value: { fragment: 'reviewed', warnings: [], cursorOffset: null } };
  const snapshot: TemplatePreviewSnapshot = { service, noteId: choice.noteId, body: choice.body, variables, outcome };

  it('uses only the exact reviewed request including service, template body and all variables', () => {
    expect(currentTemplatePreview(snapshot, service, choice, variables)).toEqual(outcome);
    expect(currentTemplatePreview(snapshot, { ...service }, choice, variables)).toBeNull();
    expect(currentTemplatePreview(snapshot, service, { ...choice, noteId: 'other' }, variables)).toBeNull();
    expect(currentTemplatePreview(snapshot, service, { ...choice, body: 'new template' }, variables)).toBeNull();
    expect(currentTemplatePreview(snapshot, service, null, variables)).toBeNull();
    for (const changed of [{ ...variables, title: 'New' }, { ...variables, date: '2026-10-24' }, { ...variables, time: '09:00' }]) {
      expect(currentTemplatePreview(snapshot, service, choice, changed)).toBeNull();
    }
  });

  it('does not publish a request that resolves after it was replaced or closed', async () => {
    let finish!: (value: typeof outcome) => void;
    const deferred = { expandTemplate: () => new Promise<typeof outcome>(resolve => { finish = resolve; }) };
    const publish = vi.fn();
    const stop = observeTemplatePreview(deferred, choice, variables, publish);
    stop();
    finish(outcome);
    await Promise.resolve();
    expect(publish).not.toHaveBeenCalled();
  });

  it('publishes a typed thrown failure bound to its inputs, not a reusable old preview', async () => {
    const publish = vi.fn();
    const failing = { expandTemplate: async () => { throw Error('unavailable'); } };
    observeTemplatePreview(failing, choice, variables, publish);
    await Promise.resolve();
    const published = publish.mock.calls[0]?.[0] as TemplatePreviewSnapshot;
    expect(currentTemplatePreview(published, failing, choice, variables)).toMatchObject({ ok: false, error: { code: 'IO_DENIED' } });
    expect(currentTemplatePreview(published, failing, choice, { ...variables, title: 'New' })).toBeNull();
  });
});

describe('template insertion target', () => {
  const target: TemplateInsertTarget = { noteId: 'n1', paneId: 'left', source: 'before SELECTED after', selection: { from: 7, to: 15 } };
  it('sends the captured source and selection to the editor for atomic validation', () => {
    const apply = vi.fn(() => true);
    expect(applyTemplateInsert(target, { ...target }, apply, 'X', 1, true)).toBe(true);
    expect(apply).toHaveBeenCalledExactlyOnceWith([{ start: 7, end: 15, text: 'X' }], { noteId: 'n1', source: target.source, selection: target.selection, caret: 8 });
  });
  it('rejects changes to note, pane, source, caret, permissions or editor availability', () => {
    const apply = vi.fn(() => true);
    for (const current of [{ ...target, noteId: 'other' }, { ...target, paneId: 'right' }, { ...target, source: 'new draft' }, { ...target, selection: { from: 8, to: 15 } }, null]) {
      expect(sameTemplateTarget(target, current)).toBe(false);
      expect(applyTemplateInsert(target, current, apply, 'X', 1, true)).toBe(false);
    }
    expect(applyTemplateInsert(target, target, apply, 'X', 1, false)).toBe(false);
    expect(applyTemplateInsert(target, target, undefined, 'X', 1, true)).toBe(false);
    expect(apply).not.toHaveBeenCalled();
  });
});
