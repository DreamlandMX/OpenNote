import { afterEach, expect, it, vi } from 'vitest';
import type { ParsedHeading, ServiceOutcome } from '../editor/ports';
import { currentOutline, observeOutline, type OutlineSnapshot } from './observe-outline';

const source = '# Alpha\n## Beta';
const headings: readonly ParsedHeading[] = [
  { text: 'Alpha', slug: 'alpha', level: 1, line: 0, offset: 0 },
  { text: 'Beta', slug: 'beta', level: 2, line: 1, offset: 8 },
];
const success: ServiceOutcome<readonly ParsedHeading[]> = { ok: true, value: headings };
const failure: ServiceOutcome<readonly ParsedHeading[]> = {
  ok: false,
  error: { code: 'IO_DENIED', messageKey: 'test', params: {}, retryable: true, operationId: null, retryAfterSeconds: null },
};
afterEach(() => vi.useRealTimers());

it('does not use old headings after editing even when the old line still exists', () => {
  const snapshot: OutlineSnapshot = { noteId: 'a', source, outcome: success };
  expect(currentOutline(snapshot, 'a', '# Alpha\nordinary text')).toBeNull();
  expect(currentOutline(snapshot, 'a', source)).toEqual(success);
});

it('rejects another note with identical source and an old error for a different source', () => {
  expect(currentOutline({ noteId: 'a', source, outcome: success }, 'b', source)).toBeNull();
  expect(currentOutline({ noteId: 'a', source, outcome: failure }, 'a', '# New')).toBeNull();
  expect(currentOutline(null, 'a', source)).toBeNull();
});

it('debounces the actual parse and publishes the source and note identity with the result', async () => {
  vi.useFakeTimers();
  const published: OutlineSnapshot[] = [];
  const inputs: string[] = [];
  const stop = observeOutline({ parseHeadings: async input => { inputs.push(input); return success; } }, 'a', source, value => published.push(value));
  await vi.advanceTimersByTimeAsync(149);
  expect(inputs).toEqual([]);
  await vi.advanceTimersByTimeAsync(1);
  expect(inputs).toEqual([source]);
  expect(published).toEqual([{ noteId: 'a', source, outcome: success }]);
  stop();
});

it.each([success, failure])('drops an in-flight result after close, source change or note switch', async outcome => {
  vi.useFakeTimers();
  let resolve!: (value: ServiceOutcome<readonly ParsedHeading[]>) => void;
  const pending = new Promise<ServiceOutcome<readonly ParsedHeading[]>>(done => { resolve = done; });
  const published: OutlineSnapshot[] = [];
  const stop = observeOutline({ parseHeadings: () => pending }, 'a', source, value => published.push(value));
  await vi.advanceTimersByTimeAsync(150);
  stop();
  resolve(outcome);
  await vi.advanceTimersByTimeAsync(0);
  expect(published).toEqual([]);
});

it('does not parse when stopped during debounce', async () => {
  vi.useFakeTimers();
  const inputs: string[] = [];
  const stop = observeOutline({ parseHeadings: async input => { inputs.push(input); return success; } }, 'a', source, () => {});
  stop();
  await vi.advanceTimersByTimeAsync(150);
  expect(inputs).toEqual([]);
});

it('binds a real parse failure to its source without retaining old successful headings', async () => {
  vi.useFakeTimers();
  const published: OutlineSnapshot[] = [];
  observeOutline({ parseHeadings: async () => failure }, 'a', source, value => published.push(value));
  await vi.advanceTimersByTimeAsync(150);
  expect(currentOutline(published[0]!, 'a', source)).toEqual(failure);
});

it.each(['throw', 'reject'])('reports an unexpected parser %s as a recoverable UI error', async kind => {
  vi.useFakeTimers();
  const published: OutlineSnapshot[] = [];
  observeOutline({ parseHeadings: () => {
    if (kind === 'throw') throw new Error('failed');
    return Promise.reject(new Error('failed'));
  } }, 'a', source, value => published.push(value));
  await vi.advanceTimersByTimeAsync(150);
  expect(published).toMatchObject([{ noteId: 'a', source, outcome: { ok: false, error: { code: 'IO_DENIED' } } }]);
});
