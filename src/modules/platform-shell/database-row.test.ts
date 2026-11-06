import { describe, expect, it } from 'vitest';
import { newRowPath } from './database-row';

describe('database row creation path', () => {
  it('places the row in the scoped folder and sanitises the title', () => {
    expect(newRowPath('Projects/2026/', 'Q3/Q4 plan?')).toEqual({
      ok: true,
      path: 'Projects/2026/Q3-Q4 plan-.md',
      name: 'Q3-Q4 plan-',
    });
  });

  it('normalises Windows separators and whitespace around the folder', () => {
    expect(newRowPath('  Projects\\2026  ', '  Kickoff  ')).toEqual({
      ok: true,
      path: 'Projects/2026/Kickoff.md',
      name: 'Kickoff',
    });
  });

  it('creates at the vault root when the folder scope is empty', () => {
    expect(newRowPath('', 'Ideas')).toEqual({ ok: true, path: 'Ideas.md', name: 'Ideas' });
    expect(newRowPath('   ', 'Ideas')).toEqual({ ok: true, path: 'Ideas.md', name: 'Ideas' });
  });

  it('requires a title instead of inventing an untitled row', () => {
    expect(newRowPath('notes', '')).toEqual({ ok: false, reason: 'title-required' });
    expect(newRowPath('notes', '   ')).toEqual({ ok: false, reason: 'title-required' });
  });

  it.each([
    '/absolute',
    '\\absolute',
    'C:\\notes',
    'c:notes',
    'notes/../outside',
    'notes/./inside',
    'notes//nested',
    '.opennote',
    'notes/.opennote',
    'notes/name:stream',
    'notes/trailing.',
    'notes/trailing /sub',
    'notes/bad*name',
  ])('refuses a folder scope the host would reject: %s', (folder) => {
    expect(newRowPath(folder, 'Note')).toEqual({ ok: false, reason: 'invalid-folder' });
  });

  it('keeps unicode titles intact', () => {
    expect(newRowPath('笔记/项目', '会议记录')).toEqual({
      ok: true,
      path: '笔记/项目/会议记录.md',
      name: '会议记录',
    });
  });
});
