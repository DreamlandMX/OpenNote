import type { MessageKey } from '../presentation-i18n';
import type { LatexEnvironmentStatus, LatexPrepareStage } from '../latex-workspace/ports';

/** Bytes as the user reads them (decimal units, one decimal place). */
export function formatBytes(bytes: number, locale: string): string {
  const units = ['B', 'KB', 'MB', 'GB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const digits = unit === 0 ? 0 : 1;
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: digits, minimumFractionDigits: digits }).format(value)} ${units[unit]}`;
}

export const STAGE_KEYS: Readonly<Record<LatexPrepareStage, MessageKey>> = {
  downloading: 'latexEnv.stage.downloading',
  verifying: 'latexEnv.stage.verifying',
  fetchingResources: 'latexEnv.stage.fetchingResources',
  checkingOffline: 'latexEnv.stage.checkingOffline',
  activating: 'latexEnv.stage.activating',
};

const REASON_KEYS: ReadonlySet<string> = new Set([
  'latexEnv.reason.cancelled',
  'latexEnv.reason.timedOut',
  'latexEnv.reason.tooLarge',
  'latexEnv.reason.download',
  'latexEnv.reason.engineMismatch',
  'latexEnv.reason.engine',
  'latexEnv.reason.sandbox',
  'latexEnv.reason.io',
  'latexEnv.invalid.unreadable',
  'latexEnv.invalid.engine',
  'latexEnv.invalid.files',
]);

/** A host reason key the UI knows, or a generic one; never shows a raw key. */
export function reasonKey(key: string | null | undefined): MessageKey {
  return key !== null && key !== undefined && REASON_KEYS.has(key)
    ? (key as MessageKey)
    : 'latexEnv.reason.unknown';
}

export type EnvironmentView = 'loading' | 'preparing' | 'ready' | 'notReady';

export function environmentView(status: LatexEnvironmentStatus | null): EnvironmentView {
  if (status === null) return 'loading';
  if (status.preparing !== null) return 'preparing';
  return status.environment.kind === 'ready' ? 'ready' : 'notReady';
}
