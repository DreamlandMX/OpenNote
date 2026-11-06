import { describe, expect, it } from 'vitest';
import { folderChoices } from './ExportPanel';

describe('export folder choices', () => {
  it('lists every folder prefix once, sorted', () => {
    expect(
      folderChoices([
        'a/b/one.md',
        'a/two.md',
        'z/three.md',
        'root.md',
      ]),
    ).toEqual(['a', 'a/b', 'z']);
  });

  it('normalizes Windows separators and ignores root notes', () => {
    expect(folderChoices(['notes\\deep\\a.md', 'plain.md'])).toEqual(['notes', 'notes/deep']);
  });

  it('returns nothing when every note sits at the vault root', () => {
    expect(folderChoices(['a.md', 'b.md'])).toEqual([]);
    expect(folderChoices([])).toEqual([]);
  });
});
