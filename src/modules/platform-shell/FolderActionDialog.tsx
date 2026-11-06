import { useEffect, useRef } from 'react';
import type { Translator } from '../presentation-i18n';

export type FolderActionPhase =
  | { kind: 'checking' }
  /** `refusal` set: the action cannot run, and says why. */
  | { kind: 'review'; refusal: string | null; otherFiles: readonly string[]; otherCount: number }
  | { kind: 'running'; done: number; total: number }
  | { kind: 'stopped'; done: number; total: number; error: string }
  | { kind: 'done'; total: number };

interface Props {
  title: string;
  /** What the action will do, one line each; shown while reviewing. */
  summary: readonly string[];
  confirmLabel: string;
  danger: boolean;
  phase: FolderActionPhase;
  translator: Translator;
  onConfirm(): void;
  onResume(): void;
  onClose(): void;
}

/**
 * Review, progress and outcome of a folder rename, move or delete. The steps
 * run one note at a time, so a stop leaves a consistent vault and Continue
 * resumes at the step that failed. It cannot be dismissed while running.
 */
export function FolderActionDialog({ title, summary, confirmLabel, danger, phase, translator, onConfirm, onResume, onClose }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { dialog.current?.showModal?.(); }, []);
  const running = phase.kind === 'running';
  return <dialog ref={dialog} className="draft-dialog folder-dialog" aria-labelledby="folder-action-heading"
    onCancel={(event) => { event.preventDefault(); if (!running) onClose(); }}>
    <h2 id="folder-action-heading">{title}</h2>
    {phase.kind === 'checking' ? <p role="status">{translator('folderOp.checking')}</p> : null}
    {phase.kind === 'review' ? <>
      {phase.refusal === null ? <ul>{summary.map((line) => <li key={line}>{line}</li>)}</ul> : <p role="alert">{phase.refusal}</p>}
      {phase.otherCount > 0 ? <>
        <p>{translator('folderOp.otherFiles', { count: phase.otherCount })}</p>
        <ul className="folder-dialog-files">{phase.otherFiles.map((path) => <li key={path}>{path}</li>)}</ul>
      </> : null}
    </> : null}
    {phase.kind === 'running' || phase.kind === 'stopped' ? <>
      <progress max={phase.total} value={phase.done} aria-label={translator('folderOp.progress', { done: phase.done, total: phase.total })} />
      <p role="status">{translator('folderOp.progress', { done: phase.done, total: phase.total })}</p>
    </> : null}
    {phase.kind === 'stopped' ? <p role="alert">{translator('folderOp.stopped', { done: phase.done, total: phase.total, error: phase.error })}</p> : null}
    {phase.kind === 'done' ? <p role="status">{translator('folderOp.done', { total: phase.total })}</p> : null}
    <div className="panel-actions">
      {phase.kind === 'done' ? null : <button type="button" className="secondary" disabled={running} onClick={onClose}>{translator('folderOp.close')}</button>}
      {phase.kind === 'review' && phase.refusal === null ? <button type="button" className={danger ? 'primary danger' : 'primary'} onClick={onConfirm}>{confirmLabel}</button> : null}
      {phase.kind === 'stopped' ? <button type="button" className="primary" onClick={onResume}>{translator('folderOp.resume')}</button> : null}
      {phase.kind === 'done' ? <button type="button" className="primary" onClick={onClose}>{translator('folderOp.close')}</button> : null}
    </div>
  </dialog>;
}
