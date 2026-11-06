import { describe, expect, it } from 'vitest';
import {
  DEFAULT_LOCALE,
  dirForLocale,
  matchLocaleTag,
  resolveLocaleFromTags,
  SUPPORTED_LOCALES,
  type SupportedLocaleId,
} from './locales';
import { loadLocaleResource, RESOURCE_MAP } from './translate';
import { en } from './resources/en';
import { formatMessageArgs } from './messages';

const EXPECTED_LOCALES: readonly SupportedLocaleId[] = ['zh-CN', 'en', 'es'];

describe('locale resolution', () => {
  it('falls back to English for unsupported or malformed input', () => {
    expect(DEFAULT_LOCALE).toBe('en');
    expect(matchLocaleTag('xx-XY')).toBeNull();
    expect(matchLocaleTag('')).toBeNull();
    expect(matchLocaleTag('en-')).toBeNull();
    expect(resolveLocaleFromTags(['xx-XY', 'zz-Bogus', ''])).toBe('en');
    expect(resolveLocaleFromTags([])).toBe('en');
  });

  it('maps supported variants to the right interface locale', () => {
    expect(matchLocaleTag('en-US')).toBe('en');
    expect(matchLocaleTag('en-GB')).toBe('en');
    expect(matchLocaleTag('es-419')).toBe('es');
    expect(matchLocaleTag('es-ES')).toBe('es');
    expect(matchLocaleTag('es-MX')).toBe('es');
  });

  it('applies the documented script and region rules for Chinese', () => {
    expect(matchLocaleTag('zh-CN')).toBe('zh-CN');
    expect(matchLocaleTag('zh-SG')).toBe('zh-CN');
    expect(matchLocaleTag('zh-Hans')).toBe('zh-CN');
    expect(matchLocaleTag('zh-Hans-HK')).toBe('zh-CN');
    expect(matchLocaleTag('zh-TW')).toBe('zh-CN');
    expect(matchLocaleTag('zh-HK')).toBe('zh-CN');
    expect(matchLocaleTag('zh-Hant')).toBe('zh-CN');
    expect(matchLocaleTag('zh')).toBe('zh-CN');
  });

  it('skips removed languages and uses only the three supported interfaces', () => {
    expect(resolveLocaleFromTags(['de-AT', 'zh-Hant'])).toBe('zh-CN');
    expect(resolveLocaleFromTags(['ar', 'es-MX', 'en'])).toBe('es');
    expect(resolveLocaleFromTags(['ja', 'de', 'ar'])).toBe('en');
    for (const tag of ['ja', 'de', 'ar', 'fr', 'pt-BR', 'ko', 'it', 'ru', 'hi', 'id', 'tr', 'vi']) {
      expect(matchLocaleTag(tag)).toBeNull();
    }
    expect(resolveLocaleFromTags(['sv-SE', 'zh-Hant'])).toBe('zh-CN');
    for (const { id } of SUPPORTED_LOCALES) expect(dirForLocale(id)).toBe('ltr');
  });
});

const PLACEHOLDER_PATTERN = /\{(\w+)\}/g;

function extractPlaceholders(template: string): string[] {
  return [...template.matchAll(PLACEHOLDER_PATTERN)].map((match) => match[1] as string).sort();
}

describe('message resources', () => {
  it('covers every approved interface locale and matches the English key set', () => {
    expect(SUPPORTED_LOCALES.map((l) => l.id)).toEqual(EXPECTED_LOCALES);
    expect(Object.keys(RESOURCE_MAP).sort()).toEqual([...EXPECTED_LOCALES].sort());
    const enKeys = Object.keys(en).sort();
    for (const locale of SUPPORTED_LOCALES) {
      const resource = RESOURCE_MAP[locale.id];
      expect(Object.keys(resource).sort()).toEqual(enKeys);
    }
  });

  it('keeps placeholder names identical to English in every locale', () => {
    for (const key of Object.keys(en) as (keyof typeof en)[]) {
      const englishPlaceholders = extractPlaceholders(en[key]);
      const messages = Object.entries(RESOURCE_MAP)
        .map(([locale, resource]) => [locale, resource[key]] as const)
        .filter(([, template]) => template !== undefined);
      for (const [locale, template] of messages) {
        expect(template).toBeTruthy();
        expect([locale, extractPlaceholders(template as string)]).toEqual([
          locale,
          englishPlaceholders,
        ]);
        expect(template !== undefined).toBe(true);
        // With no parameters supplied, formatting must leave the template
        // untouched; iterating the formatted string character-by-character
        // was quadratic and timed out as the key set grew.
        expect([locale, formatMessageArgs(template as string, {})]).toEqual([
          locale,
          template,
        ]);
      }
    }
  });

  it('formats named parameters without inventing values', () => {
    const localized = RESOURCE_MAP.en['a11y.settings.languageChanged'];
    expect(formatMessageArgs(localized, { name: 'English' })).toBe('Language changed to English');
  });

  it('keeps the current locale when a requested pack cannot load', async () => {
    await expect(loadLocaleResource('es', async () => {
      throw new Error('unavailable');
    })).resolves.toBe(false);
    await expect(loadLocaleResource('es', async () => {
      throw 'offline';
    })).resolves.toBe(false);
    await expect(loadLocaleResource('es', async () => ({
      ...RESOURCE_MAP.es,
      'onboarding.locale.failed': '',
    }))).resolves.toBe(false);
    await expect(loadLocaleResource('es')).resolves.toBe(true);
  });
});
