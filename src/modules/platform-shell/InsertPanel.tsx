import { useEffect, useRef, useState } from 'react';
import { X } from 'lucide-react';
import type {
  AssistApplier,
  InsertKind,
  InsertParameters,
  InsertionPlan,
  NoteService,
} from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  canInsert,
  DEFAULT_IMAGE_WIDTH,
  dependencyInsertion,
  insertionEdits,
  insertionPointConflicts,
  isInsertionStale,
  issuesFor,
  missingDependencyLines,
  parseTableInput,
  sameInsertParameters,
  suggestLabel,
} from './insertion-form';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** The buffer the insertion targets; labels and preamble come from it. */
  source: string;
  /** The editor selection the fragment replaces, if the pane reported one. */
  selection: { from: number; to: number } | null;
  /** Applies all edits in one editor transaction, so they undo together. */
  apply: AssistApplier;
  onClose(): void;
}

const KINDS: readonly InsertKind[] = ['equation', 'image', 'table', 'section', 'reference'];

/**
 * S73/S74 structured insertion form.
 *
 * The plan comes from M13, so a missing package is reported with the exact line to
 * add instead of being inserted silently (F48-R03), and a duplicate label is named
 * before anything is written. Package lines are added only after the user checks
 * the box; they and the fragment go through one editor transaction so one undo
 * restores the note (F48-R04). A formula field is inserted as TeX while a title is
 * escaped as plain text (F48-R01).
 */
export function InsertPanel({
  service,
  translator,
  locale,
  source,
  selection,
  apply,
  onClose,
}: Props) {
  const [kind, setKind] = useState<InsertKind>('section');
  const [body, setBody] = useState('');
  const [imagePath, setImagePath] = useState('');
  const [width, setWidth] = useState(DEFAULT_IMAGE_WIDTH);
  const [rows, setRows] = useState('');
  const [advanced, setAdvanced] = useState('');
  const [title, setTitle] = useState('');
  const [level, setLevel] = useState(1);
  const [label, setLabel] = useState('');
  const [caption, setCaption] = useState('');
  const [plan, setPlan] = useState<InsertionPlan | null>(null);
  const [planSource, setPlanSource] = useState<string | null>(null);
  const [planParameters, setPlanParameters] = useState<InsertParameters | null>(null);
  // F48-R04: a package change is only applied after this box is checked.
  const [includeDependencies, setIncludeDependencies] = useState(false);
  const [labels, setLabels] = useState<readonly string[]>([]);
  const [error, setError] = useState<ServiceError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const openerRef = useRef<Element | null>(null);
  // A slower preview must not overwrite the plan of a newer one.
  const previewRequest = useRef(0);
  // F48 §8: the overlay takes focus so Escape works, then hands it back to its trigger.
  useEffect(() => {
    openerRef.current = document.activeElement;
    headingRef.current?.focus();
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);
  // F48 §7 STALE_INSERT_POINT: the previewed source may no longer be the note.
  const sourceStale = isInsertionStale(planSource, source);
  // §8: the preview and the form must still describe the same insertion.
  const formStale = planParameters !== null && !sameInsertParameters(planParameters, parameters());
  const stale = sourceStale || formStale;
  const fragmentStart = selection?.from ?? source.length;
  const fragmentEnd = selection?.to ?? source.length;
  const missing = missingDependencyLines(plan);
  const dependencyEdit =
    plan !== null && canInsert(plan) ? dependencyInsertion(source, missing) : null;
  const dependenciesAvailable =
    dependencyEdit !== null &&
    !insertionPointConflicts(dependencyEdit.start, fragmentStart, fragmentEnd);

  function parameters(): InsertParameters {
    const labelValue = label.trim() === '' ? null : label.trim();
    switch (kind) {
      case 'equation':
        return { kind: 'equation', body, label: labelValue };
      case 'image':
        return {
          kind: 'image',
          path: imagePath,
          width,
          caption: caption.trim() === '' ? null : caption,
          label: labelValue,
        };
      case 'table':
        return {
          kind: 'table',
          rows: parseTableInput(rows),
          advanced: advanced.trim() === '' ? null : advanced,
          caption: caption.trim() === '' ? null : caption,
          label: labelValue,
        };
      case 'section':
        return { kind: 'section', title, level, label: labelValue };
      case 'reference':
      default:
        return { kind: 'reference', label: label.trim() };
    }
  }

  async function preview(): Promise<void> {
    const request = ++previewRequest.current;
    const previewed = source;
    const attempted = parameters();
    const outcome = await service.planLatexInsertion(attempted, previewed);
    if (request !== previewRequest.current) {
      return;
    }
    if (!outcome.ok) {
      setPlan(null);
      setPlanSource(null);
      setPlanParameters(null);
      setIncludeDependencies(false);
      setError(outcome.error);
      return;
    }
    setPlan(outcome.value);
    setPlanSource(previewed);
    setPlanParameters(attempted);
    setIncludeDependencies(false);
    setError(null);
    setNotice(null);
    const found = await service.latexLabels(previewed);
    if (request !== previewRequest.current) {
      return;
    }
    setLabels(found.ok ? found.value : []);
  }

  function insert(): void {
    if (plan === null || !canInsert(plan)) {
      return;
    }
    if (stale) {
      setPlan(null);
      setPlanSource(null);
      setPlanParameters(null);
      setIncludeDependencies(false);
      setNotice(translator(sourceStale ? 'latexInsert.stale' : 'latexInsert.formStale'));
      return;
    }
    const withDependencies = includeDependencies && dependenciesAvailable;
    // F48-R04: both edits go through one editor transaction, so one undo
    // restores the note even when the preamble changed.
    const edits = insertionEdits(
      source,
      plan.fragment,
      { from: fragmentStart, to: fragmentEnd },
      missing,
      withDependencies,
    );
    if (edits === null) {
      setNotice(translator('latexInsert.failed'));
      return;
    }
    if (apply(edits)) {
      setNotice(translator(withDependencies ? 'latexInsert.doneWithDependencies' : 'latexInsert.done'));
      setPlan(null);
      setPlanSource(null);
      setPlanParameters(null);
      setIncludeDependencies(false);
    } else {
      setNotice(translator('latexInsert.failed'));
    }
  }

  return (
    <aside
      className="insert-panel"
      aria-labelledby="insert-heading"
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="insert-head">
        <h2 id="insert-heading" ref={headingRef} tabIndex={-1}>
          {translator('latexInsert.title')}
        </h2>
        <button
          type="button"
          className="insert-close"
          aria-label={translator('latexInsert.close')}
          title={translator('latexInsert.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <p className="insert-body">{translator('latexInsert.body')}</p>

      <fieldset className="insert-kinds">
        <legend>{translator('latexInsert.kind')}</legend>
        {KINDS.map((option) => (
          <label key={option} className="theme-option">
            <input
              type="radio"
              name="insert-kind"
              checked={kind === option}
              onChange={() => {
                setKind(option);
                setPlan(null);
                setPlanSource(null);
                setPlanParameters(null);
                setIncludeDependencies(false);
                setNotice(null);
              }}
            />
            {translator(`latexInsert.kind.${option}`)}
          </label>
        ))}
      </fieldset>

      <div className="insert-fields">
        {kind === 'equation' ? (
          <label>
            <span>{translator('latexInsert.field.body')}</span>
            <textarea
              rows={3}
              spellCheck={false}
              value={body}
              onChange={(event) => setBody(event.target.value)}
            />
            <small>{translator('latexInsert.field.bodyHint')}</small>
            {issuesFor(plan, 'body').map((issue) => (
              <small key={issue.key} className="insert-issue" role="alert">
                {translator('latexInsert.issue.missing', issue.params)}
              </small>
            ))}
          </label>
        ) : null}

        {kind === 'image' ? (
          <>
            <label>
              <span>{translator('latexInsert.field.path')}</span>
              <input
                type="text"
                value={imagePath}
                placeholder="assets/figure.png"
                onChange={(event) => setImagePath(event.target.value)}
              />
              {issuesFor(plan, 'path').map((issue) => (
                <small key={issue.key} className="insert-issue" role="alert">
                  {translator('latexInsert.issue.unsafePath', issue.params)}
                </small>
              ))}
            </label>
            <label>
              <span>{translator('latexInsert.field.width')}</span>
              <input
                type="range"
                min={0.1}
                max={1}
                step={0.05}
                value={width}
                onChange={(event) => setWidth(Number(event.target.value))}
                aria-valuetext={`${Math.round(width * 100)}%`}
              />
              <output>{Math.round(width * 100)}%</output>
              {issuesFor(plan, 'width').map((issue) => (
                <small key={issue.key} className="insert-issue" role="alert">
                  {translator('latexInsert.issue.invalidWidth', issue.params)}
                </small>
              ))}
            </label>
            <label>
              <span>{translator('latexInsert.field.caption')}</span>
              <input
                type="text"
                value={caption}
                onChange={(event) => setCaption(event.target.value)}
              />
            </label>
          </>
        ) : null}

        {kind === 'table' ? (
          <>
            <label>
              <span>{translator('latexInsert.field.rows')}</span>
              <textarea
                rows={4}
                spellCheck={false}
                value={rows}
                placeholder={'a | b\n1 | 2'}
                onChange={(event) => setRows(event.target.value)}
              />
              <small>{translator('latexInsert.field.rowsHint')}</small>
              {issuesFor(plan, 'rows').map((issue) => (
                <small key={issue.key} className="insert-issue" role="alert">
                  {translator('latexInsert.issue.malformedTable', issue.params)}
                </small>
              ))}
            </label>
            <label>
              <span>{translator('latexInsert.field.advanced')}</span>
              <textarea
                rows={3}
                spellCheck={false}
                value={advanced}
                onChange={(event) => setAdvanced(event.target.value)}
              />
              <small>{translator('latexInsert.field.advancedHint')}</small>
            </label>
            <label>
              <span>{translator('latexInsert.field.caption')}</span>
              <input
                type="text"
                value={caption}
                onChange={(event) => setCaption(event.target.value)}
              />
            </label>
          </>
        ) : null}

        {kind === 'section' ? (
          <>
            <label>
              <span>{translator('latexInsert.field.title')}</span>
              <input
                type="text"
                value={title}
                onChange={(event) => setTitle(event.target.value)}
              />
              {issuesFor(plan, 'title').map((issue) => (
                <small key={issue.key} className="insert-issue" role="alert">
                  {translator('latexInsert.issue.missing', issue.params)}
                </small>
              ))}
            </label>
            <label>
              <span>{translator('latexInsert.field.level')}</span>
              <select value={level} onChange={(event) => setLevel(Number(event.target.value))}>
                <option value={1}>{translator('latexInsert.level.1')}</option>
                <option value={2}>{translator('latexInsert.level.2')}</option>
                <option value={3}>{translator('latexInsert.level.3')}</option>
              </select>
              {issuesFor(plan, 'level').map((issue) => (
                <small key={issue.key} className="insert-issue" role="alert">
                  {translator('latexInsert.issue.invalidLevel', issue.params)}
                </small>
              ))}
            </label>
          </>
        ) : null}

        {kind !== 'reference' ? (
          <label>
            <span>{translator('latexInsert.field.label')}</span>
            <input
              type="text"
              value={label}
              placeholder={suggestLabel(kind, labels)}
              onChange={(event) => setLabel(event.target.value)}
            />
            {issuesFor(plan, 'label').map((issue) => (
              <small key={issue.key} className="insert-issue" role="alert">
                {translator('latexInsert.issue.invalidLabel', issue.params)}
              </small>
            ))}
          </label>
        ) : (
          <label>
            <span>{translator('latexInsert.field.reference')}</span>
            <select value={label} onChange={(event) => setLabel(event.target.value)}>
              <option value="">{translator('latexInsert.choose')}</option>
              {labels.map((existing) => (
                <option key={existing} value={existing}>
                  {existing}
                </option>
              ))}
            </select>
            {issuesFor(plan, 'label').map((issue) => (
              <small key={issue.key} className="insert-issue" role="alert">
                {translator('latexInsert.issue.unknownLabel', issue.params)}
              </small>
            ))}
          </label>
        )}
      </div>

      <div className="insert-controls">
        <button type="button" className="secondary" onClick={() => void preview()}>
          {translator('latexInsert.preview')}
        </button>
        <button type="button" className="primary" disabled={!canInsert(plan) || stale} onClick={insert}>
          {translator('latexInsert.apply')}
        </button>
      </div>

      {error !== null ? (
        <div className="insert-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
        </div>
      ) : null}
      {notice !== null ? (
        <p className="insert-notice" role="status">
          {notice}
        </p>
      ) : null}
      {sourceStale ? (
        <p className="insert-notice" role="status">
          {translator('latexInsert.stale')}
        </p>
      ) : null}
      {formStale ? (
        <p className="insert-notice" role="status">
          {translator('latexInsert.formStale')}
        </p>
      ) : null}

      {plan !== null ? (
        <>
          <pre className="insert-fragment" aria-label={translator('latexInsert.fragment')}>
            {plan.fragment === '' ? translator('latexInsert.blocked') : plan.fragment}
          </pre>
          {missing.length > 0 ? (
            <section className="insert-deps" aria-labelledby="insert-deps-heading">
              <h3 id="insert-deps-heading">{translator('latexInsert.dependencies')}</h3>
              <p>
                {translator(
                  dependenciesAvailable
                    ? 'latexInsert.dependenciesHint'
                    : 'latexInsert.dependenciesManual',
                )}
              </p>
              <ul>
                {missing.map((line) => (
                  <li key={line}>
                    <code>{line}</code>
                  </li>
                ))}
              </ul>
              {dependenciesAvailable ? (
                <label className="insert-dep-confirm">
                  <input
                    type="checkbox"
                    checked={includeDependencies}
                    onChange={(event) => setIncludeDependencies(event.target.checked)}
                  />
                  {translator('latexInsert.includeDependencies')}
                </label>
              ) : null}
            </section>
          ) : null}
        </>
      ) : null}

      <p className="insert-note">{translator('latexInsert.hint')}</p>
    </aside>
  );
}
