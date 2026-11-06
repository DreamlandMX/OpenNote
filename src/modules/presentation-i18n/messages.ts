import type { SupportedLocaleId } from './locales';
import { en } from './resources/en';

export type MessageKey = keyof typeof en;
export type MessageResource = Record<MessageKey, string>;

export type MessageParams = Readonly<Record<string, string | number>>;

export function formatMessageArgs(template: string, params: MessageParams | undefined): string {
  if (!params) {
    return template;
  }
  return template.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const param = params[name];
    return param === undefined ? whole : String(param);
  });
}

export const RESOURCE_NOT_TRANSLATED = Symbol('resource-not-translated');

export function lookupMessage(
  resources: Readonly<Record<SupportedLocaleId, MessageResource>>,
  locale: SupportedLocaleId,
  key: MessageKey,
): string | symbol {
  const inLocale = resources[locale][key];
  if (typeof inLocale === 'string' && inLocale.length > 0) {
    return inLocale;
  }
  const inEnglish = en[key];
  if (typeof inEnglish === 'string' && inEnglish.length > 0) {
    return inEnglish;
  }
  return RESOURCE_NOT_TRANSLATED;
}
