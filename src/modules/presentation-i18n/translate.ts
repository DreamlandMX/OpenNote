import type { SupportedLocaleId } from './locales';
import type { MessageKey, MessageParams, MessageResource } from './messages';
import { formatMessageArgs, lookupMessage, RESOURCE_NOT_TRANSLATED } from './messages';
import { en } from './resources/en';
import { zh_CN } from './resources/zh-CN';
import { es } from './resources/es';

export const RESOURCE_MAP: Readonly<Record<SupportedLocaleId, MessageResource>> = {
  en,
  'zh-CN': zh_CN,
  es,
};

export type Translator = (key: MessageKey, params?: MessageParams) => string;

export type LocaleResourceLoader = (locale: SupportedLocaleId) => Promise<MessageResource>;

const ENGLISH_KEYS = Object.keys(en);

export async function loadLocaleResource(
  locale: SupportedLocaleId,
  loader: LocaleResourceLoader = async (requested) => RESOURCE_MAP[requested],
): Promise<boolean> {
  try {
    const resource = await loader(locale);
    return ENGLISH_KEYS.every((key) => {
      const message = resource[key as MessageKey];
      return typeof message === 'string' && message.length > 0;
    });
  } catch {
    return false;
  }
}

export class LocaleContext {
  constructor(private localeId: SupportedLocaleId) {}

  get locale(): SupportedLocaleId {
    return this.localeId;
  }

  get translator(): Translator {
    return (key, params) => this.t(key, params);
  }

  t(key: MessageKey, params?: MessageParams): string {
    const found = lookupMessage(RESOURCE_MAP, this.localeId, key);
    if (found === RESOURCE_NOT_TRANSLATED) {
      return formatMessageArgs(RESOURCE_MAP.en['dialog.error.fallback'], params);
    }
    return formatMessageArgs(found as string, params);
  }
}

export function translate(locale: SupportedLocaleId, key: MessageKey, params?: MessageParams): string {
  return new LocaleContext(locale).t(key, params);
}

export type { MessageKey, MessageParams, MessageResource } from './messages';
