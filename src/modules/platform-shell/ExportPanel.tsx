import { useState } from 'react';
import { Download, X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import { noteDocument, noteTitle, printRenderedNote, renderNoteForExport } from '../editor';
import type {
  ExportCompatibilityLoss,
  ExportFormat,
  ExportOutcome,
  ExportScope,
  NoteService,
} from '../editor';
import type { ServiceError } from '../domain';
import type { MessageKey, Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** Current note path, offered as the single-note scope. */
  activePath: string | null;
  /** The open note's current text, for the document formats. */
  activeSource: string | null;
  /** Note paths, used to offer the folders that actually exist. */
  notePaths: readonly string[];
  defaultScope: ExportScope;
  onClose(): void;
}

/** Every distinct folder prefix, so the picker cannot offer an empty folder. */
export function folderChoices(notePaths: readonly string[]): string[] {
  const folders = new Set<string>();
  for (const path of notePaths) {
    const segments = path.replace(/\\/g, '/').split('/');
    segments.pop();
    let prefix = '';
    for (const segment of segments) {
      prefix = prefix === '' ? segment : `${prefix}/${segment}`;
      if (prefix !== '') {
        folders.add(prefix);
      }
    }
  }
  return [...folders].sort((left, right) => left.localeCompare(right));
}

type Result =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'cancelled' }
  | { kind: 'done'; outcome: ExportOutcome }
  | { kind: 'failed'; error: ServiceError };

const COMPATIBILITY_KEY: Record<ExportCompatibilityLoss['syntax'], MessageKey> = {
  embed: 'export.compatibility.embed',
  wikiLink: 'export.compatibility.wikiLink',
  blockReference: 'export.compatibility.blockReference',
};

function compatibilityKey(syntax: ExportCompatibilityLoss['syntax']): MessageKey {
  switch (syntax) {
    case 'embed':
      return COMPATIBILITY_KEY.embed;
    case 'wikiLink':
      return COMPATIBILITY_KEY.wikiLink;
    case 'blockReference':
      return COMPATIBILITY_KEY.blockReference;
  }
}

/**
 * S39 export. The confirmation step comes first because an export writes a real
 * file; the result step then reports what was actually published, including any
 * loss, and never shows "done" for a cancelled or unverified run (F25-A02/R04).
 */
export function ExportPanel({
  service,
  translator,
  locale,
  activePath,
  activeSource,
  notePaths,
  defaultScope,
  onClose,
}: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [scope, setScope] = useState<ExportScope>(defaultScope);
  const [format, setFormat] = useState<ExportFormat>('zip');
  const folders = folderChoices(notePaths);
  const [folder, setFolder] = useState(folders[0] ?? '');
  const [includeAttachments, setIncludeAttachments] = useState(true);
  const [result, setResult] = useState<Result>({ kind: 'idle' });
  const [documentState, setDocumentState] = useState<'idle' | 'working' | 'saved' | 'failed'>('idle');

  function renderCurrentNote() {
    return renderNoteForExport(service, activePath ?? '', activeSource ?? '', {
      frontmatterLabel: translator('editor.read.frontmatterLabel'),
      blockedImageRemote: translator('editor.read.imageBlockedRemote'),
      blockedImageLocal: (name: string) => translator('editor.read.imageBlockedLocal', { name }),
      openEmbeddedSource: (path: string) => translator('embed.openSource', { path }),
    });
  }

  async function saveHtml(): Promise<void> {
    if (activePath === null) return;
    setDocumentState('working');
    try {
      const rendered = await renderCurrentNote();
      const title = noteTitle(activePath);
      const saved = await service.saveNoteHtml({
        suggestedName: `${title}.html`,
        html: noteDocument(title, rendered.body, rendered.styles, locale),
        dialogTitle: translator('export.note.dialogTitle'),
      });
      setDocumentState(!saved.ok ? 'failed' : saved.value.cancelled ? 'idle' : 'saved');
    } catch {
      setDocumentState('failed');
    }
  }

  async function printNote(): Promise<void> {
    if (activePath === null) return;
    setDocumentState('working');
    try {
      const rendered = await renderCurrentNote();
      setDocumentState('idle');
      await printRenderedNote(noteTitle(activePath), rendered);
    } catch {
      setDocumentState('failed');
    }
  }

  async function run(): Promise<void> {
    setResult({ kind: 'running' });
    const outcome = await service.exportVault({
      scope,
      format,
      vaultTarget:
        scope === 'note' ? activePath ?? undefined : scope === 'folder' ? folder : undefined,
      suggestedName:
        scope === 'vault' && format === 'zip'
          ? 'OpenNote export.zip'
          : scope === 'vault'
            ? 'OpenNote export'
            : undefined,
      includeAttachments,
    });
    if (!outcome.ok) {
      setResult({ kind: 'failed', error: outcome.error });
      return;
    }
    if (outcome.value.cancelled) {
      setResult({ kind: 'cancelled' });
      return;
    }
    setResult({ kind: 'done', outcome: outcome.value });
  }

  return (
    <aside
      className="export-panel"
      aria-labelledby="export-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="export-head">
        <h2 id="export-heading">{translator('export.title')}</h2>
        <button
          type="button"
          className="export-close"
          aria-label={translator('export.close')}
          title={translator('export.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <fieldset className="export-scope" disabled={result.kind === 'running'}>
        <legend>{translator('export.format')}</legend>
        {(['zip', 'markdown'] as const).map((option) => (
          <label key={option}>
            <input
              type="radio"
              name="export-format"
              value={option}
              checked={format === option}
              disabled={result.kind === 'running'}
              onChange={() => setFormat(option)}
            />
            <span>{translator(`export.format.${option}`)}</span>
          </label>
        ))}
      </fieldset>
      {format === 'markdown' ? (
        <p className="export-hint">{translator('export.format.markdownHint')}</p>
      ) : null}

      <fieldset className="export-scope" disabled={result.kind === 'running'}>
        <legend>{translator('export.scope')}</legend>
        {(['vault', 'folder', 'note'] as const).map((option) => (
          <label key={option}>
            <input
              type="radio"
              name="export-scope"
              value={option}
              checked={scope === option}
              disabled={
                (option === 'note' && activePath === null) ||
                (option === 'folder' && folders.length === 0)
              }
              onChange={() => setScope(option)}
            />
            <span>{translator(`export.scope.${option}`)}</span>
          </label>
        ))}
      </fieldset>

      {scope === 'folder' ? (
        <label className="export-folder">
          <span>{translator('export.folderLabel')}</span>
          <select value={folder} onChange={(event) => setFolder(event.target.value)}>
            {folders.map((option) => (
              <option key={option} value={option}>
                {option}
              </option>
            ))}
          </select>
        </label>
      ) : null}

      <label className="export-attachments">
        <input
          type="checkbox"
          checked={includeAttachments}
          disabled={result.kind === 'running'}
          onChange={(event) => setIncludeAttachments(event.target.checked)}
        />
        <span>{translator('export.includeAttachments')}</span>
      </label>
      {!includeAttachments ? (
        <p className="export-warning" role="status">
          {translator(
            format === 'markdown'
              ? 'export.attachmentsOmitted.markdown'
              : 'export.attachmentsOmitted',
          )}
        </p>
      ) : null}
      <p className="export-hint">
        {translator(format === 'markdown' ? 'export.hint.markdown' : 'export.hint')}
      </p>

      {scope === 'note' && activePath !== null && activeSource !== null ? (
        <section className="export-document" aria-labelledby="export-document-heading">
          <h3 id="export-document-heading">{translator('export.note.heading')}</h3>
          <div className="export-actions">
            <button type="button" className="secondary" disabled={documentState === 'working'} onClick={() => void saveHtml()}>{translator('export.note.html')}</button>
            <button type="button" className="secondary" disabled={documentState === 'working'} onClick={() => void printNote()}>{translator('export.note.print')}</button>
          </div>
          <p className="export-hint">{translator('export.note.hint')}</p>
          {documentState === 'working' ? <p className="export-hint" role="status">{translator('export.note.working')}</p> : null}
          {documentState === 'saved' ? <p className="export-hint" role="status">{translator('export.note.saved')}</p> : null}
          {documentState === 'failed' ? <p className="export-warning" role="alert">{translator('export.note.failed')}</p> : null}
        </section>
      ) : null}

      <div className="export-actions">
        <button
          type="button"
          className="primary"
          disabled={
            result.kind === 'running' ||
            (scope === 'note' && activePath === null) ||
            (scope === 'folder' && folder === '')
          }
          onClick={() => void run()}
        >
          <Download size={15} aria-hidden="true" />
          {result.kind === 'running' ? translator('export.running') : translator('export.start')}
        </button>
        <button type="button" className="secondary" onClick={onClose}>
          {translator('export.close')}
        </button>
      </div>

      {result.kind === 'cancelled' ? (
        <p className="export-status" role="status">
          {translator('export.cancelled')}
        </p>
      ) : null}

      {result.kind === 'failed' ? (
        <div className="export-error" role="alert">
          <p>{serviceErrorMessage(locale, result.error.code)}</p>
          <p>{translator('export.failed')}</p>
        </div>
      ) : null}

      {result.kind === 'done' ? (
        <div className="export-result" role="status">
          <p>
            <strong>{translator('export.done')}</strong>
          </p>
          <p>
            {translator('export.summary', {
              notes: result.outcome.notes,
              attachments: result.outcome.attachments,
              bytes: result.outcome.bytes,
            })}
          </p>
          {result.outcome.path !== null ? (
            <p className="export-path">{result.outcome.path}</p>
          ) : null}
          {!result.outcome.verified ? (
            <p className="export-warning">{translator('export.unverified')}</p>
          ) : null}
          {result.outcome.losses.length > 0 ? (
            <>
              <p className="export-warning">
                {translator('export.losses', { count: result.outcome.losses.length })}
              </p>
              <ul className="export-losses">
                {result.outcome.losses.map((loss) => (
                  <li key={loss}>{loss}</li>
                ))}
              </ul>
            </>
          ) : null}
          {result.outcome.compatibilityLosses.length > 0 ? (
            <>
              <p className="export-warning">
                {translator('export.compatibilityLosses', {
                  count: result.outcome.compatibilityLosses.length,
                })}
              </p>
              <ul className="export-losses">
                {result.outcome.compatibilityLosses.map((loss) => (
                  <li key={`${loss.path}\u0000${loss.syntax}\u0000${loss.target}`}>
                    {translator(compatibilityKey(loss.syntax), {
                      path: loss.path,
                      target: loss.target,
                    })}
                  </li>
                ))}
              </ul>
            </>
          ) : null}
        </div>
      ) : null}
    </aside>
  );
}
