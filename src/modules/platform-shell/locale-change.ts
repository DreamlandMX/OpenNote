import {
  loadLocaleResource,
  selfNameForLocale,
  translate,
  type LocaleResourceLoader,
  type SupportedLocaleId,
} from '../presentation-i18n';

export interface LocaleChangePort {
  currentLocale(): SupportedLocaleId;
  activate(locale: SupportedLocaleId): void;
  persist(locale: SupportedLocaleId): void;
  announce(message: string): void;
}

export type LocaleChangeResult = 'changed' | 'kept' | 'superseded';

/**
 * Commits a language change only after its whole resource set loads.
 * A newer request wins, and a failure leaves the active locale and its
 * device preference untouched.
 */
export async function changeLocale(
  requested: SupportedLocaleId,
  sequence: number,
  latestSequence: () => number,
  port: LocaleChangePort,
  loader?: LocaleResourceLoader,
): Promise<LocaleChangeResult> {
  const loaded = await loadLocaleResource(requested, loader);
  if (sequence !== latestSequence()) {
    return 'superseded';
  }
  if (!loaded) {
    port.announce(translate(port.currentLocale(), 'onboarding.locale.failed'));
    return 'kept';
  }
  port.activate(requested);
  port.persist(requested);
  port.announce(
    translate(requested, 'a11y.settings.languageChanged', {
      name: selfNameForLocale(requested),
    }),
  );
  return 'changed';
}
