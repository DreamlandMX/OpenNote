import { describe, expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { importLossLines, importReport } from './import-view';

const translator = new LocaleContext('en').translator;

describe('import results', () => {
  it('says why each thing was lost, and repeats a line only once', () => {
    const lines = importLossLines([
      { path: 'a.exe', reason: 'unsupportedType' },
      { path: 'a.exe', reason: 'unsupportedType' },
      { path: 'gbk.md', reason: 'encoding' },
      { path: 'n.md', reason: 'block.callout' },
    ], translator);
    expect(lines).toHaveLength(3);
    expect(lines[0]).toBe(`a.exe: ${translator('import.loss.unsupportedType')}`);
    expect(lines[2]).toBe(`n.md: ${translator('import.loss.block')}`);
  });

  it('keeps the path of a reason this build does not know', () => {
    expect(importLossLines([{ path: 'x', reason: 'fromTheFuture' }], translator)).toEqual(['x']);
  });

  it('writes a report that can be kept as a note', () => {
    const report = importReport({
      batchId: 'b', state: 'partial', sourceName: 'e.zip', total: 3, finished: 3, notesDone: 1, attachmentsDone: 1,
      failed: [['a.md', 'error.importSourceChanged']],
      losses: [{ path: 'x.exe', reason: 'unsupportedType' }],
    }, translator);
    expect(report.startsWith('# ')).toBe(true);
    expect(report).toContain('e.zip');
    expect(report).toContain(`- a.md: ${translator('import.fail.changed')}`);
    expect(report).toContain('- x.exe: ');
  });

  it('has a heading for every state the host reports', () => {
    for (const state of ['running', 'interrupted', 'cancelled', 'complete', 'partial', 'preview']) {
      expect(translator(`import.state.${state}` as never)).not.toContain('import.state');
    }
  });
});
