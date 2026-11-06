import { useState } from 'react';
import { newOperationId } from './tauri-note-service';
import { X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import type { NoteService, RenameCommit, RenamePlan } from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  canCommit,
  commitSummary,
  folderOf,
  issueRows,
  issueText,
  needsExtension,
  normalizeDestination,
  stepRows,
  warningText,
  withExtension,
} from './rename-view';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  noteId: string;
  /** The current vault-relative path, shown so the change is legible. */
  currentPath: string;
  /** Where to start, when a rename begun elsewhere needs this preview. */
  initialDestination?: string;
  onClose(): void;
  onApply: NoteService['rename'];
}

/**
 * S13 rename preview.
 *
 * Renaming changes the note's path and rewrites every link that points at it, so
 * the dialog shows the blast radius before anything happens: which notes would be
 * rewritten, which links would be left alone and why, and what blocks the commit.
 *
 */
export function RenameDialog({
  service,
  translator,
  locale,
  noteId,
  currentPath,
  initialDestination,
  onClose,
  onApply,
}: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [destination, setDestination] = useState(initialDestination ?? currentPath);
  const [plan, setPlan] = useState<RenamePlan | null>(null);
  const [commit, setCommit] = useState<RenameCommit | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [busy, setBusy] = useState(false);
  const [operationId, setOperationId] = useState(newOperationId);
  const [submitted, setSubmitted] = useState(false);

  const normalized = normalizeDestination(destination);
  const extensionWarning = needsExtension(normalized);

  async function preview(): Promise<void> {
    if (normalized === '') {
      return;
    }
    setBusy(true);
    setPlan(null);
    setCommit(null);
    setError(null);
    setOperationId(newOperationId());
    const target = withExtension(normalized);
    const planned = await service.previewNoteRename(noteId, target);
    if (!planned.ok) {
      setBusy(false);
      setError(planned.error);
      return;
    }
    setPlan(planned.value);
    const committed = await service.previewRenameCommit(planned.value);
    setBusy(false);
    if (committed.ok) {
      setCommit(committed.value);
      setError(null);
    } else {
      setError(committed.error);
    }
  }

  async function apply(): Promise<void> {
    const baseline = plan?.expectedHashes.find(b => b.noteId === noteId);
    if (!plan || !baseline || !canCommit(commit) || busy) return;
    setBusy(true);
    setSubmitted(true);
    try {
      const outcome = await onApply(plan, baseline.expectedRevision, operationId);
      if (outcome.ok && outcome.value.completed) onClose();
      else if (!outcome.ok) {
        setError(outcome.error);
        if (outcome.error.operationId !== operationId) setSubmitted(false);
      }
    } finally { setBusy(false); }
  }

  const summary = commit === null ? null : commitSummary(commit);
  const issues = commit === null ? [] : issueRows(commit);
  const steps = commit === null ? [] : stepRows(commit);

  return (
    <aside
      className="rename-panel"
      aria-labelledby="rename-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !busy && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="rename-head">
        <h2 id="rename-heading">{translator('rename.title')}</h2>
        <button
          type="button"
          className="rename-close"
          aria-label={translator('rename.close')}
          title={translator('rename.close')}
          disabled={busy}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <p className="rename-body">{translator('rename.body')}</p>
      <p className="rename-from">{translator('rename.current', { path: currentPath })}</p>

      <label className="rename-field">
        <span>{translator('rename.destination')}</span>
        <input
          type="text"
          value={destination}
          disabled={busy || submitted}
          onChange={(event) => {
            setDestination(event.target.value);
            setPlan(null);
            setCommit(null);
          }}
        />
        {extensionWarning ? (
          <small className="rename-note">{translator('rename.extension')}</small>
        ) : null}
        {normalized === '' ? (
          <small className="rename-issue" role="alert">
            {translator('rename.escapesRoot')}
          </small>
        ) : null}
        {normalized !== '' ? (
          <small className="rename-note">
            {translator('rename.folder', { folder: folderOf(withExtension(normalized)) || '—' })}
          </small>
        ) : null}
      </label>

      <div className="rename-controls">
        <button
          type="button"
          className="primary"
          disabled={busy || submitted || normalized === '' || withExtension(normalized) === currentPath}
          onClick={() => void preview()}
        >
          {translator('rename.preview')}
        </button>
      </div>

      {error !== null ? (
        <div className="rename-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
          {error.params.completedItems !== undefined && error.params.totalItems !== undefined ? <p>{translator('rename.progress', { completed: error.params.completedItems, total: error.params.totalItems })}</p> : null}
        </div>
      ) : null}

      {commit !== null && summary !== null ? (
        <>
          <p className="rename-summary" role="status">
            {translator('rename.summary', {
              from: commit.moveFrom,
              to: commit.moveTo,
              notes: summary.notes,
              edits: summary.edits,
            })}
          </p>
          <p className="rename-note">
            {summary.keepsIdentity
              ? translator('rename.keepsIdentity')
              : translator('rename.newIdentity')}
          </p>

          {steps.length > 0 ? (
            <section className="rename-section" aria-labelledby="rename-notes">
              <h3 id="rename-notes">{translator('rename.affected')}</h3>
              <ul>
                {steps.map((row) => (
                  <li key={row.noteId} className="rename-row">
                    <span className="rename-path">{row.relativePath}</span>
                    <span className="rename-count">
                      {translator('rename.linkCount', { count: row.edits })}
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          ) : (
            <p className="rename-note" role="status">
              {translator('rename.noLinks')}
            </p>
          )}

          {issues.length > 0 ? (
            <section className="rename-section" aria-labelledby="rename-issues">
              <h3 id="rename-issues">{translator('rename.issues')}</h3>
              <ul>
                {issues.map((row, index) => (
                  <li key={`${row.key}:${index}`} className="rename-row rename-issue">
                    {issueText(row, translator)}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          {commit.untouched.length > 0 ? (
            <section className="rename-section" aria-labelledby="rename-untouched">
              <h3 id="rename-untouched">{translator('rename.untouched')}</h3>
              <p className="rename-note">{translator('rename.untouchedHint')}</p>
              <ul>
                {commit.untouched.map((warning, index) => (
                  <li key={`${warning.noteId}:${index}`} className="rename-row rename-warn">
                    {warningText(warning, translator)}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          <button type="button" className="primary" disabled={busy || !plan || !canCommit(commit)} onClick={() => void apply()}>
            {translator('rename.apply')}
          </button>
          <p className="rename-note">{translator('rename.recoveryHint')}</p>
        </>
      ) : null}

      {plan === null && commit === null ? (
        <p className="rename-note">{translator('rename.hint')}</p>
      ) : null}
    </aside>
  );
}
