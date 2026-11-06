import { expect, it } from 'vitest';
import { CheckpointController } from './checkpoint-controller';
import type { CheckpointRequest, CheckpointResult, ServiceOutcome } from './ports';

it('runs one checkpoint per note and retries the exact failed request before newer edits', async () => {
  const calls: CheckpointRequest[] = [];
  const pending: ((value: ServiceOutcome<CheckpointResult>) => void)[] = [];
  let id = 0;
  const controller = new CheckpointController({ writeCheckpoint(request) {
    calls.push(request);
    return new Promise(resolve => pending.push(resolve));
  } }, () => `op-${++id}`);
  const input = { noteId: 'n', relativePath: 'n.md', source: 'first', baseRevision: 1, baseContentHash: 'h1' };
  const first = controller.write(input);
  expect(await controller.write({ ...input, source: 'second' })).toBeNull();
  expect(calls).toHaveLength(1);
  pending[0]!({ ok: false, error: { code: 'FILE_LOCKED', messageKey: 'locked', params: {}, retryable: true, operationId: null, retryAfterSeconds: null } });
  await first;
  const retry = controller.write({ ...input, source: 'second', baseRevision: 2, baseContentHash: 'h2' });
  expect(calls[1]).toEqual(calls[0]);
  pending[1]!({ ok: true, value: { noteId: 'n', draftId: 'op-1', savedAt: 'now', sequence: 1, sourceHash: 'hash' } });
  await retry;
  const newer = controller.write({ ...input, source: 'second', baseRevision: 2, baseContentHash: 'h2' });
  expect(calls[2]).toMatchObject({ source: 'second', operationId: 'op-2', baseRevision: 2, baseContentHash: 'h2' });
  pending[2]!({ ok: true, value: { noteId: 'n', draftId: 'op-2', savedAt: 'now', sequence: 2, sourceHash: 'hash2' } });
  await newer;
  expect(await controller.write({ ...input, source: 'second', baseRevision: 2, baseContentHash: 'h2' })).toBeNull();
});
