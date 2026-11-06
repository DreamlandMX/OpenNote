import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { NoteTitle } from './NoteTitle';

it('shows the file name without its folder or extension, labelled in each language', () => {
  for (const locale of ['en', 'zh-CN', 'es'] as const) {
    const translator = new LocaleContext(locale).translator;
    const html = renderToStaticMarkup(<NoteTitle
      relativePath="journal/Weekly review.md" editable focusToken={null}
      translator={translator} onRename={async () => true}
    />);
    expect(html).toContain('value="Weekly review"');
    expect(html).toContain(`aria-label="${translator('workspace.title.label')}"`);
    expect(html).not.toContain('readOnly');
  }
});

it('cannot be edited in a read-only note', () => {
  const translator = new LocaleContext('en').translator;
  const html = renderToStaticMarkup(<NoteTitle
    relativePath="a.md" editable={false} focusToken={null}
    translator={translator} onRename={async () => true}
  />);
  expect(html).toContain('readOnly=""');
});
