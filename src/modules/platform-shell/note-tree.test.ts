import { expect, it } from 'vitest';
import { groupNotes } from './NoteTree';
import type { NoteSummary } from '../domain';

it('groups nested paths without changing the original file order or confusing shared prefixes', () => {
  const notes = ['journal/day.md', 'journal-2026.md', 'journal/deep/idea.md'].map((relativePath, i) => ({ noteId: String(i), relativePath } as NoteSummary));
  const tree = groupNotes(notes);
  expect(tree.files.map(note => note.relativePath)).toEqual(['journal-2026.md']);
  expect(tree.folders[0]?.[0]).toBe('journal');
  expect(groupNotes(tree.folders[0]![1], 'journal/').folders[0]?.[0]).toBe('deep');
  expect(notes[0]?.relativePath).toBe('journal/day.md');
});
