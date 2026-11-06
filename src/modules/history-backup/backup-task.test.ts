import { describe, expect, it, vi } from 'vitest';
import { monitorBackupTask } from './backup-task';
import type { BackupJob, BackupService } from './ports';
import type { ServiceError } from '../domain';

const error: ServiceError = { code: 'IO_DENIED', messageKey: 'error.connection', params: {}, retryable: true, operationId: null, retryAfterSeconds: null };
const running: BackupJob = { jobId: 'job', kind: 'create', status: 'running', cancelRequested: false, progress: { phase: 'copying', completedObjects: 1, totalObjects: 3 }, result: null, error: null };
const service = (): BackupService => ({ context: vi.fn(), chooseDestination: vi.fn(), setRetention: vi.fn(), start: vi.fn(), preview: vi.fn(), previewOverwrite: vi.fn(), previewUpgrade: vi.fn(), status: vi.fn(), cancel: vi.fn() });

describe('backup task lifetime', () => {
  it('accepts the authoritative terminal context when status polling is unavailable', async () => {
    const port = service(); const views: BackupJob[] = [];
    vi.mocked(port.status).mockResolvedValue({ ok: false, error });
    vi.mocked(port.context).mockResolvedValue({ ok: true, value: { selection: null, savedDestination: null, configurationError: null, job: { ...running, status: 'cancelled' } } });
    let polls = 0;
    const outcome = await monitorBackupTask(port, running, job => views.push(job), () => {}, async () => { expect(++polls).toBe(1); });
    expect(outcome.ok && outcome.value.status).toBe('cancelled');
    expect(views.map(job => job.status)).toEqual(['running', 'cancelled']);
    expect(port.status).toHaveBeenCalledTimes(1);
  });
  it('does not release a running task on a transient status failure', async () => {
    const port = service(); const views: BackupJob[] = []; const failed = vi.fn();
    vi.mocked(port.status).mockResolvedValueOnce({ ok: false, error }).mockResolvedValueOnce({ ok: true, value: { ...running, status: 'succeeded' } });
    vi.mocked(port.context).mockResolvedValueOnce({ ok: true, value: { selection: null, savedDestination: null, configurationError: null, job: running } });
    const outcome = await monitorBackupTask(port, running, job => views.push(job), failed, async () => {});
    expect(outcome.ok && outcome.value.status).toBe('succeeded'); expect(views.map(job => job.status)).toEqual(['running', 'succeeded']); expect(failed).toHaveBeenCalledWith(error);
  });
  it('ends only after the host confirms the old handle is gone or terminal', async () => {
    const port = service(); vi.mocked(port.status).mockResolvedValueOnce({ ok: false, error });
    vi.mocked(port.context).mockResolvedValueOnce({ ok: true, value: { selection: null, savedDestination: null, configurationError: null, job: null } });
    expect(await monitorBackupTask(port, running, () => {}, () => {}, async () => {})).toEqual({ ok: false, error });
    expect((await monitorBackupTask(port, { ...running, status: 'cancelled' }, () => {}, () => {})).ok).toBe(true);
    expect(port.status).toHaveBeenCalledTimes(1);
  });
});
