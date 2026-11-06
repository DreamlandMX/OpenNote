import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { HomeScreen } from './HomeScreen';

it('requires an explicit name and chosen location before creating a vault', () => {
  const translator = new LocaleContext('en').translator;
  const html = renderToStaticMarkup(<HomeScreen translator={translator} connected failureMessage={null} onBack={null} onVaultAction={async () => undefined} onChooseFolder={async () => null} />);
  expect(html).toContain(translator('home.create.name'));
  expect(html).toContain(translator('home.create.location'));
  expect(html).toContain('required=""');
  expect(html).toContain('readOnly=""');
  expect(html).toMatch(/type="submit"[^>]*disabled/);
  expect(html).toContain(translator('home.open.title'));
  expect(html).not.toContain('C:\\');
});

it('retains an actionable creation error alongside the form', () => {
  const translator = new LocaleContext('en').translator;
  const html = renderToStaticMarkup(<HomeScreen translator={translator} connected failureMessage="Folder is not writable" onBack={null} onVaultAction={async () => undefined} />);
  expect(html).toContain('role="alert"');
  expect(html).toContain('Folder is not writable');
  expect(html).toContain('type="submit"');
});
