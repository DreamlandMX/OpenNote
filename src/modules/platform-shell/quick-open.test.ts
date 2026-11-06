import { describe, expect, it } from 'vitest';
import {
  breadcrumbOf,
  quickOpenTitle,
  rankQuickOpen,
  scoreQuickOpen,
  truncateQuery,
  type QuickOpenCandidate,
} from './quick-open';

const notes: QuickOpenCandidate[] = [
  { noteId: 'n1', relativePath: '学习/知识管理/构建个人知识库.md', title: '构建个人知识库' },
  { noteId: 'n2', relativePath: '学习/阅读/构建阅读习惯.md', title: '构建阅读习惯' },
  { noteId: 'n3', relativePath: '项目/构建项目索引.md', title: '构建项目索引' },
  { noteId: 'n4', relativePath: '归档/构建.md', title: '构建' },
];

describe('quick open matching', () => {
  it('ranks an exact title first and keeps the remaining prefix matches', () => {
    const ranked = rankQuickOpen(notes, '构建');
    expect(ranked[0]?.noteId).toBe('n4');
    expect(new Set(ranked.slice(1).map((note) => note.noteId))).toEqual(
      new Set(['n1', 'n2', 'n3']),
    );
    expect(ranked.slice(1).every((note) => note.title.startsWith('构建'))).toBe(true);
  });

  it('ranks a title prefix above a title substring', () => {
    const mixed: QuickOpenCandidate[] = [
      { noteId: 'sub', relativePath: 'x/我的构建设计.md', title: '我的构建设计' },
      { noteId: 'pre', relativePath: 'y/构建方法.md', title: '构建方法' },
    ];
    expect(rankQuickOpen(mixed, '构建').map((note) => note.noteId)).toEqual(['pre', 'sub']);
  });

  it('finds a note by a path segment without changing the title', () => {
    expect(rankQuickOpen(notes, '知识管理').map((note) => note.noteId)).toEqual(['n1']);
    expect(notes[0]?.title).toBe('构建个人知识库');
  });

  it('returns no row for no match instead of inventing one', () => {
    expect(rankQuickOpen(notes, '不存在的标题')).toEqual([]);
  });

  it('keeps two same-titled notes distinguishable by path and id', () => {
    const duplicates: QuickOpenCandidate[] = [
      { noteId: 'b', relativePath: '甲/同名.md', title: '同名' },
      { noteId: 'a', relativePath: '乙/同名.md', title: '同名' },
    ];
    const ranked = rankQuickOpen(duplicates, '同名');
    expect(ranked.map((note) => note.relativePath)).toEqual(['乙/同名.md', '甲/同名.md']);
    expect(ranked.map((note) => note.title)).toEqual(['同名', '同名']);
  });

  it('matches case-insensitively and ignores surrounding whitespace', () => {
    const english: QuickOpenCandidate[] = [
      { noteId: 'e1', relativePath: 'Notes/Weekly Review.md', title: 'Weekly Review' },
    ];
    expect(scoreQuickOpen(english[0]!, '  weekly ')).toBe(1);
    expect(scoreQuickOpen(english[0]!, 'REVIEW')).toBe(2);
    expect(scoreQuickOpen(english[0]!, 'notes/')).toBe(3);
  });

  it('strips a markdown extension and builds a display breadcrumb', () => {
    expect(quickOpenTitle('学习/知识管理/构建个人知识库.md')).toBe('构建个人知识库');
    expect(quickOpenTitle('根笔记.MD')).toBe('根笔记');
    expect(breadcrumbOf('学习/知识管理/构建个人知识库.md')).toBe('学习 / 知识管理');
    expect(breadcrumbOf('根笔记.md')).toBe('');
  });

  it('caps the query at 200 characters without splitting an astral character', () => {
    const long = '😀'.repeat(205);
    const capped = truncateQuery(long);
    expect(capped.truncated).toBe(true);
    expect(Array.from(capped.value)).toHaveLength(200);
    expect(capped.value.endsWith('😀')).toBe(true);
    expect(truncateQuery('构建').truncated).toBe(false);
  });
});
