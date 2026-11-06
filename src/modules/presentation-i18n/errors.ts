import type { ErrorCode } from '../domain';
import { translate, type MessageKey } from './translate';
import type { SupportedLocaleId } from './locales';

const SERVICE_ERROR_KEYS: Record<ErrorCode, MessageKey> = {
  INVALID_INPUT: 'errors.INVALID_INPUT',
  UNSUPPORTED_SCHEMA: 'errors.UNSUPPORTED_SCHEMA',
  OBJECT_NOT_FOUND: 'errors.OBJECT_NOT_FOUND',
  VERSION_CONFLICT: 'errors.VERSION_CONFLICT',
  IDEMPOTENCY_MISMATCH: 'errors.IDEMPOTENCY_MISMATCH',
  IO_DENIED: 'errors.IO_DENIED',
  DISK_FULL: 'errors.DISK_FULL',
  FILE_LOCKED: 'errors.FILE_LOCKED',
  RESCAN_REQUIRED: 'errors.RESCAN_REQUIRED',
  NOT_IMPLEMENTED: 'errors.NOT_IMPLEMENTED',
  HISTORY_MISSING: 'errors.HISTORY_MISSING',
  PATH_COLLISION: 'errors.PATH_COLLISION',
  BACKUP_INCOMPLETE: 'errors.BACKUP_INCOMPLETE',
  RESTORE_INTERRUPTED: 'errors.RESTORE_INTERRUPTED',
  OPERATION_CANCELLED: 'errors.OPERATION_CANCELLED',
};

export function serviceErrorMessage(locale: SupportedLocaleId, code: ErrorCode, messageKey?: string): string {
  if (code === 'INVALID_INPUT' && messageKey === 'error.vaultAlreadyInitialized') {
    return translate(locale, 'errors.vaultAlreadyInitialized');
  }
  return translate(locale, SERVICE_ERROR_KEYS[code]);
}

export function saveFailureMessages(locale: SupportedLocaleId, code: ErrorCode): string[] {
  const reason = serviceErrorMessage(locale, code);
  // These localized reasons already include the retained-text and recovery guidance.
  return code === 'IO_DENIED' || code === 'DISK_FULL'
    ? [reason]
    : [reason, translate(locale, 'editor.save.failed.body')];
}
