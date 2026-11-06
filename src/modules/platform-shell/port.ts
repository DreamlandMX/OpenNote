import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ErrorCode, ServiceError, Vault } from '../domain';

export type ShellPortKind = 'disconnected' | 'tauri';

export type ShellCallResult<T> = { ok: true; value: T } | { ok: false; error: ServiceError };

export interface VaultDialogOptions {
  selectionId?: string;
  displayName?: string;
  dialogTitle?: string;
  sampleOptIn?: boolean;
}

export interface ChosenVaultFolder { selectionId: string; displayPath: string }

export type CloseRequestListener = () => void | Promise<void>;

export type CloseRequestUnlisten = () => Promise<void>;

export interface ShellPort {
  chooseVaultFolder?(dialogTitle: string): Promise<ShellCallResult<ChosenVaultFolder | null>>;
  readonly kind: ShellPortKind;
  createVault(options?: VaultDialogOptions): Promise<ShellCallResult<Vault | null>>;
  openVault(options?: VaultDialogOptions): Promise<ShellCallResult<Vault | null>>;
  /** Reopens the previous session's vault without a dialog; `null` when there is none. */
  reopenLastVault?(): Promise<ShellCallResult<Vault | null>>;
  onCloseRequested(listener: CloseRequestListener): CloseRequestUnlisten;
  confirmClose(): Promise<ShellCallResult<void>>;
}

const CLOSE_REQUESTED_EVENT = 'opennote://close-requested';

const GENERIC_SERVICE_ERROR_KEY = 'opennote.service.error';

const KNOWN_ERROR_CODES: readonly ErrorCode[] = [
  'INVALID_INPUT',
  'UNSUPPORTED_SCHEMA',
  'OBJECT_NOT_FOUND',
  'VERSION_CONFLICT',
  'IDEMPOTENCY_MISMATCH',
  'IO_DENIED',
  'DISK_FULL',
  'FILE_LOCKED',
  'RESCAN_REQUIRED',
  'NOT_IMPLEMENTED',
  'HISTORY_MISSING',
  'PATH_COLLISION',
  'OPERATION_CANCELLED',
];

function isKnownErrorCode(value: unknown): value is ErrorCode {
  return typeof value === 'string' && (KNOWN_ERROR_CODES as readonly string[]).includes(value);
}

function unknownFailure(): ServiceError {
  return {
    code: 'IO_DENIED',
    messageKey: GENERIC_SERVICE_ERROR_KEY,
    params: {},
    retryable: false,
    operationId: null,
    retryAfterSeconds: null,
  };
}

export function serviceErrorFromThrown(thrown: unknown): ServiceError {
  if (
    thrown !== null &&
    typeof thrown === 'object' &&
    isKnownErrorCode((thrown as { code: unknown }).code)
  ) {
    const candidate = thrown as {
      code: ErrorCode;
      messageKey?: unknown;
      params?: unknown;
      retryable?: unknown;
      operationId?: unknown;
      retryAfterSeconds?: unknown;
    };
    const params: Record<string, string> = {};
    if (
      candidate.params !== null &&
      typeof candidate.params === 'object' &&
      !Array.isArray(candidate.params)
    ) {
      for (const [key, value] of Object.entries(candidate.params as Record<string, unknown>)) {
        if (typeof value === 'string') {
          params[key] = value;
        }
      }
    }
    return {
      code: candidate.code,
      messageKey:
        typeof candidate.messageKey === 'string' ? candidate.messageKey : GENERIC_SERVICE_ERROR_KEY,
      params,
      retryable: candidate.retryable === true,
      operationId: typeof candidate.operationId === 'string' ? candidate.operationId : null,
      retryAfterSeconds:
        typeof candidate.retryAfterSeconds === 'number' && Number.isFinite(candidate.retryAfterSeconds)
          ? candidate.retryAfterSeconds
          : null,
    };
  }
  return unknownFailure();
}

function invokeVaultPicker(
  command: 'create_vault' | 'open_vault',
  options?: VaultDialogOptions,
): Promise<ShellCallResult<Vault | null>> {
  const args: Record<string, unknown> = {};
  if (command === 'create_vault' && options?.selectionId) {
    args.selectionId = options.selectionId;
    args.displayName = options.displayName;
  }
  const dialogTitle = options?.dialogTitle;
  if (dialogTitle !== undefined) {
    args.dialogTitle = dialogTitle;
  }
  if (command === 'create_vault' && options?.sampleOptIn !== undefined) {
    args.sampleOptIn = options.sampleOptIn;
  }
  return invoke<Vault | null>(command, args).then(
    (value) => ({ ok: true as const, value }),
    (thrown: unknown) => ({ ok: false as const, error: serviceErrorFromThrown(thrown) }),
  );
}

function closeRequestUnlisten(listener: CloseRequestListener): CloseRequestUnlisten {
  let unlisten: UnlistenFn | null = null;

  function detach(): Promise<void> {
    const current = unlisten;
    unlisten = null;
    if (current === null) {
      return Promise.resolve();
    }
    return Promise.resolve()
      .then(() => current())
      .then(
        () => undefined,
        () => undefined,
      );
  }

  const registration = listen(CLOSE_REQUESTED_EVENT, () => {
    try {
      void Promise.resolve(listener()).catch(() => undefined);
    } catch {
      return;
    }
  });
  registration.then(
    (detachFn) => {
      unlisten = detachFn;
    },
    () => undefined,
  );

  return () =>
    registration
      .then(
        () => detach(),
        () => undefined,
      )
      .then(() => undefined);
}

const disconnectedFailure: ServiceError = {
  code: 'NOT_IMPLEMENTED',
  messageKey: '',
  params: {},
  retryable: false,
  operationId: null,
  retryAfterSeconds: null,
};

export const disconnectedShellPort: ShellPort = {
  kind: 'disconnected',
  createVault() {
    return Promise.resolve({ ok: false as const, error: disconnectedFailure });
  },
  openVault() {
    return Promise.resolve({ ok: false as const, error: disconnectedFailure });
  },
  onCloseRequested() {
    return () => Promise.resolve();
  },
  confirmClose() {
    return Promise.resolve({ ok: false as const, error: disconnectedFailure });
  },
};

export function createTauriShellPort(): ShellPort | null {
  const internals = (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  if (internals === undefined) {
    return null;
  }
  return {
    kind: 'tauri',
    chooseVaultFolder(dialogTitle) {
      return invoke<ChosenVaultFolder | null>('choose_vault_folder', { dialogTitle }).then(
        value => ({ ok: true as const, value }),
        error => ({ ok: false as const, error: serviceErrorFromThrown(error) }),
      );
    },
    createVault(options?: VaultDialogOptions): Promise<ShellCallResult<Vault | null>> {
      return invokeVaultPicker('create_vault', options);
    },
    openVault(options?: VaultDialogOptions): Promise<ShellCallResult<Vault | null>> {
      return invokeVaultPicker('open_vault', options);
    },
    reopenLastVault(): Promise<ShellCallResult<Vault | null>> {
      return invoke<Vault | null>('reopen_last_vault', {}).then(
        (value) => ({ ok: true as const, value }),
        (thrown: unknown) => ({ ok: false as const, error: serviceErrorFromThrown(thrown) }),
      );
    },
    onCloseRequested(listener: CloseRequestListener): CloseRequestUnlisten {
      return closeRequestUnlisten(listener);
    },
    confirmClose(): Promise<ShellCallResult<void>> {
      return invoke('confirm_close', {}).then(
        () => ({ ok: true as const, value: undefined }),
        (thrown: unknown) => ({ ok: false as const, error: serviceErrorFromThrown(thrown) }),
      );
    },
  };
}

export function resolveShellPort(injected: ShellPort | null | undefined): ShellPort {
  if (injected) {
    return injected;
  }
  return createTauriShellPort() ?? disconnectedShellPort;
}
