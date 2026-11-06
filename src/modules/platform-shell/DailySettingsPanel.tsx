import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import { Settings2, X } from 'lucide-react';
import type { ServiceError } from '../domain';
import type { NoteService } from '../editor';
import type { Translator, SupportedLocaleId } from '../presentation-i18n';
import { serviceErrorMessage } from '../presentation-i18n';
import { toChoices, type TemplateChoice } from './template-view';
import type { DailySettings } from './device-prefs';
import {
  DEFAULT_DAILY_FOLDER_PATTERN,
  dailySettingsDraft,
  dailyTemplateMissing,
  draftToDailySettings,
} from './daily-settings';

interface Props {
  service: NoteService;
  initial: DailySettings;
  translator: Translator;
  locale: SupportedLocaleId;
  onSave(settings: DailySettings): void;
  onClose(): void;
}

/**
 * F17 daily folder/template settings.
 *
 * The stored template id is checked against the current template list before it
 * is saved again. A template that was deleted is therefore reported here
 * instead of silently producing a blank note later (F17 §4/§7). The host still
 * validates the pattern and remains authoritative.
 */
export function DailySettingsPanel({ service, initial, translator, locale, onSave, onClose }: Props) {
  const draft = dailySettingsDraft(initial);
  const [folderPattern, setFolderPattern] = useState(draft.folderPattern);
  const [templateId, setTemplateId] = useState(draft.templateId);
  const [templates, setTemplates] = useState<readonly TemplateChoice[] | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [problem, setProblem] = useState<'invalid' | 'unsafe' | null>(null);
  const openerRef = useRef<Element | null>(null);
  const patternRef = useRef<HTMLInputElement | null>(null);
  const serviceRef = useRef(service);
  serviceRef.current = service;

  useEffect(() => {
    openerRef.current = document.activeElement;
    patternRef.current?.focus();
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);

  useEffect(() => {
    let active = true;
    void serviceRef.current.listTemplates().then(
      (outcome) => {
        if (!active) {
          return;
        }
        if (outcome.ok) {
          setTemplates(toChoices(outcome.value));
          setError(null);
        } else {
          setError(outcome.error);
        }
      },
      () => {
        if (active) {
          setLoadFailed(true);
        }
      },
    );
    return () => {
      active = false;
    };
  }, []);

  const available = templates ?? [];
  const missing = templates !== null && dailyTemplateMissing(templateId, available);
  const selectedValue = available.some((template) => template.noteId === templateId)
    ? templateId
    : '';

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    // A deleted template must not be written back: the warning promises a
    // blank note, and keeping the stale id would silently blank it later too.
    const result = draftToDailySettings({
      folderPattern,
      templateId: missing ? '' : templateId,
    });
    if (result.settings === null) {
      setProblem(result.problem);
      return;
    }
    setProblem(null);
    onSave(result.settings);
  }

  function closeOnEscape(event: KeyboardEvent<HTMLElement>): void {
    if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
      event.preventDefault();
      onClose();
    }
  }

  return (
    <aside
      className="daily-settings-panel"
      role="dialog"
      aria-labelledby="daily-settings-heading"
      onKeyDown={closeOnEscape}
    >
      <div className="daily-settings-head">
        <h2 id="daily-settings-heading">
          <Settings2 size={18} aria-hidden="true" />
          {translator('daily.settings')}
        </h2>
        <button
          type="button"
          className="daily-settings-close"
          aria-label={translator('daily.cancel')}
          title={translator('daily.cancel')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </div>
      <form className="daily-settings-form" onSubmit={submit}>
        <label htmlFor="daily-settings-pattern">{translator('daily.folderPattern')}</label>
        <input
          id="daily-settings-pattern"
          ref={patternRef}
          type="text"
          value={folderPattern}
          maxLength={240}
          autoComplete="off"
          spellCheck={false}
          placeholder={DEFAULT_DAILY_FOLDER_PATTERN}
          aria-describedby="daily-settings-pattern-hint"
          aria-invalid={problem !== null ? 'true' : undefined}
          onChange={(event) => {
            setFolderPattern(event.currentTarget.value);
            setProblem(null);
          }}
        />
        <p id="daily-settings-pattern-hint" className="daily-settings-hint">
          {translator('daily.folderPatternHint')}
        </p>
        {problem !== null ? (
          <p className="daily-settings-error" role="alert">
            {translator(problem === 'unsafe' ? 'daily.patternUnsafe' : 'daily.patternInvalid')}
          </p>
        ) : null}

        <label htmlFor="daily-settings-template">{translator('daily.template')}</label>
        <select
          id="daily-settings-template"
          value={selectedValue}
          disabled={templates === null || error !== null || loadFailed}
          aria-busy={templates === null ? 'true' : undefined}
          onChange={(event) => setTemplateId(event.currentTarget.value)}
        >
          <option value="">{translator('daily.templateNone')}</option>
          {available.map((template) => (
            <option key={template.noteId} value={template.noteId}>
              {template.title === '' ? template.relativePath : template.title}
            </option>
          ))}
        </select>
        {missing ? (
          <p className="daily-settings-warning" role="status">
            {translator('daily.templateMissing')}
          </p>
        ) : null}
        {error !== null ? (
          <p className="daily-settings-error" role="alert">
            {serviceErrorMessage(locale, error.code)}
          </p>
        ) : null}
        {loadFailed ? (
          <p className="daily-settings-error" role="alert">
            {translator('dialog.error.fallback')}
          </p>
        ) : null}

        <div className="daily-settings-actions">
          <button type="button" className="secondary" onClick={onClose}>
            {translator('daily.cancel')}
          </button>
          <button
            type="submit"
            className="primary"
            disabled={templates === null || error !== null || loadFailed}
          >
            {translator('daily.save')}
          </button>
        </div>
      </form>
    </aside>
  );
}
