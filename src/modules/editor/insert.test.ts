import { describe, expect, it } from 'vitest';
import {
  codeInsertion,
  detectSlashQuery,
  equationInsertion,
  headingInsertion,
  linkInsertion,
  MAX_EQUATION_CHARS,
  rankSlashItems,
  SLASH_ITEMS,
  tableInsertion,
  taskInsertion,
  textInsertion,
  validateTableSize,
  wrapText,
  type FragmentKind,
  type Selection,
} from './insert';

const labels: Record<FragmentKind, string> = {
  text: '文本',
  heading: '标题',
  task: '任务',
  table: '表格',
  equation: '公式',
  code: '代码',
  link: '链接',
};

function apply(selection: Selection, insertion: { text: string; selection: Selection }, doc: string) {
  const next = doc.slice(0, selection.from) + insertion.text + doc.slice(selection.to);
  return { next, selection: insertion.selection };
}

describe('insertion model', () => {
  it('wraps the selection and keeps it selected', () => {
    const result = wrapText({ from: 2, to: 6 }, 'bold', '**', '**', 'text');
    expect(result.text).toBe('**bold**');
    expect(result.selection).toEqual({ from: 4, to: 8 });
    expect(apply({ from: 2, to: 6 }, result, 'a bold z').next).toBe('a **bold** z');
  });

  it('places the caret between markers with no selection', () => {
    const result = wrapText({ from: 3, to: 3 }, '', '_', '_', 'emphasis');
    expect(result.text).toBe('_emphasis_');
    expect(result.selection).toEqual({ from: 4, to: 12 });
  });

  it('builds a heading with a clamped level', () => {
    expect(headingInsertion({ from: 0, to: 0 }, 9, 'Title').text).toBe('###### Title');
    expect(headingInsertion({ from: 0, to: 0 }, 0, 'Title').text).toBe('# Title');
  });

  it('replaces the selection with a table and puts the caret in the first cell', () => {
    const result = tableInsertion({ from: 4, to: 9 }, 2, 3);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const lines = result.insertion.text.split('\n');
    expect(lines).toHaveLength(4);
    expect(lines[0]).toBe('|  |  |  |');
    expect(lines[1]).toBe('| --- | --- | --- |');
    expect(lines[3]).toBe('|  |  |  |');
    expect(result.insertion.selection).toEqual({ from: 6, to: 6 });
    const doc = 'keep table here';
    const applied = apply({ from: 5, to: 10 }, result.insertion, doc);
    expect(applied.next.startsWith('keep ')).toBe(true);
    expect(applied.next).not.toContain('table');
    expect(applied.next.endsWith(' here')).toBe(true);
  });

  it('applies per-column alignment', () => {
    const result = tableInsertion({ from: 0, to: 0 }, 1, 3, ['left', 'center', 'right']);
    if (!result.ok) throw new Error('expected a table');
    expect(result.insertion.text.split('\n')[1]).toBe('| :--- | :---: | ---: |');
  });

  it('refuses out-of-range table sizes instead of clamping silently', () => {
    expect(validateTableSize(0, 3)).not.toBeNull();
    expect(validateTableSize(101, 3)).not.toBeNull();
    expect(validateTableSize(3, 0)).not.toBeNull();
    expect(validateTableSize(3, 31)).not.toBeNull();
    expect(validateTableSize(3, 3)).toBeNull();
    const refused = tableInsertion({ from: 0, to: 0 }, 3, 31);
    expect(refused).toEqual({ ok: false, error: { code: 'tableRange', rows: false, columns: true } });
  });

  it('builds one-transaction insertions whose text matches the requested size', () => {
    const result = tableInsertion({ from: 0, to: 0 }, 100, 30);
    if (!result.ok) throw new Error('expected a table');
    expect(result.insertion.text.split('\n')).toHaveLength(102);
    expect(result.insertion.text.split('\n')[0]?.split('|')).toHaveLength(32);
  });

  it('folds newlines for inline math and keeps display math on its own lines', () => {
    const inline = equationInsertion({ from: 0, to: 0 }, 'a\nb', false);
    if (!inline.ok) throw new Error('expected an equation');
    expect(inline.insertion.text).toBe('$a b$');
    const display = equationInsertion({ from: 0, to: 0 }, 'x^2', true);
    if (!display.ok) throw new Error('expected an equation');
    expect(display.insertion.text).toBe('$$\nx^2\n$$');
  });

  it('refuses an oversized equation instead of truncating it', () => {
    const long = 'x'.repeat(MAX_EQUATION_CHARS + 1);
    expect(equationInsertion({ from: 0, to: 0 }, long, false)).toEqual({
      ok: false,
      error: { code: 'equationTooLong', limit: MAX_EQUATION_CHARS },
    });
    const atLimit = equationInsertion({ from: 0, to: 0 }, 'x'.repeat(MAX_EQUATION_CHARS), false);
    expect(atLimit.ok).toBe(true);
  });

  it('strips a language that would break the fence info string', () => {
    const result = codeInsertion({ from: 0, to: 0 }, 'rs` evil', 'fn main() {}');
    expect(result.text).toBe('```rs evil\nfn main() {}\n```');
    // Backticks inside the body would close the fence early; the language is the
    // only place this helper filters, so a body with a fence is the caller's
    // problem only if it came from a template, never from typed text.
    expect(result.text.startsWith('```')).toBe(true);
  });

  it('builds a task, a link and plain text with predictable carets', () => {
    expect(taskInsertion({ from: 0, to: 0 }, ' buy milk ').text).toBe('- [ ] buy milk');
    const link = linkInsertion({ from: 0, to: 0 }, 'label', 'notes/a.md');
    expect(link.text).toBe('[label](notes/a.md)');
    // `[label](` is 8 characters, so the target range starts at index 8.
    expect(link.selection).toEqual({ from: 8, to: 18 });
    expect(textInsertion({ from: 0, to: 0 }, 'abc').selection).toEqual({ from: 3, to: 3 });
  });

  it('does not invent a link target when both text and target are empty', () => {
    const link = linkInsertion({ from: 0, to: 0 }, '', '');
    expect(link.text).toBe('[]()');
  });
});

describe('slash query detection', () => {
  it('finds a slash at the line start and after whitespace', () => {
    expect(detectSlashQuery('/table', 6)).toEqual({ from: 0, query: 'table' });
    expect(detectSlashQuery('see /code', 9)).toEqual({ from: 4, query: 'code' });
    expect(detectSlashQuery('a\n/math', 7)).toEqual({ from: 2, query: 'math' });
  });

  it('ignores a slash inside a word, a path or after whitespace', () => {
    expect(detectSlashQuery('and/or', 7)).toBeNull();
    expect(detectSlashQuery('notes/a.md', 10)).toBeNull();
    expect(detectSlashQuery('/table row', 10)).toBeNull();
  });

  it('returns null outside the source range', () => {
    expect(detectSlashQuery('/x', -1)).toBeNull();
    expect(detectSlashQuery('/x', 9)).toBeNull();
  });

  it('ranks by localized label, stable alias and prefix order', () => {
    expect(rankSlashItems('', labels).map((item) => item.kind)).toEqual(
      SLASH_ITEMS.map((item) => item.kind),
    );
    expect(rankSlashItems('表格', labels)[0]?.kind).toBe('table');
    expect(rankSlashItems('form', labels)[0]?.kind).toBe('equation');
    expect(rankSlashItems('co', labels)[0]?.kind).toBe('code');
    expect(rankSlashItems('nothing', labels)).toEqual([]);
  });

  it('applying a slash item removes the typed command from the note', () => {
    const doc = 'intro\n/tab';
    const query = detectSlashQuery(doc, doc.length);
    expect(query).not.toBeNull();
    if (query === null) return;
    const result = tableInsertion({ from: query.from, to: query.from }, 1, 2);
    if (!result.ok) throw new Error('expected a table');
    const applied = apply({ from: query.from, to: doc.length }, result.insertion, doc);
    expect(applied.next.startsWith('intro\n|')).toBe(true);
    expect(applied.next).not.toContain('/tab');
  });
});
