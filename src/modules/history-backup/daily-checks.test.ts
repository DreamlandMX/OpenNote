import { afterEach, expect, it, vi } from 'vitest';
import { startDailyBackupChecks } from './daily-checks';
afterEach(() => vi.useRealTimers());

it('avoids host calls before the persisted deadline and checks at the deadline without a render', async () => {
  vi.useFakeTimers(); vi.setSystemTime(0);
  let deadline = 3600;
  const poll = vi.fn(async () => { deadline += 3600; return true; });
  const loop = startDailyBackupChecks(() => true, poll, () => deadline);
  await vi.advanceTimersByTimeAsync(3_599_000); expect(poll).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(61_000); expect(poll).toHaveBeenCalledTimes(1);
  loop.stop();
});

it('keeps a clock rollback pending while the editor blocks a check', async () => {
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-10-18T12:00:00Z'));
  let eligible = false;
  const due = Date.now() / 1000 + 86400;
  const poll = vi.fn().mockResolvedValueOnce('deferred').mockResolvedValue(true);
  const loop = startDailyBackupChecks(() => eligible, poll, () => due);
  await vi.advanceTimersByTimeAsync(1000);
  vi.setSystemTime(new Date('2025-10-18T12:00:00Z'));
  await vi.advanceTimersByTimeAsync(60_000); expect(poll).not.toHaveBeenCalled();
  eligible = true;
  await vi.advanceTimersByTimeAsync(60_000); expect(poll).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(60_000); expect(poll).toHaveBeenCalledTimes(2);
  await vi.advanceTimersByTimeAsync(60_000); expect(poll).toHaveBeenCalledTimes(2);
  loop.stop();
});

it('keeps polling when the wall clock moves backwards', async () => {
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-10-18T12:00:00Z'));
  const poll = vi.fn().mockResolvedValue(true); const loop = startDailyBackupChecks(() => true, poll);
  await vi.advanceTimersByTimeAsync(1000); expect(poll).toHaveBeenCalledTimes(1);
  vi.setSystemTime(new Date('2025-10-18T12:00:00Z'));
  await vi.advanceTimersByTimeAsync(60_000); expect(poll).toHaveBeenCalledTimes(2);
  loop.stop();
});

it('defers dirty or busy work without resetting the interval and never overlaps polls', async () => {
  vi.useFakeTimers(); let eligible = false; let finish!: (value: boolean) => void;
  const poll = vi.fn(() => new Promise<boolean>(resolve => { finish = resolve; }));
  const loop = startDailyBackupChecks(() => eligible, poll);
  await vi.advanceTimersByTimeAsync(60_000); expect(poll).not.toHaveBeenCalled();
  eligible = true; await vi.advanceTimersByTimeAsync(60_000); expect(poll).toHaveBeenCalledTimes(1);
  await loop.check(); await vi.advanceTimersByTimeAsync(120_000); expect(poll).toHaveBeenCalledTimes(1);
  finish(true); await Promise.resolve(); await vi.advanceTimersByTimeAsync(60_000); expect(poll).toHaveBeenCalledTimes(2);
  loop.stop(); finish(true);
});

it('backs off failures and does not let focus-triggered checks bypass the delay', async () => {
  vi.useFakeTimers(); const poll = vi.fn().mockResolvedValueOnce(false).mockResolvedValue(true);
  const loop = startDailyBackupChecks(() => true, poll);
  await vi.advanceTimersByTimeAsync(1000); expect(poll).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(299_999); await loop.check(); expect(poll).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1); await loop.check(); expect(poll).toHaveBeenCalledTimes(2);
  loop.stop(); await vi.advanceTimersByTimeAsync(600_000); expect(poll).toHaveBeenCalledTimes(2);
});

it('stops polling after disposal even if the previous request resolves later', async () => {
  vi.useFakeTimers(); let finish!: (value: boolean) => void;
  const poll = vi.fn(() => new Promise<boolean>(resolve => { finish = resolve; }));
  const loop = startDailyBackupChecks(() => true, poll);
  await vi.advanceTimersByTimeAsync(1000); loop.stop(); finish(true);
  await vi.advanceTimersByTimeAsync(600_000); await loop.check(); expect(poll).toHaveBeenCalledTimes(1);
});
