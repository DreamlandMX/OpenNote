import { describe, expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { findPhrases } from './find-replace';

describe('find and replace wording', () => {
  it('covers every word the search panel asks for', () => {
    // Taken from @codemirror/search 6.7.2; a version bump should re-check this list.
    const asked = ['Find', 'Replace', 'next', 'previous', 'all', 'match case', 'by word', 'regexp', 'replace', 'replace all', 'close', 'current match', 'on line', 'replaced $ matches', 'replaced match on line $', 'Go to line', 'go'];
    const phrases = findPhrases(new LocaleContext('en').translator);
    expect(asked.filter((word) => phrases[word] === undefined)).toEqual([]);
  });

  it('keeps the placeholders CodeMirror fills in', () => {
    for (const locale of ['en', 'zh-CN', 'es'] as const) {
      const phrases = findPhrases(new LocaleContext(locale).translator);
      expect(phrases['replaced $ matches']).toContain('$');
      expect(phrases['replaced match on line $']).toContain('$');
    }
  });
});
