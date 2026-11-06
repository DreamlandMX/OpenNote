import { useEffect, useRef, useState } from 'react';
import { BookOpen, ChevronRight, FileText, GraduationCap, NotebookPen, X } from 'lucide-react';
import { useOverlayFocus } from './use-overlay-focus';
import type {
  DocumentLanguage,
  LatexMetadata,
  LatexTemplateId,
  NoteService,
  ScaffoldPlan,
} from '../editor';
import type { ServiceError } from '../domain';
import type { LatexProjectService } from '../latex-workspace/ports';
import { newOperationId } from './tauri-note-service';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import { localCalendarDate } from './regional';
import {
  defaultDestination,
  defaultMetadata,
  initialLanguage,
  issueKey,
  issueParams,
  paperOptions,
  planSummary,
  previewFiles,
} from './scaffold-view';

interface Props {
  /** The folder the project is created in; `''` or absent is the vault root. */
  parentFolder?: string;
  service: NoteService;
  latexService: LatexProjectService;
  vaultId: string;
  writable: boolean;
  translator: Translator;
  locale: SupportedLocaleId;
  /** Opens a created project's workspace. */
  onOpenProject(projectId: string): void;
  onClose(): void;
}

const TEMPLATES: readonly { id: LatexTemplateId; icon: typeof FileText }[] = [
  { id: 'report', icon: FileText },
  { id: 'paper', icon: GraduationCap },
  { id: 'assignment', icon: NotebookPen },
];

/**
 * S81 LaTeX project wizard.
 *
 * The plan comes from M13, so the preview is exactly what would be published. The
 * document language is chosen once and an interface change afterwards does not
 * replace it (F46-R07). Nothing is compiled, and a non-empty destination is
 * reported rather than overwritten (F46-R03). Creating writes exactly the
 * previewed files, each only where nothing exists, then registers the project.
 */
export function ProjectWizardPanel({ parentFolder = '', service, latexService, vaultId, writable, translator, locale, onOpenProject, onClose }: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [template, setTemplate] = useState<LatexTemplateId>('report');
  // The language is captured on first render; later UI changes do not replace it.
  const [language, setLanguage] = useState<DocumentLanguage>(
    initialLanguage(locale, null),
  );
  const [metadata, setMetadata] = useState<LatexMetadata>(() =>
    defaultMetadata(initialLanguage(locale, null), localCalendarDate(new Date())),
  );
  const [destination, setDestination] = useState('');
  const [plan, setPlan] = useState<ScaffoldPlan | null>(null);
  const [escaped, setEscaped] = useState<string | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [busy, setBusy] = useState(false);
  // One id per form state, so a retried create after a lost reply is idempotent.
  const [createOperation] = useState(newOperationId);
  // Problems sit below the form; bring them into view as they appear.
  const verdictRef = useRef<HTMLDivElement>(null);
  useEffect(() => { verdictRef.current?.scrollIntoView?.({ block: 'nearest' }); }, [plan]);

  const preview = plan === null ? null : previewFiles(plan);
  // Named after the title, inside the folder it was started from.
  const suggested = parentFolder === '' ? defaultDestination(metadata.title) : `${parentFolder}/${defaultDestination(metadata.title)}`;

  /** Any change makes an earlier check stale, so its verdict is dropped. */
  function changed(): void {
    setPlan(null);
    setError(null);
    setEscaped(null);
  }

  function request() {
    return {
      templateId: template,
      metadata: { ...metadata, language },
      destination: destination.trim() === '' ? suggested : destination.trim(),
    };
  }

  async function create(): Promise<void> {
    setBusy(true);
    setError(null);
    const planned = await service.planLatexScaffold(request());
    if (!planned.ok) {
      setBusy(false);
      setError(planned.error);
      return;
    }
    setPlan(planned.value);
    if (!planSummary(planned.value).executable) {
      setBusy(false);
      return;
    }
    const outcome = await latexService.createProject(vaultId, request(), createOperation);
    setBusy(false);
    if (!outcome.ok) {
      setError(outcome.error);
      return;
    }
    // The host re-plans against the real folder; show what it found if it stopped.
    setPlan(outcome.value.plan);
    const project = outcome.value.project;
    if (project !== null) onOpenProject(project.projectId);
  }

  return (
    <aside
      className="wizard-panel"
      aria-labelledby="wizard-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="wizard-head">
        <h2 id="wizard-heading">{translator('wizard.title')}</h2>
        <button
          type="button"
          className="wizard-close"
          aria-label={translator('wizard.close')}
          title={translator('wizard.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <fieldset className="wizard-templates">
        <legend>{translator('wizard.template')}</legend>
        {TEMPLATES.map(({ id, icon: Icon }) => (
          <label key={id} className="wizard-card" data-selected={template === id ? 'true' : undefined}>
            <input type="radio" name="wizard-template" checked={template === id} onChange={() => { setTemplate(id); changed(); }} />
            <Icon size={20} aria-hidden="true" />
            <strong>{translator(`wizard.template.${id}`)}</strong>
            <small>{translator(`wizard.template.${id}.hint`)}</small>
          </label>
        ))}
      </fieldset>

      <label className="wizard-title-field">
        <span>{translator('wizard.field.title')}</span>
        <input
          type="text"
          autoFocus
          value={metadata.title}
          placeholder={translator('wizard.titlePlaceholder')}
          onChange={(event) => { setMetadata({ ...metadata, title: event.target.value }); changed(); }}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.nativeEvent.isComposing && metadata.title.trim() !== '' && !busy) void create();
          }}
        />
      </label>

      <details className="wizard-more">
        <summary><ChevronRight className="wizard-chevron" size={14} aria-hidden="true" />{translator('wizard.moreOptions')}</summary>
        <div className="wizard-fields">
          <label>
            <span>{translator('wizard.field.author')}</span>
            <input type="text" value={metadata.author} onChange={(event) => { setMetadata({ ...metadata, author: event.target.value }); changed(); }} />
          </label>
          <label>
            <span>{translator('wizard.field.date')}</span>
            <input type="date" value={metadata.date} onChange={(event) => { setMetadata({ ...metadata, date: event.target.value }); changed(); }} />
          </label>
          <label>
            <span>{translator('wizard.field.language')}</span>
            <select value={language} onChange={(event) => { setLanguage(event.target.value as DocumentLanguage); changed(); }}>
              <option value="zhCn">{translator('wizard.language.zhCn')}</option>
              <option value="en">{translator('wizard.language.en')}</option>
            </select>
          </label>
          <label>
            <span>{translator('wizard.field.paper')}</span>
            <select
              value={metadata.paper}
              onChange={(event) => { setMetadata({ ...metadata, paper: event.target.value as LatexMetadata['paper'] }); changed(); }}
            >
              {paperOptions().map((option) => (
                <option key={option.value} value={option.value}>{translator(option.key)}</option>
              ))}
            </select>
          </label>
          <label>
            <span>{translator('wizard.field.destination')}</span>
            <input type="text" value={destination} placeholder={suggested}
              onChange={(event) => { setDestination(event.target.value); changed(); }} />
          </label>
        </div>
        <p className="wizard-note">{translator('wizard.languageNote')}</p>
        <button
          type="button"
          className="secondary"
          disabled={metadata.title.trim() === ''}
          onClick={() => {
            void service.escapeLatexText(metadata.title).then((outcome) => {
              setEscaped(outcome.ok ? outcome.value : null);
            });
          }}
        >
          {translator('wizard.showEscaped')}
        </button>
        {escaped !== null ? (
          <pre className="wizard-escaped" aria-label={translator('wizard.showEscaped')}>{escaped}</pre>
        ) : null}
      </details>

      {error !== null ? (
        <div className="wizard-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
        </div>
      ) : null}

      {plan !== null && plan.issues.length > 0 && preview !== null ? (
        <div ref={verdictRef} className="wizard-problems" role="alert">
          <ul className="wizard-issues" aria-label={translator('wizard.issues')}>
            {plan.issues.map((issue, index) => (
              <li key={`${issue.kind}:${index}`}>{translator(issueKey(issue), issueParams(issue))}</li>
            ))}
          </ul>
          <details className="wizard-files">
            <summary><ChevronRight className="wizard-chevron" size={14} aria-hidden="true" />{translator('wizard.files')}</summary>
            <ul>
              {preview.shown.map((path) => <li key={path} className="wizard-row">{path}</li>)}
            </ul>
            {preview.hidden > 0 ? <p className="wizard-note">{translator('wizard.more', { count: preview.hidden })}</p> : null}
          </details>
        </div>
      ) : null}

      <div className="wizard-controls">
        <button type="button" className="primary" disabled={busy || !writable || metadata.title.trim() === ''} onClick={() => void create()}>
          <BookOpen size={16} aria-hidden="true" /> {translator(busy ? 'wizard.creating' : 'wizard.create')}
        </button>
        <span className="wizard-note">{translator(metadata.title.trim() === '' ? 'wizard.needTitle' : 'wizard.createHint')}</span>
      </div>
      {!writable ? <p className="wizard-note">{translator('workspace.vault.readOnly')}</p> : null}
    </aside>
  );
}
