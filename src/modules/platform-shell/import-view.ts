import type { ImportLoss, ImportStatus } from '../editor';
import type { MessageKey, Translator } from '../presentation-i18n';

const REASONS: Readonly<Record<string, MessageKey>> = {
  unsupportedType: 'import.loss.unsupportedType',
  link: 'import.loss.link',
  pathUnsafe: 'import.loss.pathUnsafe',
  tooLarge: 'import.loss.tooLarge',
  encoding: 'import.loss.encoding',
  encrypted: 'import.loss.encrypted',
  duplicate: 'import.loss.duplicate',
  deadLink: 'import.loss.deadLink',
  unreadable: 'import.loss.unreadable',
  archiveInvalid: 'import.loss.archiveInvalid',
  'block.callout': 'import.loss.block',
  'block.toggle': 'import.loss.block',
  'block.table': 'import.loss.block',
  'block.embed': 'import.loss.block',
};

/** One readable line per loss. A reason this build does not know still shows its path. */
export function importLossLines(losses: readonly ImportLoss[], translator: Translator): string[] {
  return [...new Set(losses.map((loss) => {
    const key = REASONS[loss.reason];
    return key === undefined ? loss.path : `${loss.path}: ${translator(key)}`;
  }))];
}

const FAILURES: Readonly<Record<string, MessageKey>> = {
  'error.importSourceChanged': 'import.fail.changed',
  'error.noteAlreadyExists': 'import.fail.exists',
  'error.attachmentPathOccupied': 'import.fail.exists',
  'error.diskFull': 'import.fail.diskFull',
};

/** Why one item failed, in words. */
export function importFailureLine(path: string, key: string, translator: Translator): string {
  return `${path}: ${translator(FAILURES[key] ?? 'import.fail.other')}`;
}

const PREPARE_ERRORS: Readonly<Record<string, MessageKey>> = {
  'error.importArchiveInvalid': 'import.error.archive',
  'error.importEntryLimit': 'import.error.tooMany',
  'error.importExpandedLimit': 'import.error.tooLarge',
  'error.importPathInvalid': 'import.error.path',
  'error.importNothing': 'import.error.nothing',
};

/** A specific message for a failure while reading the source; null to use the general one. */
export function importErrorKey(messageKey: string): MessageKey | null {
  return PREPARE_ERRORS[messageKey] ?? null;
}

/** The conversion report as Markdown, for the user to keep (F24 §6). */
export function importReport(status: ImportStatus, translator: Translator): string {
  const lines = [
    `# ${translator('import.reportTitle', { name: status.sourceName })}`,
    '',
    translator('import.progress', { notes: status.notesDone, attachments: status.attachmentsDone, done: status.finished, total: status.total }),
    '',
  ];
  for (const [path, key] of status.failed) lines.push(`- ${importFailureLine(path, key, translator)}`);
  for (const line of importLossLines(status.losses, translator)) lines.push(`- ${line}`);
  return `${lines.join('\n')}\n`;
}
