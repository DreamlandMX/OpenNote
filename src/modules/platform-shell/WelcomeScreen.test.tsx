import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { WelcomeScreen } from './WelcomeScreen';

it('offers both onboarding and opening an existing vault with one language selector', () => {
  const translator = new LocaleContext('en').translator;
  const html = renderToStaticMarkup(<WelcomeScreen translator={translator} locale="en" onLocaleChange={() => undefined} onContinue={() => undefined} onOpenExisting={() => undefined} />);
  expect(html).toContain(translator('home.open.title'));
  expect(html).toContain(translator('onboarding.welcome.continue'));
  expect(html.match(/<select/g)).toHaveLength(1);
  expect(html.match(/<option/g)).toHaveLength(3);
});
