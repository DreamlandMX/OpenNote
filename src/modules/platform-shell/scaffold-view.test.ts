import { describe, expect, it } from 'vitest';
import type { ScaffoldPlan } from '../editor';
import {
  canCreate,
  defaultDestination,
  defaultMetadata,
  initialLanguage,
  issueKey,
  issueParams,
  metadataIssue,
  paperOptions,
  planSummary,
  PREVIEW_LIMIT,
  previewFiles,
} from './scaffold-view';

function plan(overrides: Partial<ScaffoldPlan> = {}): ScaffoldPlan {
  return {
    destination: 'proj',
    entryFile: 'main.tex',
    files: [
      { path: 'main.tex', content: '' },
      { path: 'references.bib', content: '' },
    ],
    issues: [],
    requires: ['geometry'],
    ...overrides,
  };
}

describe('metadata issues', () => {
  it('points at the field that needs fixing', () => {
    expect(metadataIssue({ kind: 'titleEmpty' })).toEqual({
      field: 'title',
      key: 'wizard.issue.titleEmpty',
    });
    expect(metadataIssue({ kind: 'titleTooLong', chars: 300, limit: 256 }).field).toBe('title');
    expect(metadataIssue({ kind: 'authorTooLong', chars: 200, limit: 128 }).field).toBe('author');
    expect(metadataIssue({ kind: 'invalidDate', value: 'x' }).field).toBe('date');
  });

  it('maps each plan issue to its message and params', () => {
    expect(
      issueKey({ kind: 'collision', path: 'proj/main.tex' }),
    ).toBe('wizard.issue.collision');
    expect(issueParams({ kind: 'collision', path: 'proj/main.tex' })).toEqual({
      path: 'proj/main.tex',
    });
    expect(issueKey({ kind: 'unsafeDestination', destination: '../x' })).toBe(
      'wizard.issue.unsafeDestination',
    );
    expect(issueKey({ kind: 'unknownTemplate', templateId: 'poster' })).toBe(
      'wizard.issue.unknownTemplate',
    );
    expect(
      issueKey({ kind: 'invalidMetadata', error: { kind: 'titleEmpty' } }),
    ).toBe('wizard.issue.titleEmpty');
  });
});

describe('document language', () => {
  it('follows the UI on first open only', () => {
    expect(initialLanguage('zh-CN', null)).toBe('zhCn');
    expect(initialLanguage('zh-TW', null)).toBe('zhCn');
    expect(initialLanguage('en', null)).toBe('en');
    expect(initialLanguage('ja', null)).toBe('en');
  });

  it('never replaces a language the user already chose', () => {
    // F46-R07: switching the interface later must not rewrite this.
    expect(initialLanguage('ja', 'zhCn')).toBe('zhCn');
    expect(initialLanguage('zh-CN', 'en')).toBe('en');
  });
});

describe('plan gating', () => {
  it('allows creation only with no issues and at least one file', () => {
    expect(canCreate(plan())).toBe(true);
    expect(
      canCreate(plan({ issues: [{ kind: 'collision', path: 'proj/main.tex' }] })),
    ).toBe(false);
    expect(canCreate(plan({ files: [] }))).toBe(false);
  });

  it('summarizes the confirmation step without claiming more than it knows', () => {
    expect(planSummary(plan())).toEqual({
      files: 2,
      requires: 1,
      executable: true,
      entry: 'main.tex',
    });
  });

  it('collapses a long file list and counts the rest', () => {
    const files = Array.from({ length: PREVIEW_LIMIT + 2 }, (_, index) => ({
      path: `f-${index}.tex`,
      content: '',
    }));
    const preview = previewFiles(plan({ files }));
    expect(preview.shown).toHaveLength(PREVIEW_LIMIT);
    expect(preview.hidden).toBe(2);
  });

  it('shows every file when the list is small', () => {
    expect(previewFiles(plan()).shown).toEqual(['main.tex', 'references.bib']);
  });
});

describe('wizard defaults', () => {
  it('derives a safe destination from the title', () => {
    expect(defaultDestination('Weekly Review')).toBe('weekly-review');
    expect(defaultDestination('a/b:c')).toBe('a-b-c');
    expect(defaultDestination('   ')).toBe('latex-project');
  });

  it('starts on A4 with an empty author and no title', () => {
    const metadata = defaultMetadata('en', '2026-10-20');
    expect(metadata.paper).toBe('a4');
    expect(metadata.title).toBe('');
    expect(metadata.author).toBe('');
    expect(metadata.date).toBe('2026-10-20');
  });

  it('offers two paper sizes and no others', () => {
    const options = paperOptions();
    expect(options.map((option) => option.value)).toEqual(['a4', 'letter']);
  });
});
