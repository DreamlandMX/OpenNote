import { useState } from 'react';
import { newOperationId } from './tauri-note-service';
import { TagBatchStatus } from './TagBatchStatus';
import { X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import type { NoteService, TagRenamePlan, TagWritePlan, TagBatchResult, TagWriteRunner } from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  actionKey,
  canApply,
  changeDescription,
  conflictRows,
  conflictText,
  isChange,
  planSummary,
  rewriteRows,
} from './tag-rename-view';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** Freeze the source tag while the report refreshes after a write. */
  tag: string;
  runWrite: TagWriteRunner;
  onApplied(): void;
  onClose(): void;
}

/** S15 previews the saved sources; conflicts pause only the affected notes. */
export function TagRenameDialog({
  service,
  translator,
  locale,
  tag,
  runWrite,
  onApplied,
  onClose,
}: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [target, setTarget] = useState('');
  const [conflictPaths, setConflictPaths] = useState<Readonly<Record<string, string>>>({});
  const [plan, setPlan] = useState<TagRenamePlan | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [busy, setBusy] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [writePlan, setWritePlan] = useState<TagWritePlan | null>(null);
  const [result, setResult] = useState<TagBatchResult | null>(null);
  const [operationId, setOperationId] = useState(newOperationId);
  const [submitted, setSubmitted] = useState(false);

  async function preview(newTag: string | null): Promise<void> {
    setBusy(true); setPlan(null); setWritePlan(null); setError(null); setResult(null);
    setOperationId(newOperationId());
    try {
      const outcome = await service.previewTagUpdate(tag, newTag);
      if (outcome.ok) { setPlan(outcome.value.plan); setWritePlan(outcome.value.writePlan); setConflictPaths(outcome.value.conflictPaths); }
      else setError(outcome.error);
    } finally { setBusy(false); }
  }

  async function apply(resume = false): Promise<void> {
    if (!writePlan || busy || (!resume && !canApply(plan))) return;
    setBusy(true); setSubmitted(true); setError(null);
    try {
      const outcome = await runWrite(() => resume ? service.resumeTagUpdate(operationId) : service.applyTagUpdate(writePlan, operationId), writePlan.items.map(item => item.noteId));
      if (outcome.ok) { setResult(outcome.value); onApplied(); }
      else { setError(outcome.error); if (outcome.error.operationId !== operationId) setSubmitted(false); }
    } finally { setBusy(false); }
  }

  async function cancel(): Promise<void> {
    setCancelling(true);
    try {
      const outcome = await service.cancelTagUpdate(operationId);
      if (outcome.ok) { setResult(outcome.value); onApplied(); }
      else setError(outcome.error);
    } finally { setCancelling(false); }
  }

  const summary = plan === null ? null : planSummary(plan);
  const conflicts = plan === null ? [] : conflictRows(plan);
  const rewrites = plan === null ? [] : rewriteRows(plan);

  return (
    <aside
      className="tag-rename-panel"
      aria-labelledby="tag-rename-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault(); event.stopPropagation();
          if (!busy && !cancelling) onClose();
        }
      }}
    >
      <header className="tag-rename-head">
        <h2 id="tag-rename-heading">{translator('tags.renameTitle', { tag })}</h2>
        <button
          type="button"
          className="tag-rename-close"
          aria-label={translator('tags.close')}
          title={translator('tags.close')}
          disabled={busy || cancelling}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <p className="tag-rename-body">{translator('tags.renameBody')}</p>

      <label className="tag-rename-field">
        <span>{translator('tags.renameTarget')}</span>
        <input
          type="text"
          value={target}
          disabled={busy || submitted || cancelling}
          placeholder={translator('tags.renamePlaceholder')}
          onChange={(event) => {
            setTarget(event.target.value);
            setPlan(null);
            setWritePlan(null);
          }}
        />
      </label>

      <div className="tag-rename-controls">
        <button
          type="button"
          className="primary"
          disabled={busy || submitted || cancelling || !isChange(tag, target)}
          onClick={() => void preview(target.trim())}
        >
          {translator('tags.previewRename')}
        </button>
        <button type="button" className="secondary" disabled={busy || submitted || cancelling} onClick={() => void preview(null)}>
          {translator('tags.previewRemoval')}
        </button>
      </div>

      {error !== null ? (
        <div className="tag-rename-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
        </div>
      ) : null}

      {plan !== null && summary !== null ? (
        <>
          <p className="tag-rename-summary" role="status">
            {summary.merge
              ? translator('tags.mergeSummary', {
                  notes: summary.notes,
                  changes: summary.changes,
                  target: plan.newTag,
                })
              : translator('tags.renameSummary', {
                  notes: summary.notes,
                  changes: summary.changes,
                })}
          </p>

          {rewrites.length > 0 ? (
            <section className="tag-rename-section" aria-labelledby="tag-rename-notes">
              <h3 id="tag-rename-notes">{translator('tags.affectedNotes')}</h3>
              <ul>
                {rewrites.map((row) => (
                  <li key={row.noteId} className="tag-rename-row">
                    <span className="tag-rename-path">{row.relativePath}</span>
                    <span className="tag-rename-count">
                      {changeDescription(row.changed, translator)}
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          {conflicts.length > 0 ? (
            <section className="tag-rename-section" aria-labelledby="tag-rename-conflicts">
              <h3 id="tag-rename-conflicts">{translator('tags.conflicts')}</h3>
              <ul>
                {conflicts.map((row, index) => (
                  <li key={`${row.key}:${index}`} className="tag-rename-row tag-rename-conflict">
                    <span className="tag-rename-path">{conflictPaths[row.noteId]}</span>
                    <span>{conflictText(row, translator)}</span>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          {!submitted ? <button type="button" className="primary" disabled={busy || !writePlan || !canApply(plan)} onClick={() => void apply()}>{translator(actionKey(plan))}</button> : null}

        </>
      ) : null}

      {result ? <TagBatchStatus result={result} translator={translator} locale={locale} /> : null}
      {submitted && !result?.completed && !result?.cancelled ? <div className="panel-actions">
        {!busy ? <button type="button" disabled={cancelling} onClick={() => void apply(true)}>{translator('tags.resume')}</button> : null}
        <button type="button" disabled={cancelling} onClick={() => void cancel()}>{translator('tags.cancelBatch')}</button>
      </div> : null}
      <p className="tag-rename-note">{translator('tags.writeHint')}</p>
      <p className="tag-rename-note">{translator('tags.renameHint')}</p>
    </aside>
  );
}
