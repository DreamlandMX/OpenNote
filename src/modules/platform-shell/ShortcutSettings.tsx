import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { Keyboard, RotateCcw, X } from 'lucide-react';
import type { Translator } from '../presentation-i18n';
import {
  applyBinding,
  chordKey,
  chordText,
  DEFAULT_BINDINGS,
  effectiveBinding,
  isModifierKey,
  parseChord,
  resetBinding,
  type Chord,
} from './shortcuts';
import { BINDABLE_COMMANDS, bindableCommand, type BindableCommandId } from './command-catalogue';
import { handleTrappedTab } from './focus-trap';

interface Props {
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>;
  translator: Translator;
  /** Called with the new map after an accepted change. */
  onChange(bindings: Partial<Record<BindableCommandId, Chord>>): void;
  onClose(): void;
}

type Capture =
  | { kind: 'idle' }
  | { kind: 'capturing'; commandId: BindableCommandId; chord: Chord | null; text: string }
  | { kind: 'conflict'; commandId: BindableCommandId; chord: Chord; other: BindableCommandId; reset?: boolean }
  | { kind: 'error'; commandId: BindableCommandId; messageKey: 'shortcut.reserved' | 'shortcut.noModifier' | 'shortcut.invalid' };

/**
 * S46 keyboard settings.
 *
 * A chord is captured by pressing it, not by typing text, so the stored value is
 * always something the browser actually reported. Validation, conflicts and
 * system-reserved combinations come from the shared model, so what this panel
 * accepts is exactly what dispatch will later recognise (F30-R01).
 */
export function ShortcutSettings({ bindings, translator, onChange, onClose }: Props) {
  const [capture, setCapture] = useState<Capture>({ kind: 'idle' });
  const [announcement, setAnnouncement] = useState('');
  const openerRef = useRef<Element | null>(null);
  const conflictRef = useRef<HTMLDivElement | null>(null);
  const conflictOpenerRef = useRef<Element | null>(null);

  useEffect(() => {
    openerRef.current = document.activeElement;
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);

  const conflictOpen = capture.kind === 'conflict';
  useEffect(() => {
    if (!conflictOpen) {
      return;
    }
    conflictOpenerRef.current = document.activeElement;
    conflictRef.current?.focus();
  }, [conflictOpen]);

  /** Closes the conflict dialog and returns focus to the chord button. */
  function closeConflict(): void {
    setCapture({ kind: 'idle' });
    const opener = conflictOpenerRef.current;
    conflictOpenerRef.current = null;
    if (opener instanceof HTMLElement && opener.isConnected) {
      opener.focus();
    }
  }

  function beginCapture(commandId: BindableCommandId): void {
    setCapture({ kind: 'capturing', commandId, chord: null, text: '' });
    setAnnouncement(translator('shortcut.capturing'));
  }

  function handleCaptureKey(event: KeyboardEvent<HTMLButtonElement>): void {
    if (capture.kind !== 'capturing') {
      return;
    }
    // An IME composition is text, not a command (F30-R02).
    if (event.nativeEvent.isComposing) {
      return;
    }
    event.preventDefault();
    // Ctrl, Shift and the like arrive first on their own; wait for the key they modify.
    if (isModifierKey(event.key)) {
      return;
    }
    if (event.key === 'Escape') {
      setCapture({ kind: 'idle' });
      setAnnouncement(translator('shortcut.cancelled'));
      return;
    }
    const chord = parseChord(
      [
        event.ctrlKey ? 'Ctrl' : '',
        event.altKey ? 'Alt' : '',
        event.shiftKey ? 'Shift' : '',
        event.metaKey ? 'Meta' : '',
        event.key,
      ]
        .filter((part) => part !== '')
        .join('+'),
    );
    if (chord === null) {
      setCapture({ kind: 'error', commandId: capture.commandId, messageKey: 'shortcut.invalid' });
      return;
    }
    const applied = applyBinding(bindings, capture.commandId, chord);
    if (!applied.accepted) {
      const error = applied.result.error;
      if (applied.result.conflictWith !== null) {
        setCapture({
          kind: 'conflict',
          commandId: capture.commandId,
          chord,
          other: applied.result.conflictWith,
        });
        return;
      }
      const messageKey =
        error?.code === 'systemReserved'
          ? 'shortcut.reserved'
          : error?.code === 'noModifier'
            ? 'shortcut.noModifier'
            : 'shortcut.invalid';
      setCapture({ kind: 'error', commandId: capture.commandId, messageKey });
      return;
    }
    onChange(applied.bindings);
    setCapture({ kind: 'idle' });
    setAnnouncement(translator('shortcut.saved', { chord: chordText(chord) }));
  }

  function confirmReplace(): void {
    if (capture.kind !== 'conflict') {
      return;
    }
    const applied = capture.reset
      ? resetBinding(bindings, capture.commandId, true)
      : applyBinding(bindings, capture.commandId, capture.chord, true);
    if (applied.accepted) {
      onChange(applied.bindings);
      setAnnouncement(translator('shortcut.replaced', { chord: chordText(capture.chord) }));
    }
    closeConflict();
  }

  function reset(commandId: BindableCommandId): void {
    const applied = resetBinding(bindings, commandId);
    const fallback = DEFAULT_BINDINGS[commandId];
    if (!applied.accepted && applied.result.conflictWith !== null && fallback !== undefined) {
      setCapture({ kind: 'conflict', commandId, chord: fallback, other: applied.result.conflictWith, reset: true });
      return;
    }
    if (!applied.accepted) return;
    onChange(applied.bindings);
    setCapture({ kind: 'idle' });
    setAnnouncement(
      fallback === undefined
        ? translator('shortcut.resetNone')
        : translator('shortcut.resetDone', { chord: chordText(fallback) }),
    );
  }

  return (
    <aside
      className="shortcut-panel"
      aria-labelledby="shortcut-heading"
      onKeyDown={(event) => {
        // While capturing, Escape cancels the capture (handled by the chord
        // button) instead of closing the panel, so a user cannot lose the
        // dialog just by aborting a key press.
        if (event.key === 'Escape' && !event.nativeEvent.isComposing && capture.kind !== 'capturing') {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="shortcut-head">
        <h2 id="shortcut-heading">{translator('shortcut.title')}</h2>
        <button
          type="button"
          className="shortcut-close"
          aria-label={translator('shortcut.close')}
          title={translator('shortcut.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>
      <p className="shortcut-body">{translator('shortcut.body')}</p>

      <ul className="shortcut-list">
        {BINDABLE_COMMANDS.map((command) => {
          const binding = effectiveBinding(bindings, command.id);
          const capturing = capture.kind === 'capturing' && capture.commandId === command.id;
          const isDefault = bindings[command.id] === undefined;
          return (
            <li key={command.id} className="shortcut-row">
              <span className="shortcut-name">{translator(command.labelKey)}</span>
              <button
                type="button"
                className="shortcut-chord"
                aria-label={translator('shortcut.change', {
                  name: translator(command.labelKey),
                })}
                onClick={() => beginCapture(command.id)}
                onKeyDown={handleCaptureKey}
              >
                {capturing ? (
                  translator('shortcut.pressNow')
                ) : binding === null ? (
                  translator('shortcut.unbound')
                ) : (
                  chordText(binding)
                )}
              </button>
              {isDefault ? (
                <span className="shortcut-badge">{translator('shortcut.defaultBadge')}</span>
              ) : (
                <button
                  type="button"
                  className="shortcut-reset"
                  aria-label={translator('shortcut.reset', {
                    name: translator(command.labelKey),
                  })}
                  title={translator('shortcut.reset', { name: translator(command.labelKey) })}
                  onClick={() => reset(command.id)}
                >
                  <RotateCcw size={14} aria-hidden="true" />
                </button>
              )}
              {capture.kind === 'conflict' && capture.commandId === command.id ? (
                <div
                  className="shortcut-dialog"
                  role="alertdialog"
                  aria-modal="true"
                  aria-labelledby="shortcut-conflict"
                  ref={conflictRef}
                  tabIndex={-1}
                  onKeyDown={(event) => {
                    if (event.key === 'Tab') {
                      if (handleTrappedTab(event.currentTarget, document.activeElement, event.shiftKey)) {
                        event.preventDefault();
                      }
                      event.stopPropagation();
                      return;
                    }
                    // Escape cancels this conflict only; closing the whole panel
                    // would throw away the binding the user is still editing.
                    if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
                      event.preventDefault();
                      event.stopPropagation();
                      closeConflict();
                    }
                  }}
                >
                  <p id="shortcut-conflict">
                    {translator('shortcut.conflict', {
                      chord: chordText(capture.chord),
                      other: translator(bindableCommand(capture.other).labelKey),
                    })}
                  </p>
                  <div className="shortcut-dialog-actions">
                    <button type="button" className="primary" onClick={confirmReplace}>
                      {translator('shortcut.replace')}
                    </button>
                    <button type="button" className="secondary" onClick={closeConflict}>
                      {translator('shortcut.cancel')}
                    </button>
                  </div>
                </div>
              ) : null}
              {capture.kind === 'error' && capture.commandId === command.id ? (
                <p className="shortcut-error" role="alert">
                  {translator(capture.messageKey)}
                </p>
              ) : null}
            </li>
          );
        })}
      </ul>

      <p className="shortcut-hint">
        <Keyboard size={14} aria-hidden="true" /> {translator('shortcut.hint')}
      </p>
      <p className="visually-hidden" role="status" aria-live="polite">
        {announcement}
      </p>
    </aside>
  );
}

/** True when a stored map can round-trip through the text form. */
export function bindingsRoundTrip(
  bindings: Readonly<Partial<Record<BindableCommandId, Chord>>>,
): boolean {
  return Object.entries(bindings).every(([, chord]) => {
    if (chord === undefined || chord.length === 0) {
      return true;
    }
    const parsed = parseChord(chordText(chord));
    return parsed !== null && chordKey(parsed) === chordKey(chord);
  });
}
