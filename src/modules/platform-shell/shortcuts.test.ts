import { describe, expect, it } from 'vitest';
import {
  applyBinding,
  chordMatcher,
  isModifierKey,
  bindChord,
  chordKey,
  chordText,
  effectiveBinding,
  chordFromEvent,
  matchesCommand,
  resolveCommand,
  resolveStep,
  MAX_CHORD_STEPS,
  parseChord,
  parseBindings,
  resetBinding,
  validateChord,
  type BindableCommandId,
  type Chord,
} from './shortcuts';

function chord(...steps: string[]): Chord {
  const parsed = parseChord(steps.join(' '));
  if (parsed === null) {
    throw new Error(`bad test chord: ${steps.join(' ')}`);
  }
  return parsed;
}

describe('chord text and comparison', () => {
  it('normalizes modifier order and key case', () => {
    expect(chordText(chord('Shift+Ctrl+f'))).toBe('Ctrl+Shift+F');
    expect(chordKey(chord('Shift+Ctrl+f'))).toBe('ctrl+shift+f');
    expect(chordKey(chord('ctrl+shift+F'))).toBe(chordKey(chord('Shift+Ctrl+f')));
  });

  it('renders a two-step chord with a space', () => {
    expect(chordText(chord('Ctrl+K', 'Ctrl+S'))).toBe('Ctrl+K Ctrl+S');
  });

  it('parses aliases and rejects unknown modifiers', () => {
    expect(parseChord('Control+Alt+Delete')).not.toBeNull();
    expect(parseChord('Cmd+P')).not.toBeNull();
    expect(parseChord('Hyper+P')).toBeNull();
    expect(parseChord('')).toBeNull();
  });
});

describe('chord validation', () => {
  it('requires at least one step', () => {
    expect(validateChord([])).toEqual({ code: 'empty' });
  });

  it('allows at most two steps', () => {
    expect(validateChord(chord('Ctrl+K', 'Ctrl+S'))).toBeNull();
    const three = [...chord('Ctrl+K', 'Ctrl+S'), { modifiers: ['ctrl'] as const, key: 't' }];
    expect(validateChord(three)).toEqual({ code: 'tooManySteps', limit: MAX_CHORD_STEPS });
    expect(MAX_CHORD_STEPS).toBe(2);
  });

  it('refuses a system-reserved combination instead of remapping it', () => {
    for (const reserved of ['Alt+F4', 'Ctrl+W', 'Meta+L', 'F5']) {
      expect(validateChord(chord(reserved))).toEqual({
        code: 'systemReserved',
        chord: reserved.replace(/^(\w+)$/, (match) => match.toUpperCase()),
      });
    }
  });

  it('refuses a bare letter but allows a bare function key', () => {
    expect(validateChord(chord('k'))).toEqual({ code: 'noModifier', chord: 'K' });
    expect(validateChord(chord('F9'))).toBeNull();
  });

  it('rejects an empty step and a multi-character printable key', () => {
    expect(validateChord([{ modifiers: ['ctrl'], key: '  ' }])).toEqual({
      code: 'emptyStep',
      index: 0,
    });
    expect(validateChord([{ modifiers: ['ctrl'], key: 'ab!' }])).toEqual({
      code: 'multiCharacterKey',
      value: 'ab!',
    });
  });
});

describe('binding conflicts', () => {
  const existing: Partial<Record<BindableCommandId, Chord>> = {
    'workspace.quickOpen': chord('Ctrl+P'),
  };

  it('reports the other command and refuses without an explicit replace', () => {
    const result = bindChord(existing, 'workspace.commandPalette', chord('Ctrl+P'));
    expect(result.accepted).toBe(false);
    expect(result.error).toBeNull();
    expect(result.conflictWith).toBe('workspace.quickOpen');
  });

  it('cancelling a conflict leaves the original binding untouched', () => {
    const applied = applyBinding(existing, 'workspace.commandPalette', chord('Ctrl+P'));
    expect(applied.accepted).toBe(false);
    expect(applied.bindings).toEqual(existing);
    expect(effectiveBinding(applied.bindings, 'workspace.quickOpen')).toEqual(chord('Ctrl+P'));
  });

  it('replacing clears the previous owner instead of leaving two owners', () => {
    const applied = applyBinding(existing, 'workspace.commandPalette', chord('Ctrl+P'), true);
    expect(applied.accepted).toBe(true);
    expect(applied.result.conflictWith).toBe('workspace.quickOpen');
    expect(applied.bindings['workspace.commandPalette']).toEqual(chord('Ctrl+P'));
    expect(applied.bindings['workspace.quickOpen']).toEqual([]);
    expect(effectiveBinding(applied.bindings, 'workspace.quickOpen')).toBeNull();
  });

  it('a rejected chord never mutates the map', () => {
    const applied = applyBinding(existing, 'workspace.search', chord('Alt+F4'), true);
    expect(applied.accepted).toBe(false);
    expect(applied.result.error).toEqual({
      code: 'systemReserved',
      chord: 'Alt+F4',
    });
    expect(applied.bindings).toEqual(existing);
  });

  it('accepts an unbound combination and keeps other bindings', () => {
    const applied = applyBinding(existing, 'workspace.search', chord('Ctrl+Alt+F'));
    expect(applied.accepted).toBe(true);
    expect(applied.bindings['workspace.search']).toEqual(chord('Ctrl+Alt+F'));
    expect(applied.bindings['workspace.quickOpen']).toEqual(chord('Ctrl+P'));
  });

  it('a command may keep its own binding unchanged', () => {
    const applied = applyBinding(existing, 'workspace.quickOpen', chord('Ctrl+P'));
    expect(applied.accepted).toBe(true);
    expect(applied.result.conflictWith).toBeNull();
  });

  it('defaults are used when nothing is stored', () => {
    expect(effectiveBinding({}, 'workspace.search')).toEqual(chord('Ctrl+Shift+F'));
    expect(effectiveBinding({}, 'workspace.import')).toBeNull();
  });

  it('detects conflicts with defaults even without stored overrides', () => {
    const applied = applyBinding({}, 'workspace.search', chord('Ctrl+P'));
    expect(applied.accepted).toBe(false);
    expect(applied.result.conflictWith).toBe('workspace.quickOpen');
    expect(applied.bindings).toEqual({});
  });

  it('restoring a default requires confirmation when another command owns it', () => {
    const replaced = applyBinding({}, 'workspace.search', chord('Ctrl+P'), true).bindings;
    const cancelled = resetBinding(replaced, 'workspace.quickOpen');
    expect(cancelled.accepted).toBe(false);
    expect(cancelled.result.conflictWith).toBe('workspace.search');
    expect(cancelled.bindings).toEqual(replaced);
    const restored = resetBinding(replaced, 'workspace.quickOpen', true);
    expect(restored.accepted).toBe(true);
    expect(restored.bindings['workspace.quickOpen']).toBeUndefined();
    expect(effectiveBinding(restored.bindings, 'workspace.quickOpen')).toEqual(chord('Ctrl+P'));
    expect(effectiveBinding(restored.bindings, 'workspace.search')).toBeNull();
  });

  it('restores an available default and clears overrides for commands without defaults', () => {
    expect(resetBinding({ 'workspace.search': [] }, 'workspace.search').bindings).toEqual({});
    const restored = resetBinding({ 'workspace.import': chord('Ctrl+Alt+I') }, 'workspace.import');
    expect(restored.accepted).toBe(true);
    expect(restored.bindings).toEqual({});
  });

  it('round-trips an explicit unbinding and validates old preference text', () => {
    const replaced = applyBinding({}, 'workspace.commandPalette', chord('Ctrl+P'), true).bindings;
    const stored = Object.fromEntries(Object.entries(replaced).map(([id, binding]) => [id, chordText(binding!)]));
    const reopened = parseBindings(JSON.parse(JSON.stringify(stored)));
    expect(effectiveBinding(reopened, 'workspace.quickOpen')).toBeNull();
    expect(effectiveBinding(reopened, 'workspace.commandPalette')).toEqual(chord('Ctrl+P'));
    expect(parseBindings({ 'workspace.search': 'Ctrl+Alt+F', 'workspace.export': 'Alt+F4',
      'workspace.import': 'Hyper+I', bogus: 'Ctrl+Q' })).toEqual({ 'workspace.search': chord('Ctrl+Alt+F') });
    expect(resolveCommand(reopened, { key: 'p', ctrlKey: true, altKey: false, shiftKey: false, metaKey: false }))
      .toEqual({ kind: 'command', command: 'workspace.commandPalette' });
  });
});

describe('event resolution', () => {
  function press(key: string, modifiers: Partial<{
    isComposing: boolean;
    ctrlKey: boolean;
    shiftKey: boolean;
    altKey: boolean;
    metaKey: boolean;
  }> = {}): Parameters<typeof resolveCommand>[1] {
    return {
      key,
      isComposing: modifiers.isComposing ?? false,
      ctrlKey: modifiers.ctrlKey ?? false,
      shiftKey: modifiers.shiftKey ?? false,
      altKey: modifiers.altKey ?? false,
      metaKey: modifiers.metaKey ?? false,
    };
  }

  it('resolves the default bindings', () => {
    expect(resolveCommand({}, press('F', { ctrlKey: true, shiftKey: true }))).toEqual({
      kind: 'command',
      command: 'workspace.search',
    });
    expect(resolveCommand({}, press('p', { ctrlKey: true }))).toEqual({
      kind: 'command',
      command: 'workspace.quickOpen',
    });
    expect(resolveCommand({}, press('P', { ctrlKey: true, shiftKey: true }))).toEqual({
      kind: 'command',
      command: 'workspace.commandPalette',
    });
  });

  it('a stored override changes which keys fire', () => {
    const bindings = { 'workspace.search': chord('Ctrl+Alt+F') };
    expect(resolveCommand(bindings, press('f', { ctrlKey: true, shiftKey: true }))).toEqual({
      kind: 'none',
    });
    expect(resolveCommand(bindings, press('f', { ctrlKey: true, altKey: true }))).toEqual({
      kind: 'command',
      command: 'workspace.search',
    });
  });

  it('never runs a command for a system-reserved press', () => {
    expect(resolveCommand({}, press('F4', { altKey: true }))).toEqual({
      kind: 'reserved',
      chord: 'Alt+F4',
    });
    expect(resolveCommand({}, press('w', { ctrlKey: true }))).toEqual({
      kind: 'reserved',
      chord: 'Ctrl+W',
    });
  });

  it('ignores an unrelated key press', () => {
    expect(resolveCommand({}, press('q', { ctrlKey: true }))).toEqual({ kind: 'none' });
    expect(resolveCommand({}, press('Escape'))).toEqual({ kind: 'none' });
  });

  it('opens rename with F2 and respects an explicit unbinding', () => {
    expect(resolveCommand({}, press('F2'))).toEqual({ kind: 'command', command: 'workspace.rename' });
    expect(resolveCommand({ 'workspace.rename': [] }, press('F2'))).toEqual({ kind: 'none' });
    expect(resolveCommand({}, press('F2', { isComposing: true }))).toEqual({ kind: 'none' });
  });

  it('a press with no modifier never matches a modified binding', () => {
    expect(resolveCommand({}, press('f'))).toEqual({ kind: 'none' });
  });

  it('matchesCommand agrees with resolveCommand for one command', () => {
    const bindings = { 'workspace.export': chord('Ctrl+Shift+E') };
    const event = press('e', { ctrlKey: true, shiftKey: true });
    expect(matchesCommand(bindings, 'workspace.export', event)).toBe(true);
    expect(matchesCommand(bindings, 'workspace.search', event)).toBe(false);
    expect(matchesCommand(bindings, 'workspace.import', event)).toBe(false);
  });

  it('builds a canonical chord from an event', () => {
    expect(chordKey(chordFromEvent(press('F', { shiftKey: true, ctrlKey: true })))).toBe(
      'ctrl+shift+f',
    );
  });
});

describe('two-step dispatch', () => {
  const twoStep = { 'workspace.export': chord('Ctrl+K', 'Ctrl+E') } as Partial<
    Record<BindableCommandId, Chord>
  >;
  function press(key: string, modifiers: Partial<{ ctrlKey: boolean; shiftKey: boolean }> = {}) {
    return {
      key,
      ctrlKey: modifiers.ctrlKey ?? false,
      shiftKey: modifiers.shiftKey ?? false,
      altKey: false,
      metaKey: false,
    };
  }

  it('arms the prefix and completes on the second step', () => {
    const armed = resolveStep(twoStep, press('k', { ctrlKey: true }), null);
    expect(armed).toEqual({
      kind: 'prefix',
      prefix: { key: 'ctrl+k', command: 'workspace.export' },
    });
    const done = resolveStep(
      twoStep,
      press('e', { ctrlKey: true }),
      armed.kind === 'prefix' ? armed.prefix : null,
    );
    expect(done).toEqual({ kind: 'command', command: 'workspace.export' });
  });

  it('dispatches a replaced two-step binding to a command without a default', () => {
    const replaced = applyBinding(twoStep, 'workspace.import', chord('Ctrl+K', 'Ctrl+E'), true).bindings;
    expect(effectiveBinding(replaced, 'workspace.export')).toBeNull();
    const first = resolveStep(replaced, press('k', { ctrlKey: true }), null);
    expect(first.kind).toBe('prefix');
    expect(resolveStep(replaced, press('e', { ctrlKey: true }), first.kind === 'prefix' ? first.prefix : null))
      .toEqual({ kind: 'command', command: 'workspace.import' });
  });

  it('pressing the prefix again cancels instead of repeating', () => {
    const armed = { key: 'ctrl+k', command: 'workspace.export' as const };
    expect(resolveStep(twoStep, press('k', { ctrlKey: true }), armed)).toEqual({ kind: 'none' });
  });

  it('an unrelated press cancels the prefix and is evaluated normally', () => {
    const armed = { key: 'ctrl+k', command: 'workspace.export' as const };
    // Ctrl+P is a single-step command; it must still fire.
    expect(resolveStep(twoStep, press('p', { ctrlKey: true }), armed)).toEqual({
      kind: 'command',
      command: 'workspace.quickOpen',
    });
    expect(resolveStep(twoStep, press('q', { ctrlKey: true }), armed)).toEqual({ kind: 'none' });
  });

  it('a single-step binding is never treated as a prefix', () => {
    expect(resolveStep({}, press('f', { ctrlKey: true, shiftKey: true }), null)).toEqual({
      kind: 'command',
      command: 'workspace.search',
    });
  });

  it('a reserved second step does not complete a prefix', () => {
    const bindings = { 'workspace.export': chord('Ctrl+K', 'Ctrl+W') } as Partial<
      Record<BindableCommandId, Chord>
    >;
    // Ctrl+W is reserved, so the chord can never be completed.
    const step = resolveStep(bindings, press('w', { ctrlKey: true }), {
      key: 'ctrl+k',
      command: 'workspace.export',
    });
    expect(step).toEqual({ kind: 'reserved', chord: 'Ctrl+W' });
  });
});

describe('chordMatcher for editor commands', () => {
  const press = (key: string, ctrl = false, shift = false) => ({ key, ctrlKey: ctrl, shiftKey: shift, altKey: false, metaKey: false });

  it('fires on the current binding, read at press time', () => {
    let binding: Chord | null = [{ modifiers: ['ctrl'], key: 'f' }];
    const match = chordMatcher(() => binding);
    expect(match(press('f', true))).toBe('complete');
    expect(match(press('F', true, true))).toBe('none');
    binding = [{ modifiers: ['ctrl', 'shift'], key: 'g' }];
    expect(match(press('f', true))).toBe('none');
    expect(match(press('G', true, true))).toBe('complete');
    binding = null;
    expect(match(press('G', true, true))).toBe('none');
  });

  it('arms on the first step of a two-step chord and ignores bare modifier presses', () => {
    let time = 0;
    const match = chordMatcher(() => [{ modifiers: ['ctrl'], key: 'k' }, { modifiers: ['ctrl'], key: 'f' }], () => time);
    expect(match(press('f', true))).toBe('none');
    expect(match(press('k', true))).toBe('armed');
    expect(match(press('Control', true))).toBe('none');
    expect(match(press('f', true))).toBe('complete');
    expect(match(press('k', true))).toBe('armed');
    time = 5000;
    expect(match(press('f', true))).toBe('none');
  });

  it('cancels an armed chord on an unrelated key', () => {
    const match = chordMatcher(() => [{ modifiers: ['ctrl'], key: 'k' }, { modifiers: ['ctrl'], key: 'f' }]);
    expect(match(press('k', true))).toBe('armed');
    expect(match(press('x'))).toBe('none');
    expect(match(press('f', true))).toBe('none');
  });

  it('leaves Ctrl+F and Ctrl+H to the editor rather than a workspace surface', () => {
    expect(resolveStep({}, press('f', true), null)).toEqual({ kind: 'command', command: 'editor.find' });
    expect(resolveStep({}, press('h', true), null)).toEqual({ kind: 'command', command: 'editor.replace' });
  });
});

describe('isModifierKey', () => {
  it('treats a lone Ctrl, Shift, Alt or Windows key as the start of a chord, not a chord', () => {
    for (const key of ['Control', 'Shift', 'Alt', 'Meta', 'AltGraph', 'OS']) expect(isModifierKey(key)).toBe(true);
    for (const key of ['f', 'F2', 'Enter', 'Escape']) expect(isModifierKey(key)).toBe(false);
  });
});
