import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleSelect } from './LocaleSelect';

it('offers exactly Simplified Chinese, English and Spanish', () => {
  const html = renderToStaticMarkup(<LocaleSelect id="language" label="Language" value="en" onChange={() => undefined} />);
  expect([...html.matchAll(/<option value="([^"]+)"/g)].map((match) => match[1])).toEqual(['zh-CN', 'en', 'es']);
  expect(html).toContain('简体中文');
  expect(html).toContain('English');
  expect(html).toContain('Español');
});
