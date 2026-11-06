import { expect, it } from 'vitest';
import { Compartment, EditorState } from '@codemirror/state';
import { markdown } from '@codemirror/lang-markdown';
import { livePreviewDecorations, livePreviewField, mathPreviewEffect, mathResultsField, toggleTaskChange, wikiLinkTarget } from './live-preview';
import { history, undo } from '@codemirror/commands';
import { renderFormula } from './math-render-core';
import { EditorView } from '@codemirror/view';
import { initialEditorPosition } from './NoteEditor';
import { liveMarkdownSyntaxExtensions } from './markdown-language';

it('hides valid metadata and inactive Markdown markers while leaving the active line editable', () => {
  const doc = '---\nopennote_id: id\n---\n# Heading\n\n**bold**\n\nEdit here';
  const state = EditorState.create({ doc, selection: { anchor: doc.length }, extensions: [markdown()] });
  const ranges: { from: number; to: number }[] = [];
  livePreviewDecorations(state).between(0, doc.length, (from, to) => { if (from !== to) ranges.push({ from, to }); });
  expect(ranges).toContainEqual({ from: 0, to: doc.indexOf('#') });
  expect(ranges).toContainEqual({ from: doc.indexOf('#'), to: doc.indexOf('#') + 2 });
  const active = EditorState.create({ doc: '**bold**', selection: { anchor: 4 }, extensions: [markdown()] });
  expect(livePreviewDecorations(active).size).toBe(0);
  expect(state.doc.toString()).toBe(doc);
});

it('decorates a freshly opened note that already has frontmatter', () => {
  const doc = '---\nopennote-id: 9a54f65f-f147-4b5e-84cf-c1448990b51d\n---\n# Standalone editor check';
  const state = EditorState.create({ doc, selection: { anchor: initialEditorPosition(doc, 'live') }, extensions: liveMarkdownSyntaxExtensions() });
  const classes: string[] = [];
  state.field(livePreviewField).between(0, doc.length, (_from, _to, value) => { if (value.spec.class) classes.push(value.spec.class); });
  expect(classes).toContain('live-heading live-h1');
});

it('decorates headings when switching an existing source editor back into Live', () => {
  const language = new Compartment();
  let state = EditorState.create({ doc: '# Heading', extensions: language.of([]) });
  state = state.update({ effects: language.reconfigure(liveMarkdownSyntaxExtensions()) }).state;
  const classes: string[] = [];
  state.field(livePreviewField).between(0, state.doc.length, (_from, _to, value) => { if (value.spec.class) classes.push(value.spec.class); });
  expect(classes).toContain('live-heading live-h1');
});

it('starts live editing after hidden metadata so typing cannot insert before the YAML header', () => {
  const source = '---\nopennote-id: id\n---\n';
  expect(initialEditorPosition(source, 'live')).toBe(source.length);
  expect(initialEditorPosition(source, 'source')).toBe(0);
  expect(initialEditorPosition('# note', 'live')).toBe(0);
  const crlf = source.replaceAll('\n', '\r\n');
  expect(() => EditorState.create({ doc: crlf, selection: { anchor: initialEditorPosition(crlf, 'live') } })).not.toThrow();
  expect(initialEditorPosition(crlf, 'live')).toBe(source.length);
});

it('keeps visible bold text navigable instead of treating the whole word as an atomic object', () => {
  const state = EditorState.create({ doc: '**word**\n\nend', selection: { anchor: 13 }, extensions: [markdown(), livePreviewField] });
  const atomic = state.facet(EditorView.atomicRanges)[0]!({ state } as EditorView);
  const ranges: [number, number][] = [];
  atomic.between(0, state.doc.length, (from, to) => { ranges.push([from, to]); });
  expect(ranges).toEqual([[0, 2], [6, 8]]);
});

it('replaces inactive formulas, restores source on the active line and discards results after edits', () => {
  const doc = '$x$\n\nEdit here';
  let state = EditorState.create({ doc, selection: { anchor: doc.length }, extensions: liveMarkdownSyntaxExtensions() });
  state = state.update({ effects: mathPreviewEffect.of([renderFormula({ tex: 'x', delimiter: 'inline' })]) }).state;
  const widgets: [number, number][] = [];
  state.field(livePreviewField).between(0, doc.length, (from, to, value) => { if (value.spec.widget) widgets.push([from, to]); });
  expect(widgets).toContainEqual([0, 3]);
  state = state.update({ selection: { anchor: 1 } }).state;
  expect(state.field(livePreviewField).size).toBe(0);
  state = state.update({ changes: { from: 1, to: 2, insert: 'y' } }).state;
  expect(state.field(mathResultsField)).toEqual([]);
  expect(state.doc.toString()).toBe('$y$\n\nEdit here');
});

it('keeps code, link destinations and metadata literal even when another formula has identical TeX', () => {
  const doc = '---\nvalue: $x$\n---\n\n    $x$\n\n`$x$`\n\n[a](https://example.com/$x$)\n\n$x$\n\nend';
  let state = EditorState.create({ doc, selection: { anchor: doc.length }, extensions: liveMarkdownSyntaxExtensions() });
  state = state.update({ effects: mathPreviewEffect.of([renderFormula({ tex: 'x', delimiter: 'inline' })]) }).state;
  const widgets: [number, number][] = [];
  state.field(livePreviewField).between(0, doc.length, (from, to, value) => { if (value.spec.widget) widgets.push([from, to]); });
  expect(widgets).toEqual([[doc.lastIndexOf('$x$'), doc.lastIndexOf('$x$') + 3]]);
});

it('supports a block formula spanning lines without changing the document', () => {
  const doc = '$$\nx+y\n$$\n\nend';
  let state = EditorState.create({ doc, selection: { anchor: doc.length }, extensions: liveMarkdownSyntaxExtensions() });
  state = state.update({ effects: mathPreviewEffect.of([renderFormula({ tex: 'x+y', delimiter: 'block' })]) }).state;
  const blocks: boolean[] = [];
  state.field(livePreviewField).between(0, doc.length, (_from, _to, value) => { if (value.spec.widget) blocks.push(value.spec.block); });
  expect(blocks).toEqual([true]);
  expect(state.doc.toString()).toBe(doc);
});

it('previews formulas nested in quotes and lists using their actual source offsets', () => {
  const doc = '> $$\n> x+y\n> $$\n\n- `$x$` then $x$\n\nend';
  let state = EditorState.create({ doc, selection: { anchor: doc.length }, extensions: liveMarkdownSyntaxExtensions() });
  state = state.update({ effects: mathPreviewEffect.of([
    renderFormula({ tex: 'x+y', delimiter: 'block' }), renderFormula({ tex: 'x', delimiter: 'inline' }),
  ]) }).state;
  const widgets: [number, number][] = [];
  state.field(livePreviewField).between(0, doc.length, (from, to, value) => { if (value.spec.widget?.constructor.name === 'FormulaWidget') widgets.push([from, to]); });
  expect(widgets).toEqual([[2, doc.indexOf('$$', 4) + 2], [doc.lastIndexOf('$x$'), doc.lastIndexOf('$x$') + 3]]);
  expect(state.doc.toString()).toBe(doc);
});

function decorated(doc: string, anchor = doc.length) {
  const state = EditorState.create({ doc, selection: { anchor }, extensions: liveMarkdownSyntaxExtensions() });
  const replaced: [number, number][] = [];
  const widgets: string[] = [];
  const classes: string[] = [];
  state.field(livePreviewField).between(0, doc.length, (from, to, value) => {
    if (value.spec.widget) widgets.push(value.spec.widget.constructor.name);
    else if (from !== to && value.spec.liveHidden) replaced.push([from, to]);
    if (value.spec.class) classes.push(value.spec.class);
  });
  return { state, replaced, widgets, classes };
}

it('draws bullets, task boxes, quotes and rules away from the edited line', () => {
  const doc = '- item\n- [ ] open\n- [x] done\n\n> quoted\n\n---\n\nend';
  const { widgets, classes, replaced } = decorated(doc);
  expect(widgets.filter(name => name === 'BulletWidget')).toHaveLength(1);
  expect(widgets.filter(name => name === 'TaskWidget')).toHaveLength(2);
  // A task item hides "- " instead of drawing a bullet in front of its box.
  expect(replaced).toContainEqual([doc.indexOf('- [ ]'), doc.indexOf('- [ ]') + 2]);
  expect(classes).toContain('live-task-done');
  expect(classes).toContain('live-quote');
  expect(classes).toContain('live-hr');
  const editing = decorated(doc, doc.indexOf('open'));
  expect(editing.widgets.filter(name => name === 'TaskWidget')).toHaveLength(1);
  expect(editing.state.doc.toString()).toBe(doc);
});

it('keeps ordered list numbers as typed', () => {
  expect(decorated('1. one\n2. two\n\nend').widgets).toEqual([]);
});

it('shows a wiki link label without brackets, but not inside code or while editing it', () => {
  const doc = 'See [[Research/Reading list|the list]] and [[Ideas#Top]].\n\n`[[code]]`\n\nend';
  const { replaced, classes } = decorated(doc);
  const first = doc.indexOf('[[Research');
  expect(replaced).toContainEqual([first, doc.indexOf('the list')]);
  expect(replaced).toContainEqual([doc.indexOf(']]'), doc.indexOf(']]') + 2]);
  expect(classes.filter(name => name === 'live-wikilink')).toHaveLength(2);
  expect(decorated(doc, first + 4).classes.filter(name => name === 'live-wikilink')).toHaveLength(1);
  expect(wikiLinkTarget('Research/Reading list|the list')).toBe('Research/Reading list');
  expect(wikiLinkTarget(' Ideas#Top ')).toBe('Ideas');
});

it('toggles exactly one marker character as an undoable edit', () => {
  let state = EditorState.create({ doc: '- [ ] task', extensions: [history()] });
  const change = toggleTaskChange(state.doc, 2)!;
  expect(change).toEqual({ from: 3, to: 4, insert: 'x' });
  state = state.update({ changes: change, userEvent: 'input.toggle-task' }).state;
  expect(state.doc.toString()).toBe('- [x] task');
  expect(toggleTaskChange(state.doc, 2)).toEqual({ from: 3, to: 4, insert: ' ' });
  expect(toggleTaskChange(state.doc, 0)).toBeNull();
  expect(toggleTaskChange(state.doc, 99)).toBeNull();
  let undone: EditorState | null = null;
  undo({ state, dispatch: transaction => { undone = transaction.state; } });
  expect(undone!.doc.toString()).toBe('- [ ] task');
});

function markRanges(doc: string, name: string, anchor = doc.length): [number, number][] {
  const state = EditorState.create({ doc, selection: { anchor }, extensions: liveMarkdownSyntaxExtensions() });
  const ranges: [number, number][] = [];
  state.field(livePreviewField).between(0, doc.length, (from, to, value) => { if (value.spec.class === name) ranges.push([from, to]); });
  return ranges;
}

it('highlights ==text== and hides its markers away from the edited line', () => {
  const doc = 'a ==marked **bold**== b\n\nend';
  const { replaced, classes } = decorated(doc);
  expect(markRanges(doc, 'live-highlight')).toEqual([[2, 21]]);
  expect(replaced).toContainEqual([2, 4]);
  expect(replaced).toContainEqual([19, 21]);
  expect(classes).toContain('live-strongemphasis');
  expect(decorated(doc, 5).replaced).toHaveLength(0);
  expect(markRanges('a == b == c\n\nend', 'live-highlight')).toEqual([]);
  expect(markRanges('`==code==`\n\nend', 'live-highlight')).toEqual([]);
});

it('mutes inline and block %% comments and keeps their markers and links plain', () => {
  const inline = 'keep %%hidden [[Note]]%% keep\n\nend';
  expect(markRanges(inline, 'live-comment')).toEqual([[5, 24]]);
  expect(markRanges(inline, 'live-comment', 8)).toEqual([[5, 24]]);
  expect(decorated(inline).classes).not.toContain('live-wikilink');
  expect(decorated(inline).replaced).toHaveLength(0);
  const block = 'before\n\n%%\n# not a heading\n**not bold**\n%%\nafter';
  const commentFrom = block.indexOf('%%');
  const commentTo = block.lastIndexOf('%%') + 2;
  expect(markRanges(block, 'live-comment')).toEqual([[commentFrom, commentTo]]);
  expect(decorated(block).classes).not.toContain('live-heading live-h1');
  expect(decorated(block).classes).not.toContain('live-strongemphasis');
  expect(markRanges('%% one line %%\n\nend', 'live-comment')).toEqual([[0, 14]]);
  const open = 'text\n\n%% never closed\nstill comment';
  expect(markRanges(open, 'live-comment', 0)).toEqual([[open.indexOf('%%'), open.length]]);
  expect(markRanges('100%% sure, 50%\n\nend', 'live-comment')).toEqual([]);
});
