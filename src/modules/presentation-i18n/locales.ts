export interface SupportedLocale {
  id: string;
  selfName: string;
  englishName: string;
  dir: 'ltr' | 'rtl';
}

export const SUPPORTED_LOCALES = [
  { id: 'zh-CN', selfName: '简体中文', englishName: 'Chinese (Simplified)', dir: 'ltr' },
  { id: 'en', selfName: 'English', englishName: 'English', dir: 'ltr' },
  { id: 'es', selfName: 'Español', englishName: 'Spanish', dir: 'ltr' },
] as const satisfies readonly SupportedLocale[];

export type SupportedLocaleId = (typeof SUPPORTED_LOCALES)[number]['id'];

export const DEFAULT_LOCALE: SupportedLocaleId = 'en';

export function isSupportedLocaleId(value: string | null | undefined): value is SupportedLocaleId {
  return typeof value === 'string' && SUPPORTED_LOCALES.some((l) => l.id === value);
}

export function getSupportedLocale(id: SupportedLocaleId): SupportedLocale {
  const found = SUPPORTED_LOCALES.find((l) => l.id === id);
  if (!found) {
    throw new Error(`Unknown supported locale id: ${id}`);
  }
  return found;
}

export function dirForLocale(id: SupportedLocaleId): 'ltr' | 'rtl' {
  return getSupportedLocale(id).dir;
}

export function selfNameForLocale(id: SupportedLocaleId): string {
  return getSupportedLocale(id).selfName;
}

interface TagParts {
  language: string;
  script: string | null;
  region: string | null;
}

function parseTagParts(tag: string): TagParts | null {
  const trimmed = tag.trim();
  if (trimmed.length === 0) {
    return null;
  }
  try {
    const locale = new Intl.Locale(trimmed);
    const language = locale.language.toLowerCase();
    if (!/^[a-z]{2,8}$/.test(language)) {
      return null;
    }
    const script = locale.script ? locale.script.toLowerCase() : null;
    const region = locale.region ? locale.region.toUpperCase() : null;
    return { language, script, region };
  } catch {
    return null;
  }
}

const SUPPORTED_LANGUAGES = new Set<string>(SUPPORTED_LOCALES.map((l) => l.id.slice(0, 2).toLowerCase()));

function matchSingleTagParts(parts: TagParts): SupportedLocaleId | null {
  const candidates = SUPPORTED_LOCALES;
  const exact = candidates.find(
    (l) => l.id.toLowerCase() === parts.language && !parts.script && !parts.region,
  );
  if (exact) {
    return exact.id;
  }
  if (parts.language === 'zh') {
    return 'zh-CN';
  }
  const languageMatches = candidates.filter(
    (l) => l.id.slice(0, 2).toLowerCase() === parts.language,
  );
  if (languageMatches.length === 0) {
    return null;
  }
  const exactRegion = parts.region
    ? languageMatches.find(
        (l) => parts.region !== null && l.id.toLowerCase() === `${parts.language}-${parts.region.toLowerCase()}`,
      )
    : undefined;
  if (exactRegion) {
    return exactRegion.id;
  }
  const firstMatch = languageMatches[0];
  return firstMatch ? firstMatch.id : null;
}

export function matchLocaleTag(tag: string | null | undefined): SupportedLocaleId | null {
  if (typeof tag !== 'string') {
    return null;
  }
  const parts = parseTagParts(tag);
  if (!parts) {
    return null;
  }
  if (!SUPPORTED_LANGUAGES.has(parts.language)) {
    return null;
  }
  return matchSingleTagParts(parts);
}

export function resolveLocaleFromTags(tags: readonly (string | null | undefined)[]): SupportedLocaleId {
  for (const tag of tags) {
    const matched = matchLocaleTag(tag);
    if (matched) {
      return matched;
    }
  }
  return DEFAULT_LOCALE;
}
