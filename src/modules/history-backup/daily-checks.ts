const POLL_MS = 60_000;
const RETRY_MS = 300_000;

export function startDailyBackupChecks(canRun: () => boolean, poll: () => Promise<boolean | 'deferred'>, nextDueAt: () => number | null = () => null) {
  let stopped = false;
  let running = false;
  let nextPoll = 0;
  let lastWallClock = Date.now();
  let clockInvalidated = false;
  function schedule(delay: number) {
    if (stopped) return;
    clearTimeout(timer); timer = setTimeout(() => void check(), delay);
  }
  async function check(): Promise<void> {
    if (stopped || running) return;
    const remaining = nextPoll - performance.now();
    if (remaining > 0) { schedule(remaining); return; }
    const now = Date.now();
    if (now < lastWallClock) clockInvalidated = true;
    lastWallClock = now;
    if (!canRun()) { schedule(POLL_MS); return; }
    const due = nextDueAt();
    if (!clockInvalidated && due !== null && now < due * 1000) { schedule(POLL_MS); return; }
    clearTimeout(timer);
    running = true;
    let result: boolean | 'deferred' = false;
    try { result = await poll(); }
    catch { result = false; }
    finally {
      running = false;
      if (result === true) clockInvalidated = false;
      const delay = result === false ? RETRY_MS : POLL_MS;
      nextPoll = performance.now() + delay; schedule(delay);
    }
  }
  let timer = setTimeout(() => void check(), 1000);
  return {
    check,
    stop() { stopped = true; clearTimeout(timer); },
  };
}
