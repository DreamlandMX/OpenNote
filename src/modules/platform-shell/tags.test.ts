import { describe, expect, it } from 'vitest';
import type { TagReport } from '../editor';
import {
  filterTags,
  hasMissingNotes,
  MAX_TAG_LENGTH,
  normalizeTag,
  notesForTag,
  tagRows,
  tagSummary,
  validateTag,
} from './tags';

const report: TagReport = {
  tags: [
    {
      tag: '学习',
      noteIds: ['n1', 'n2'],
      notePaths: ['学习/构建个人知识库.md', '学习/阅读/构建阅读习惯.md'],
    },
    {
      tag: 'project',
      noteIds: ['n3'],
      notePaths: ['项目/构建项目索引.md'],
    },
    {
      tag: 'empty',
      noteIds: [],
      notePaths: [],
    },
  ],
  unreadable: [],
};

describe('tag browsing model', () => {
  it('keeps ids and paths aligned and drops an entry with no notes', () => {
    const rows = tagRows(report);
    expect(rows.map((row) => row.tag)).toEqual(['project', '学习']);
    expect(rows[1]?.notes).toEqual([
      { noteId: 'n1', relativePath: '学习/构建个人知识库.md' },
      { noteId: 'n2', relativePath: '学习/阅读/构建阅读习惯.md' },
    ]);
    expect(rows[1]?.count).toBe(2);
  });

  it('accepts a leading # in a filter and matches case-insensitively', () => {
    const rows = tagRows(report);
    expect(filterTags(rows, '#学习').map((row) => row.tag)).toEqual(['学习']);
    expect(filterTags(rows, 'PRO').map((row) => row.tag)).toEqual(['project']);
    expect(filterTags(rows, '')).toHaveLength(2);
    expect(filterTags(rows, 'missing')).toEqual([]);
  });

  it('narrows the notes of one tag by path without changing the tag list', () => {
    const rows = tagRows(report);
    const studying = rows.find((row) => row.tag === '学习') ?? null;
    expect(notesForTag(studying, '').map((note) => note.noteId)).toEqual(['n1', 'n2']);
    expect(notesForTag(studying, '阅读').map((note) => note.noteId)).toEqual(['n2']);
    expect(notesForTag(null, '')).toEqual([]);
  });

  it('summarizes distinct notes rather than counting each tag separately', () => {
    expect(tagSummary(tagRows(report))).toEqual({ tags: 2, notes: 3 });
    expect(tagSummary([])).toEqual({ tags: 0, notes: 0 });
  });

  it('validates the 1–64 character rule after trimming and keeps display case', () => {
    expect(validateTag('  学习  ')).toEqual({ ok: true, value: '学习', reason: null });
    expect(validateTag('   ')).toEqual({ ok: false, value: '', reason: 'empty' });
    expect(validateTag('x'.repeat(MAX_TAG_LENGTH)).ok).toBe(true);
    expect(validateTag('x'.repeat(MAX_TAG_LENGTH + 1))).toEqual({
      ok: false,
      value: 'x'.repeat(MAX_TAG_LENGTH + 1),
      reason: 'tooLong',
    });
    expect(validateTag('  Project  ').value).toBe('Project');
  });

  it('counts astral characters as one character in the limit', () => {
    const emoji = '😀'.repeat(MAX_TAG_LENGTH);
    expect(validateTag(emoji).ok).toBe(true);
    expect(validateTag(`${emoji}😀`).reason).toBe('tooLong');
  });

  it('normalizes a hash prefix without touching the rest of the tag', () => {
    expect(normalizeTag('  #学习 ')).toBe('学习');
    expect(normalizeTag('学习')).toBe('学习');
    expect(normalizeTag('#')).toBe('');
  });

  it('flags a report that references a note the tree no longer has', () => {
    const rows = tagRows(report);
    expect(hasMissingNotes(rows, ['n1', 'n2', 'n3'])).toBe(false);
    expect(hasMissingNotes(rows, ['n1', 'n2'])).toBe(true);
  });
});
