import { describe, expect, it } from 'vitest';
import { AUTO_BUILD_IDLE_MS, nextAutoBuild, type AutoBuildState } from './latex-autobuild';

const base: AutoBuildState = { enabled: true, changeSeq: 1, builtSeq: 0, lastEditAt: 1000, building: false };

describe('automatic LaTeX build', () => {
  it('waits for typing to pause, then builds', () => {
    expect(nextAutoBuild(base, 1000)).toEqual({ kind: 'wait', ms: AUTO_BUILD_IDLE_MS });
    expect(nextAutoBuild(base, 1000 + 400)).toEqual({ kind: 'wait', ms: AUTO_BUILD_IDLE_MS - 400 });
    expect(nextAutoBuild(base, 1000 + AUTO_BUILD_IDLE_MS)).toEqual({ kind: 'build' });
  });

  it('does nothing when everything typed is already built, or when switched off', () => {
    expect(nextAutoBuild({ ...base, builtSeq: 1 }, 99_999)).toEqual({ kind: 'idle' });
    expect(nextAutoBuild({ ...base, enabled: false }, 99_999)).toEqual({ kind: 'idle' });
  });

  it('never interrupts a running build; edits during it lead to one more build afterwards', () => {
    // Build started at change 1, then three more edits arrived while it ran.
    const during: AutoBuildState = { ...base, changeSeq: 4, builtSeq: 1, building: true, lastEditAt: 5000 };
    expect(nextAutoBuild(during, 9000)).toEqual({ kind: 'queued' });
    // It ended: one build covers all three edits.
    expect(nextAutoBuild({ ...during, building: false }, 9000)).toEqual({ kind: 'build' });
    // That build started at change 4; with no new edit nothing else follows.
    expect(nextAutoBuild({ ...during, building: false, builtSeq: 4 }, 20_000)).toEqual({ kind: 'idle' });
  });

  it('a running build with nothing new to cover stays idle', () => {
    expect(nextAutoBuild({ ...base, builtSeq: 1, building: true }, 5000)).toEqual({ kind: 'idle' });
  });
});
