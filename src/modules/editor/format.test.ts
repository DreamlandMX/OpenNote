import { describe, expect, it } from 'vitest';
import { clearInline, convertBlock, externalLink, internalLink, toggleInline, type Edit } from './format';

function apply(source: string, edit: Edit): { text: string; selected: string } {
  const text = source.slice(0, edit.from) + edit.insert + source.slice(edit.to);
  return { text, selected: text.slice(edit.selection.from, edit.selection.to) };
}

describe('toggleInline', () => {
  it('wraps a selection and keeps it selected', () => {
    expect(apply('a word b', toggleInline('a word b', { from: 2, to: 6 }, '**'))).toEqual({ text: 'a **word** b', selected: 'word' });
  });

  it('removes markers just outside the selection', () => {
    expect(apply('a **word** b', toggleInline('a **word** b', { from: 4, to: 8 }, '**'))).toEqual({ text: 'a word b', selected: 'word' });
  });

  it('removes markers at both ends of the selection', () => {
    expect(apply('a ~~word~~ b', toggleInline('a ~~word~~ b', { from: 2, to: 10 }, '~~'))).toEqual({ text: 'a word b', selected: 'word' });
  });

  it('does not read a bold pair as italic', () => {
    expect(apply('**word**', toggleInline('**word**', { from: 2, to: 6 }, '*')).text).toBe('***word***');
    expect(apply('***word***', toggleInline('***word***', { from: 3, to: 7 }, '*')).text).toBe('**word**');
    expect(apply('***word***', toggleInline('***word***', { from: 3, to: 7 }, '**')).text).toBe('*word*');
  });

  it('toggles highlight and comment markers', () => {
    expect(apply('a word b', toggleInline('a word b', { from: 2, to: 6 }, '==')).text).toBe('a ==word== b');
    expect(apply('a ==word== b', toggleInline('a ==word== b', { from: 4, to: 8 }, '==')).text).toBe('a word b');
    expect(apply('a word b', toggleInline('a word b', { from: 2, to: 6 }, '%%'))).toEqual({ text: 'a %%word%% b', selected: 'word' });
    expect(apply('a %%word%% b', toggleInline('a %%word%% b', { from: 2, to: 10 }, '%%')).text).toBe('a word b');
  });

  it('puts the caret between empty markers', () => {
    const edit = toggleInline('ab', { from: 1, to: 1 }, '`');
    expect(apply('ab', edit).text).toBe('a``b');
    expect(edit.selection).toEqual({ from: 2, to: 2 });
  });
});

describe('clearInline', () => {
  it('strips markers inside and around the selection', () => {
    const source = 'x **bold *it* `c`** y';
    expect(apply(source, clearInline(source, { from: 4, to: 17 })).text).toBe('x bold it c y');
  });

  it('strips highlight and comment markers', () => {
    const source = 'x ==mark %%note%%== y';
    expect(apply(source, clearInline(source, { from: 4, to: 17 })).text).toBe('x mark note y');
    expect(apply('%%c%%', clearInline('%%c%%', { from: 2, to: 3 })).text).toBe('c');
  });

  it('leaves block markers alone', () => {
    const source = '# **Title**';
    expect(apply(source, clearInline(source, { from: 0, to: source.length })).text).toBe('# Title');
  });
});

describe('convertBlock', () => {
  it('replaces the existing marker of every touched line', () => {
    const source = '# one\n- [ ] two\nthree';
    expect(apply(source, convertBlock(source, { from: 2, to: 13 }, 'numbered')).text).toBe('1. one\n2. two\nthree');
  });

  it('turns a heading back into a paragraph and keeps indentation', () => {
    expect(apply('  > quoted', convertBlock('  > quoted', { from: 5, to: 5 }, 'paragraph')).text).toBe('  quoted');
    expect(apply('### h', convertBlock('### h', { from: 4, to: 4 }, 'heading1')).text).toBe('# h');
  });

  it('fences lines into a code block with the caret on the code', () => {
    const edit = convertBlock('a\ncode\nb', { from: 3, to: 3 }, 'codeBlock');
    expect(apply('a\ncode\nb', edit).text).toBe('a\n```\ncode\n```\nb');
    expect(edit.selection.from).toBe(10);
  });
});

describe('links', () => {
  it('builds an external link around the selection with the caret in the URL', () => {
    const edit = externalLink('see docs', { from: 4, to: 8 });
    expect(apply('see docs', edit).text).toBe('see [docs]()');
    expect(edit.selection.from).toBe(11);
  });

  it('opens the picker trigger without a selection', () => {
    expect(apply('a', internalLink('a', { from: 1, to: 1 })).text).toBe('a[[');
    expect(apply('a b', internalLink('a b', { from: 2, to: 3 })).text).toBe('a [[b]]');
  });
});
