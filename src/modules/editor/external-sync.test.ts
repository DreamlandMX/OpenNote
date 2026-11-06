import { describe, expect, it } from 'vitest';
import { EditorState } from '@codemirror/state';
import { history, undo } from '@codemirror/commands';
import { externalChange, externalReplacement } from './external-sync';

describe('external document sync', () => {
  it('reports no change for an identical document', () => {
    expect(externalChange('same', 'same')).toBeNull();
  });

  it('keeps the prefix when text is appended', () => {
    expect(externalChange('title', 'title plus')).toEqual({
      from: 5,
      to: 5,
      insert: ' plus',
    });
  });

  it('replaces only the changed middle', () => {
    expect(externalChange('alpha beta gamma', 'alpha BETA gamma')).toEqual({
      from: 6,
      to: 10,
      insert: 'BETA',
    });
  });

  it('keeps the suffix when text is deleted', () => {
    expect(externalChange('alpha beta gamma', 'alpha gamma')).toEqual({
      from: 6,
      to: 11,
      insert: '',
    });
  });

  it('handles an insertion before and after identical text', () => {
    expect(externalChange('one two three', 'one new two three')).toEqual({
      from: 4,
      to: 4,
      insert: 'new ',
    });
  });

  it('replaces everything when nothing is shared', () => {
    expect(externalChange('old', 'new')).toEqual({ from: 0, to: 3, insert: 'new' });
  });

  it('keeps a mirrored external replacement out of this pane undo history', () => {
    // F07-R01: undoing local typing must not roll back the other pane's
    // already-committed edit.
    let state = EditorState.create({ doc: 'local', extensions: [history()] });
    state = state.update({ changes: { from: 5, insert: ' text' }, userEvent: 'input.type' }).state;
    expect(state.doc.toString()).toBe('local text');
    state = state.update(externalReplacement(externalChange('local text', 'external text')!)).state;
    expect(state.doc.toString()).toBe('external text');

    expect(
      undo({
        state,
        dispatch: (transaction) => {
          state = transaction.state;
        },
      }),
    ).toBe(true);
    expect(state.doc.toString()).toBe('external');
  });
});
