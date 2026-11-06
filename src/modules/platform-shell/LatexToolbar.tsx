import { useState } from 'react';
import type { AssistResult, NoteService } from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import { applyResult, isValidRange } from './latex-assist-view';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** Current buffer text, which the actions operate on. */
  source: string;
  /** Current selection in the buffer. */
  selection: { from: number; to: number } | null;
  /** True while an IME composition owns the pending text. */
  composing: boolean;
  /** Applies the computed edits through the editor so it is one undo step. */
  apply(edits: readonly { start: number; end: number; text: string }[]): boolean;
  /** Fallback when no editor is mounted: replace the buffer text directly. */
  onReplaceSource(next: string): void;
  onClose(): void;
}

const WRAP_COMMANDS = ['textbf', 'emph', 'section', 'subsection', 'label', 'ref', 'cite'] as const;

/**
 * F47 LaTeX editing toolbar.
 *
 * The actions are computed by M13 and applied through the editor as one
 * transaction, so each is one undo step (F47-R02). While an IME composition is
 * active every action is disabled and explained, because pairing or wrapping then
 * would swallow the half-typed text (F47-R03).
 */
export function LatexToolbar({
  service,
  translator,
  locale,
  source,
  selection,
  composing,
  apply,
  onReplaceSource,
  onClose,
}: Props) {
  const [error, setError] = useState<ServiceError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const range = selection === null ? { start: 0, end: 0 } : { start: selection.from, end: selection.to };
  const hasSelection = range.end > range.start;
  const blocked = composing;

  async function run(
    outcome: Promise<{ ok: true; value: AssistResult } | { ok: false; error: ServiceError }>,
    successKey: 'latex.done.comment' | 'latex.done.wrap' | 'latex.done.complete',
  ): Promise<void> {
    const result = await outcome;
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setError(null);
    // A result with any unsafe range is refused as a whole rather than applied
    // partially, which would leave the buffer in a state the user did not choose.
    if (!result.value.edits.every((edit) => isValidRange(source, edit.range))) {
      setNotice(translator('latex.rangeStale'));
      return;
    }
    // The port uses a nested range; the editor takes flat offsets.
    const flat = result.value.edits.map((edit) => ({
      start: edit.range.start,
      end: edit.range.end,
      text: edit.text,
    }));
    if (apply(flat)) {
      setNotice(translator(successKey));
      return;
    }
    // No editor mounted: compute the text and hand it back.
    onReplaceSource(applyResult(source, result.value));
    setNotice(translator(successKey));
  }

  return (
    <div className="latex-toolbar" role="toolbar" aria-label={translator('latex.toolbar')}>
      <span className="latex-mode">{translator('latex.toolbarHint')}</span>
      <button
        type="button"
        className="secondary"
        disabled={blocked}
        title={blocked ? translator('latex.composing') : translator('latex.comment')}
        onClick={() =>
          void run(
            service.latexToggleComment(source, range, composing),
            'latex.done.comment',
          )
        }
      >
        {translator('latex.comment')}
      </button>
      {WRAP_COMMANDS.map((command) => (
        <button
          key={command}
          type="button"
          className="secondary"
          disabled={blocked || !hasSelection}
          title={
            blocked
              ? translator('latex.composing')
              : !hasSelection
                ? translator('latex.needSelection')
                : command
          }
          onClick={() =>
            void run(
              service.latexWrapSelection(source, range, command, composing),
              'latex.done.wrap',
            )
          }
        >
          {command}
        </button>
      ))}
      <button type="button" className="secondary" onClick={onClose}>
        {translator('latex.close')}
      </button>
      {blocked ? (
        <span className="latex-blocked" role="status">
          {translator('latex.composing')}
        </span>
      ) : null}
      {notice !== null ? (
        <span className="latex-notice" role="status">
          {notice}
        </span>
      ) : null}
      {error !== null ? (
        <span className="latex-error" role="alert">
          {serviceErrorMessage(locale, error.code)}
        </span>
      ) : null}
    </div>
  );
}
