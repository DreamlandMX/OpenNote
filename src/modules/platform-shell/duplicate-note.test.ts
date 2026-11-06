import { describe, expect, it } from 'vitest';
import { duplicatePath } from './duplicate-note';

describe('duplicatePath', () => {
  it('inserts the suffix before the extension', () => {
    expect(duplicatePath('journal/Note.md', [], 'copy')).toBe('journal/Note copy.md');
    expect(duplicatePath('笔记.md', [], '副本')).toBe('笔记 副本.md');
  });

  it('numbers later copies until a free path exists', () => {
    expect(
      duplicatePath('Note.md', ['Note copy.md', 'Note copy 2.md'], 'copy'),
    ).toBe('Note copy 3.md');
  });

  it('treats Windows case collisions as taken', () => {
    expect(duplicatePath('Note.md', ['NOTE COPY.MD'], 'copy')).toBe('Note copy 2.md');
  });

  it('ignores dots in folders and notes without an extension', () => {
    expect(duplicatePath('a.b/Note', [], 'copy')).toBe('a.b/Note copy');
    expect(duplicatePath('a.b/Note.md', [], 'copy')).toBe('a.b/Note copy.md');
    expect(duplicatePath('.hidden', [], 'copy')).toBe('.hidden copy');
  });

  it('falls back to a literal label when the locale string is blank', () => {
    expect(duplicatePath('Note.md', [], '  ')).toBe('Note copy.md');
  });
});
