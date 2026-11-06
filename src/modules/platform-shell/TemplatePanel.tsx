import { useEffect, useMemo, useState } from 'react';
import { FilePlus2, LayoutTemplate, Plus } from 'lucide-react';
import { PanelHeader } from './PanelHeader';
import { useOverlayFocus } from './use-overlay-focus';
import type { NoteService, TemplateVariables } from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import { formatCalendarDate, localCalendarDate } from './regional';
import { currentTemplatePreview, observeTemplatePreview, insertable, summarizeWarnings, toChoices, variablesFor, type TemplateChoice, type TemplatePreviewSnapshot } from './template-view';
import { isCalendarDay } from './daily-view';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  region: string;
  /** Title proposed for the new note; the user may change it. */
  suggestedTitle: string;
  writable: boolean;
  canInsert: boolean;
  onInsert(text: string, caret: number): boolean;
  onCreateNote(title: string, text: string): void;
  onClose(): void;
}

/**
 * S33/S34 template picker and preview.
 *
 * Expansion runs in M06; only the preview bound to the current inputs can be
 * confirmed, so the exact reviewed fragment is inserted. Unknown
 * variables are listed and the fragment keeps them verbatim, never blanked
 * (F18-R01).
 */
export function TemplatePanel({
  service,
  translator,
  locale,
  region,
  suggestedTitle,
  writable,
  canInsert,
  onInsert,
  onCreateNote,
  onClose,
}: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [choices, setChoices] = useState<readonly TemplateChoice[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [title, setTitle] = useState(suggestedTitle);
  const [preview, setPreview] = useState<TemplatePreviewSnapshot | null>(null);
  const [insertFailed, setInsertFailed] = useState(false);
  const [error, setError] = useState<ServiceError | null>(null);
  const [loading, setLoading] = useState(false);

  const [date, setDate] = useState(() => localCalendarDate(new Date()));
  const active = choices.find((choice) => choice.noteId === selected) ?? choices[0] ?? null;
  const variables: TemplateVariables = useMemo(() => variablesFor(title, date, null), [title, date]);
  const validDate = isCalendarDay(date);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void service.listTemplates().then((outcome) => {
      if (cancelled) {
        return;
      }
      setLoading(false);
      if (outcome.ok) {
        setChoices(toChoices(outcome.value));
        setError(null);
      } else {
        setError(outcome.error);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [service]);

  useEffect(() => {
    if (active === null || !validDate) return;
    return observeTemplatePreview(service, active, variables, setPreview);
  }, [active, service, variables, validDate]);

  const previewOutcome = validDate ? currentTemplatePreview(preview, service, active, variables) : null;
  const expansion = previewOutcome?.ok ? previewOutcome.value : null;
  const previewError = previewOutcome?.ok === false ? previewOutcome.error : error;
  const warnings = expansion === null ? null : summarizeWarnings(expansion.warnings);
  const ready = expansion !== null && validDate;

  return (
    <aside
      className="template-panel"
      aria-labelledby="template-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <PanelHeader id="template-heading" icon={LayoutTemplate} title={translator('templates.title')}
        purpose={translator('templates.purpose')} closeLabel={translator('templates.close')} onClose={onClose} />

      {loading || (active !== null && validDate && previewOutcome === null) ? <p role="status">{translator('templates.loading')}</p> : null}
      {previewError !== null ? (
        <div className="template-error" role="alert">
          <p>{serviceErrorMessage(locale, previewError.code)}</p>
        </div>
      ) : null}

      {!loading && choices.length === 0 && error === null ? (
        <div className="panel-empty template-empty" role="status">
          <strong>{translator('templates.emptyTitle')}</strong>
          <p>{translator('templates.emptyBody')}</p>
          <pre className="panel-code" aria-label={translator('templates.exampleLabel')}>{templateExample(translator)}</pre>
          <TemplateVariablesNote translator={translator} />
          {writable ? (
            <div>
              <button type="button" className="panel-tool" onClick={() => onCreateNote(translator('templates.exampleTitle'), templateExample(translator))}>
                <Plus size={15} aria-hidden="true" />{translator('templates.useExample')}
              </button>
            </div>
          ) : null}
        </div>
      ) : null}

      {choices.length > 0 ? (
        <div className="template-layout">
          <ul className="template-list" aria-label={translator('templates.listLabel')}>
            {choices.map((choice) => (
              <li key={choice.noteId}>
                <button
                  type="button"
                  className="template-choice"
                  aria-current={active?.noteId === choice.noteId ? 'true' : undefined}
                  onClick={() => setSelected(choice.noteId)}
                >
                  <span>{choice.title}</span>
                  {choice.summary !== '' ? <small>{choice.summary}</small> : null}
                </button>
              </li>
            ))}
          </ul>

          <div className="template-preview">
            <label className="template-title">
              <span>{translator('templates.noteTitle')}</span>
              <input
                type="text"
                value={title}
                disabled={!writable}
                onChange={(event) => setTitle(event.target.value)}
              />
            </label>
            <label className="template-date">
              <span>{translator('templates.date')}</span>
              <input type="date" value={date} disabled={!writable} onChange={event => setDate(event.target.value)} />
            </label>
            {!validDate ? <p role="alert">{serviceErrorMessage(locale, 'INVALID_INPUT')}</p> : null}
            <p className="template-date">
              {translator('templates.dateNote', {
                date: formatCalendarDate(date, region, 'medium'),
              })}
            </p>

            {expansion !== null ? (
              <>
                <pre className="template-fragment" aria-label={translator('templates.preview')}>
                  {expansion.fragment}
                </pre>
                {warnings !== null && warnings.unknown.length > 0 ? (
                  <p className="template-warning" role="status">
                    {translator('templates.unknown', { names: warnings.unknown.join(', ') })}
                  </p>
                ) : null}
                {warnings !== null && warnings.unterminated ? (
                  <p className="template-warning" role="status">
                    {translator('templates.unterminated')}
                  </p>
                ) : null}
              </>
            ) : null}

            {(!canInsert || insertFailed) ? <p role="status">{translator('templates.stale')}</p> : null}
            <div className="template-actions">
              <button
                type="button"
                className="primary"
                disabled={!ready || !writable || !canInsert}
                onClick={() => {
                  if (!ready || !expansion || !writable || !canInsert) return;
                  const { text, caret } = insertable(expansion);
                  setInsertFailed(!onInsert(text, caret));
                }}
              >
                <FilePlus2 size={15} aria-hidden="true" />
                {translator('templates.insert')}
              </button>
              <button
                type="button"
                className="secondary"
                disabled={!ready || !writable || title.trim() === ''}
                onClick={() => {
                  if (!ready || !expansion || !writable || title.trim() === '') return;
                  onCreateNote(title, expansion.fragment);
                }}
              >
                {translator('templates.newNote')}
              </button>
              <button type="button" className="secondary" onClick={onClose}>
                {translator('templates.close')}
              </button>
            </div>
            <p className="template-hint">{translator('templates.hint')}</p>
            <TemplateVariablesNote translator={translator} />
          </div>
        </div>
      ) : null}
    </aside>
  );
}

/**
 * A template a new user can start from. It is the raw template, variables and
 * all, so the note it creates is itself a template.
 */
export function templateExample(translator: Translator): string {
  return `---
template: true
---
# {{title}}

${translator('templates.exampleDate')}: {{date}}

{{cursor}}
`;
}

/** The variables a template may use, named once in the user's language. */
function TemplateVariablesNote({ translator }: { translator: Translator }) {
  return (
    <p className="panel-note">
      {translator('templates.variables')}{' '}
      <code>{'{{title}}'}</code> {translator('templates.var.title')} · <code>{'{{date}}'}</code> {translator('templates.var.date')}
      {' · '}<code>{'{{time}}'}</code> {translator('templates.var.time')} · <code>{'{{cursor}}'}</code> {translator('templates.var.cursor')}
    </p>
  );
}
