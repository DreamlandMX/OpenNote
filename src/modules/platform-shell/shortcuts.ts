/**
 * F30 shortcut bindings.
 *
 * Bindings are device state: they never change a note and never leak into the
 * vault (F30 §6). This module owns the two rules the UI must not re-implement:
 * a chord can hold at most two steps, and a combination reserved by the system
 * can never be taken over — a conflict must be explained, not silently resolved.
 */

import { isBindableCommandId, type BindableCommandId } from './command-catalogue';

export type { BindableCommandId } from './command-catalogue';

export type Modifier = 'ctrl' | 'shift' | 'alt' | 'meta';

/** One step of a chord: modifiers plus a key, e.g. `Ctrl+Shift+F`. */
export interface ChordStep {
  modifiers: readonly Modifier[];
  /** A single printable character or a named key such as `Tab`. */
  key: string;
}

/** At most two steps; an empty stored chord explicitly disables the binding. */
export type Chord = readonly ChordStep[];

export type ChordError =
  | { code: 'empty' }
  | { code: 'tooManySteps'; limit: number }
  | { code: 'emptyStep'; index: number }
  | { code: 'multiCharacterKey'; value: string }
  | { code: 'systemReserved'; chord: string }
  | { code: 'noModifier'; chord: string };

export interface BindingResult {
  /** True only when the chord may be stored. */
  accepted: boolean;
  error: ChordError | null;
  /** Other command currently using this chord, if any. */
  conflictWith: BindableCommandId | null;
  /** Canonical form, for storage and display. */
  chord: string;
}

export const MAX_CHORD_STEPS = 2;

const MODIFIER_ORDER: readonly Modifier[] = ['ctrl', 'alt', 'shift', 'meta'];

const MODIFIER_LABEL: Record<Modifier, string> = {
  ctrl: 'Ctrl',
  alt: 'Alt',
  shift: 'Shift',
  meta: 'Meta',
};

/**
 * Combinations the operating system or the browser owns.
 *
 * F30 §7 SYSTEM_RESERVED: these must be refused with an explanation rather than
 * remapped, because the app cannot actually intercept them and a "saved"
 * binding would simply never fire.
 */
const SYSTEM_RESERVED: readonly string[] = [
  'alt+f4',
  'alt+tab',
  'ctrl+alt+delete',
  'meta+l',
  'meta+tab',
  'ctrl+w',
  'ctrl+shift+w',
  'f5',
  'ctrl+r',
  'ctrl+shift+i',
  'ctrl+shift+j',
];

function normalizeKey(key: string): string {
  const trimmed = key.trim();
  if (trimmed.length === 1) {
    return trimmed.toLocaleLowerCase();
  }
  // Named keys keep their conventional spelling for comparison, but matching is
  // case-insensitive so `tab` and `Tab` cannot both be stored.
  return trimmed.toLocaleLowerCase();
}

function orderedUnique(modifiers: readonly Modifier[]): Modifier[] {
  return MODIFIER_ORDER.filter((modifier) => modifiers.includes(modifier));
}

/** Canonical text form, e.g. `Ctrl+Shift+F`; steps joined with a space. */
export function chordText(chord: Chord): string {
  return chord
    .map((step) => {
      const modifiers = orderedUnique(step.modifiers).map(
        (modifier) => MODIFIER_LABEL[modifier],
      );
      const key = step.key.length === 1 ? step.key.toLocaleUpperCase() : step.key;
      return [...modifiers, key].join('+');
    })
    .join(' ');
}

/** Comparison form: lower-cased, modifiers ordered, no spaces around `+`. */
export function chordKey(chord: Chord): string {
  return chord
    .map((step) => {
      const modifiers = orderedUnique(step.modifiers);
      return [...modifiers, normalizeKey(step.key)].join('+');
    })
    .join(' ');
}

export function validateChord(chord: Chord): ChordError | null {
  if (chord.length === 0) {
    return { code: 'empty' };
  }
  if (chord.length > MAX_CHORD_STEPS) {
    return { code: 'tooManySteps', limit: MAX_CHORD_STEPS };
  }
  for (let index = 0; index < chord.length; index += 1) {
    const step = chord[index];
    if (step === undefined || step.key.trim() === '') {
      return { code: 'emptyStep', index };
    }
    // A named key is longer than one character on purpose; only reject a key
    // that would make the display ambiguous, i.e. more than one character that
    // is not a known name.
    // Named keys are longer than one character by design: `F5`, `ArrowUp`,
    // `PageDown`. Anything else multi-character would make the display
    // ambiguous, so it is refused instead of being stored as an odd key.
    if (step.key.length > 1 && !/^[A-Za-z][A-Za-z0-9]*$/.test(step.key)) {
      return { code: 'multiCharacterKey', value: step.key };
    }
    const text = chordKey([step]);
    if (SYSTEM_RESERVED.includes(text)) {
      return { code: 'systemReserved', chord: chordText([step]) };
    }
  }
  // A bare letter would swallow typing everywhere; a lone modifier is not a
  // usable chord either.
  const first = chord[0];
  if (first !== undefined && first.modifiers.length === 0 && first.key.length === 1) {
    return { code: 'noModifier', chord: chordText([first]) };
  }
  return null;
}

/**
 * Validates a proposed binding against the current map.
 *
 * `replace` is the caller's explicit decision after seeing a conflict; without
 * it a conflict is reported and nothing changes, which is what F30-A03 requires.
 */
export function bindChord(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
  commandId: BindableCommandId,
  chord: Chord,
  replace = false,
): BindingResult {
  const text = chordText(chord);
  const error = validateChord(chord);
  if (error !== null) {
    return { accepted: false, error, conflictWith: null, chord: text };
  }
  const key = chordKey(chord);
  const conflict = (Object.keys({ ...DEFAULT_BINDINGS, ...bindings }) as BindableCommandId[]).find(
    (other) => other !== commandId && chordKey(effectiveBinding(bindings, other) ?? []) === key,
  );
  if (conflict !== undefined && !replace) {
    return {
      accepted: false,
      error: null,
      conflictWith: conflict,
      chord: text,
    };
  }
  return { accepted: true, error: null, conflictWith: conflict ?? null, chord: text };
}

/** Applies a successful binding, clearing the previous owner on replace. */
export function applyBinding(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
  commandId: BindableCommandId,
  chord: Chord,
  replace = false,
): { accepted: boolean; bindings: Partial<Record<BindableCommandId, Chord>>; result: BindingResult } {
  const result = bindChord(bindings, commandId, chord, replace);
  if (!result.accepted) {
    return { accepted: false, bindings: { ...bindings }, result };
  }
  const next: Partial<Record<BindableCommandId, Chord>> = { ...bindings };
  if (result.conflictWith !== null) {
    next[result.conflictWith] = [];
  }
  next[commandId] = chord;
  return { accepted: true, bindings: next, result };
}

export function resetBinding(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
  commandId: BindableCommandId,
  replace = false,
): ReturnType<typeof applyBinding> {
  const fallback = DEFAULT_BINDINGS[commandId];
  const applied = fallback === undefined
    ? { accepted: true, bindings: { ...bindings }, result: { accepted: true, error: null, conflictWith: null, chord: '' } }
    : applyBinding(bindings, commandId, fallback, replace);
  if (applied.accepted) {
    delete applied.bindings[commandId];
  }
  return applied;
}

/** Empty text is an explicit unbinding; missing or invalid entries use defaults. */
export function parseBindings(stored: Readonly<Record<string, string>>): Partial<Record<BindableCommandId, Chord>> {
  const bindings: Partial<Record<BindableCommandId, Chord>> = {};
  for (const [id, text] of Object.entries(stored)) {
    if (!isBindableCommandId(id)) continue;
    const chord = text === '' ? [] : parseChord(text);
    if (chord !== null && (chord.length === 0 || validateChord(chord) === null)) {
      bindings[id] = chord;
    }
  }
  return bindings;
}

/** Parses a user-typed chord such as `Ctrl+Shift+F` or `Ctrl+K Ctrl+S`. */
export function parseChord(text: string): Chord | null {
  const steps = text.trim().split(/\s+/).filter((step) => step !== '');
  if (steps.length === 0) {
    return null;
  }
  const chord: ChordStep[] = [];
  for (const step of steps) {
    const parts = step.split('+').map((part) => part.trim()).filter((part) => part !== '');
    if (parts.length === 0) {
      return null;
    }
    const key = parts[parts.length - 1] ?? '';
    const modifiers: Modifier[] = [];
    for (const part of parts.slice(0, -1)) {
      const lower = part.toLocaleLowerCase();
      if (lower === 'ctrl' || lower === 'control') {
        modifiers.push('ctrl');
      } else if (lower === 'shift') {
        modifiers.push('shift');
      } else if (lower === 'alt' || lower === 'option') {
        modifiers.push('alt');
      } else if (lower === 'meta' || lower === 'cmd' || lower === 'super') {
        modifiers.push('meta');
      } else {
        return null;
      }
    }
    if (key === '') {
      return null;
    }
    chord.push({
      modifiers: orderedUnique(modifiers),
      key: key.length === 1 ? key.toLocaleLowerCase() : key,
    });
  }
  return chord;
}

/** Default bindings, shown when a command has no stored override. */
export const DEFAULT_BINDINGS: Readonly<Partial<Record<BindableCommandId, Chord>>> = {
  'workspace.search': [{ modifiers: ['ctrl', 'shift'], key: 'f' }],
  'workspace.quickOpen': [{ modifiers: ['ctrl'], key: 'p' }],
  'workspace.commandPalette': [{ modifiers: ['ctrl', 'shift'], key: 'p' }],
  'workspace.export': [{ modifiers: ['ctrl', 'shift'], key: 'e' }],
  'workspace.rename': [{ modifiers: [], key: 'F2' }],
  'editor.find': [{ modifiers: ['ctrl'], key: 'f' }],
  'editor.replace': [{ modifiers: ['ctrl'], key: 'h' }],
};

/**
 * The subset of a keyboard event this module needs.
 *
 * Keeping it structural lets the resolver be tested without a DOM, and lets the
 * caller pass a real `KeyboardEvent` unchanged.
 */
export interface KeyEventLike {
  key: string;
  isComposing?: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

/** Builds the chord a single key press represents, in canonical form. */
export function chordFromEvent(event: KeyEventLike): Chord {
  const modifiers: Modifier[] = [];
  if (event.ctrlKey) {
    modifiers.push('ctrl');
  }
  if (event.shiftKey) {
    modifiers.push('shift');
  }
  if (event.altKey) {
    modifiers.push('alt');
  }
  if (event.metaKey) {
    modifiers.push('meta');
  }
  return [{ modifiers, key: event.key }];
}

export type ResolvedCommand =
  | { kind: 'command'; command: BindableCommandId }
  | { kind: 'reserved'; chord: string }
  | { kind: 'none' };

/**
 * Resolves a key press against the effective bindings.
 *
 * A stored override wins over the default, so rebinding a command actually
 * changes which keys fire. Two rules are deliberately strict:
 *
 * * a key press that is system-reserved reports `reserved` instead of running a
 *   command, so a stored binding can never shadow something the OS owns;
 * * a press that matches nothing is `none`, never a guess.
 */
export function resolveCommand(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
  event: KeyEventLike,
): ResolvedCommand {
  if (event.isComposing) return { kind: 'none' };
  const pressed = chordKey(chordFromEvent(event));
  if (pressed === '') {
    return { kind: 'none' };
  }
  const error = validateChord(chordFromEvent(event));
  if (error !== null && error.code === 'systemReserved') {
    return { kind: 'reserved', chord: error.chord };
  }
  for (const commandId of Object.keys(DEFAULT_BINDINGS) as BindableCommandId[]) {
    const binding = effectiveBinding(bindings, commandId);
    if (binding !== null && chordKey(binding) === pressed) {
      return { kind: 'command', command: commandId };
    }
  }
  // A stored binding for a command that has no default still counts.
  for (const [key, chord] of Object.entries(bindings)) {
    if (chord !== undefined && chord.length > 0 && chordKey(chord) === pressed) {
      return { kind: 'command', command: key as BindableCommandId };
    }
  }
  return { kind: 'none' };
}

/** True when the press matches the command's effective binding. */
export function matchesCommand(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
  commandId: BindableCommandId,
  event: KeyEventLike,
): boolean {
  const binding = effectiveBinding(bindings, commandId);
  if (binding === null) {
    return false;
  }
  return chordKey(binding) === chordKey(chordFromEvent(event));
}

/**
 * Two-step dispatch state. A stored chord with two steps needs a prefix: the
 * first press arms it, and only a matching second press runs the command.
 */
export interface ChordPrefix {
  /** The armed first step, in comparison form. */
  key: string;
  /** Command the prefix belongs to. */
  command: BindableCommandId;
}

export type StepResolution =
  | { kind: 'command'; command: BindableCommandId }
  | { kind: 'prefix'; prefix: ChordPrefix }
  | { kind: 'reserved'; chord: string }
  | { kind: 'none' };

/**
 * Resolves one key press, taking an armed prefix into account.
 *
 * A press that completes an armed two-step chord runs its command. A press that
 * matches a different command's prefix replaces the armed prefix only when it
 * is itself a prefix; otherwise the prefix is dropped and the press is evaluated
 * from scratch, which is what a user expects after pressing something unrelated.
 */
export function resolveStep(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
  event: KeyEventLike,
  armed: ChordPrefix | null,
): StepResolution {
  if (event.isComposing) return { kind: 'none' };
  const pressed = chordKey(chordFromEvent(event));
  const error = validateChord(chordFromEvent(event));
  if (error !== null && error.code === 'systemReserved') {
    return { kind: 'reserved', chord: error.chord };
  }
  if (armed !== null) {
    const completed = findTwoStep(bindings, armed.key, pressed);
    if (completed !== null) {
      return { kind: 'command', command: completed };
    }
    // An unrelated press cancels the prefix rather than silently waiting.
    if (pressed !== armed.key) {
      armed = null;
    } else {
      // The same prefix pressed again is "cancel", not a repeat.
      return { kind: 'none' };
    }
  }
  const single = resolveCommand(bindings, event);
  if (single.kind === 'command') {
    return single;
  }
  const prefix = findPrefix(bindings, pressed);
  if (prefix !== null) {
    return { kind: 'prefix', prefix };
  }
  return single;
}

function findTwoStep(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
  first: string,
  second: string,
): BindableCommandId | null {
  for (const commandId of Object.keys({ ...DEFAULT_BINDINGS, ...bindings }) as BindableCommandId[]) {
    const binding = effectiveBinding(bindings, commandId);
    if (binding === null || binding.length !== 2) {
      continue;
    }
    const [stepOne, stepTwo] = binding;
    if (
      stepOne !== undefined &&
      stepTwo !== undefined &&
      chordKey([stepOne]) === first &&
      chordKey([stepTwo]) === second
    ) {
      return commandId;
    }
  }
  return null;
}

function findPrefix(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
  pressed: string,
): ChordPrefix | null {
  for (const commandId of Object.keys({ ...DEFAULT_BINDINGS, ...bindings }) as BindableCommandId[]) {
    const binding = effectiveBinding(bindings, commandId);
    if (binding === null || binding.length !== 2) {
      continue;
    }
    const [stepOne] = binding;
    if (stepOne !== undefined && chordKey([stepOne]) === pressed) {
      return { key: pressed, command: commandId };
    }
  }
  return null;
}

export function effectiveBinding(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
  commandId: BindableCommandId,
): Chord | null {
  const binding = bindings[commandId] ?? DEFAULT_BINDINGS[commandId];
  return binding === undefined || binding.length === 0 ? null : binding;
}

const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta', 'AltGraph', 'OS']);

/** A press of a modifier alone, which only starts a chord and is never one. */
export function isModifierKey(key: string): boolean {
  return MODIFIER_KEYS.has(key);
}

/**
 * A matcher for one command's current chord, for surfaces that handle their
 * own keys (the editors and the reading pane). The first step of a two-step
 * chord reports `armed` and is remembered for four seconds; any other press,
 * except a bare modifier, cancels it.
 */
export function chordMatcher(
  chord: () => Chord | null,
  now: () => number = Date.now,
): (event: KeyEventLike) => 'complete' | 'armed' | 'none' {
  let armedAt: number | null = null;
  return (event) => {
    if (MODIFIER_KEYS.has(event.key)) return 'none';
    const binding = chord();
    if (binding === null || event.isComposing) {
      armedAt = null;
      return 'none';
    }
    const pressed = chordKey(chordFromEvent(event));
    const [first, second] = binding;
    if (second !== undefined && armedAt !== null && now() - armedAt < 4000) {
      armedAt = null;
      return chordKey([second]) === pressed ? 'complete' : 'none';
    }
    armedAt = null;
    if (first === undefined || chordKey([first]) !== pressed) return 'none';
    if (second === undefined) return 'complete';
    armedAt = now();
    return 'armed';
  };
}
