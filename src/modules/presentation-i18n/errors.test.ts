import { describe, expect, it } from 'vitest';
import { saveFailureMessages, serviceErrorMessage } from './errors';
import { SUPPORTED_LOCALES } from './locales';
import { translate } from './translate';

describe('vault creation errors', () => {
  it('explains an existing vault in the three supported languages', () => {
    expect(serviceErrorMessage('en', 'INVALID_INPUT', 'error.vaultAlreadyInitialized')).toContain('Open a vault');
    expect(serviceErrorMessage('zh-CN', 'INVALID_INPUT', 'error.vaultAlreadyInitialized')).toContain('打开资料库');
    expect(serviceErrorMessage('es', 'INVALID_INPUT', 'error.vaultAlreadyInitialized')).toContain('Abrir una bóveda');
  });

  it('retains code-based messages for unknown keys and other error codes', () => {
    expect(serviceErrorMessage('en', 'INVALID_INPUT', 'untrusted.key')).toBe(serviceErrorMessage('en', 'INVALID_INPUT'));
    expect(serviceErrorMessage('en', 'IO_DENIED', 'error.vaultAlreadyInitialized')).toBe(serviceErrorMessage('en', 'IO_DENIED'));
  });
});

describe('save failure recovery guidance', () => {
  it.each(SUPPORTED_LOCALES)('avoids duplicate guidance in $id without losing other error reasons', ({ id }) => {
    expect(saveFailureMessages(id, 'IO_DENIED')).toEqual([serviceErrorMessage(id, 'IO_DENIED')]);
    expect(saveFailureMessages(id, 'DISK_FULL')).toEqual([serviceErrorMessage(id, 'DISK_FULL')]);
    expect(saveFailureMessages(id, 'FILE_LOCKED')).toEqual([
      serviceErrorMessage(id, 'FILE_LOCKED'), translate(id, 'editor.save.failed.body'),
    ]);
  });
});
