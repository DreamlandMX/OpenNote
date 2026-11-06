import type { FormEvent } from 'react';
import { ArrowRight, Leaf } from 'lucide-react';
import type { Translator } from '../presentation-i18n';
import type { SupportedLocaleId } from '../presentation-i18n';
import { LocaleSelect } from './LocaleSelect';

export interface WelcomeScreenProps {
  translator: Translator;
  locale: SupportedLocaleId;
  onLocaleChange: (locale: SupportedLocaleId) => void | Promise<void>;
  onContinue: () => void;
  onOpenExisting?: () => void;
}

export function WelcomeScreen({ translator, locale, onLocaleChange, onContinue, onOpenExisting }: WelcomeScreenProps) {
  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onContinue();
  }

  return (
    <div className="welcome content-panel" lang={locale}>
      <img
        className="welcome-art"
        src="/welcome-room-to-think.png"
        width={1095}
        height={1437}
        alt=""
        aria-hidden="true"
        draggable={false}
      />
      <div className="welcome-copy">
      <p className="welcome-brand"><Leaf size={22} aria-hidden="true" />{translator('shell.brand.name')}</p>
      <h1>{translator('onboarding.welcome.title')}</h1>
      <p className="tagline">{translator('onboarding.welcome.tagline')}</p>
      <form onSubmit={handleSubmit}>
        <button type="submit" className="primary">
          {translator('onboarding.welcome.continue')}
          <ArrowRight size={16} aria-hidden="true" />
        </button>
      </form>
      {onOpenExisting ? <button type="button" className="welcome-open" onClick={onOpenExisting}>{translator('home.open.title')}</button> : null}
      <p className="summary">{translator('onboarding.welcome.summary')}</p>
      </div>
      <div className="welcome-language"><LocaleSelect id="welcome-locale" label={translator('onboarding.welcome.language')} value={locale} onChange={onLocaleChange} /></div>
    </div>
  );
}
