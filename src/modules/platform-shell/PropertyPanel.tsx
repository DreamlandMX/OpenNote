import { useEffect, useMemo, useRef, useState } from 'react';
import { Plus, X } from 'lucide-react';
import type { MessageKey, Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import type { NoteService, PropertyValue } from '../editor';
import type { ServiceError } from '../domain';
import {
  displayValue,
  incompatibleValues,
  isEditableName,
  kindOfHostValue,
  parseInput,
  toPropertyValue,
  type HostProperty,
  type PropertyKind,
  type ValueParse,
} from './properties';

interface Props {
  service: NoteService;
  noteId: string;
  /** Live buffer source; the panel never reads the saved file behind the editor. */
  source: string;
  translator: Translator;
  locale: SupportedLocaleId;
  readOnly: boolean;
  /** Applies the rewritten source to the editor buffer; it does not save. */
  onApply(previousSource: string, nextSource: string, label: string): void;
  onClose(): void;
}

type FieldErrorKey = (typeof ERROR_KEYS)[keyof typeof ERROR_KEYS];
const GENERIC_FIELD_ERROR: FieldErrorKey = 'properties.empty';

const KINDS: readonly PropertyKind[] = [
  'text',
  'number',
  'boolean',
  'date',
  'dateTime',
  'select',
  'multiSelect',
  'link',
];

const KIND_LABEL_KEYS: Record<PropertyKind, MessageKey> = {
  text: 'properties.kind.text',
  number: 'properties.kind.number',
  boolean: 'properties.kind.boolean',
  date: 'properties.kind.date',
  dateTime: 'properties.kind.dateTime',
  select: 'properties.kind.select',
  multiSelect: 'properties.kind.multiSelect',
  link: 'properties.kind.link',
  unset: 'properties.kind.unset',
};

const ERROR_KEYS: Record<string, MessageKey> = {
  invalidNumber: 'properties.error.number',
  invalidBoolean: 'properties.error.boolean',
  invalidDate: 'properties.error.date',
  invalidDateTime: 'properties.error.dateTime',
  invalidLink: 'properties.error.link',
  empty: 'properties.empty',
};

/**
 * S16 property editor. Every edit is one targeted frontmatter rewrite computed
 * by the host; the panel then hands the whole new source to the editor buffer,
 * so saving stays in the normal path with its revision and operation checks
 * (F09 §6). Unknown fields and their order are preserved by the host writer
 * (F09-A02), and a value the chosen type cannot parse is reported instead of
 * being cleared (F09-R04).
 */
export function PropertyPanel({
  service,
  noteId,
  source,
  translator,
  locale,
  readOnly,
  onApply,
  onClose,
}: Props) {
  const [properties, setProperties] = useState<readonly HostProperty[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ServiceError | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [kind, setKind] = useState<PropertyKind>('text');
  const [fieldError, setFieldError] = useState<FieldErrorKey | null>(null);
  const [newName, setNewName] = useState('');
  const [newKind, setNewKind] = useState<PropertyKind>('text');
  const [warning, setWarning] = useState<readonly string[]>([]);
  const openerRef = useRef<Element | null>(null);
  const sessionRef = useRef(0);

  const fields = useMemo(
    () => properties.filter((property) => property.name.toLowerCase() !== 'opennote-id'),
    [properties],
  );

  async function load(): Promise<void> {
    const session = ++sessionRef.current;
    setLoading(true);
    // Parses the live buffer, not the saved file, so an unsaved frontmatter
    // edit is visible immediately and a save is still the only writer.
    const outcome = await service.parseProperties(source);
    if (session !== sessionRef.current) {
      return;
    }
    setLoading(false);
    if (!outcome.ok) {
      setError(outcome.error);
      return;
    }
    setError(null);
    setProperties(outcome.value);
  }

  useEffect(() => {
    openerRef.current = document.activeElement;
    void load();
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [noteId, service, source]);

  function beginEdit(property: HostProperty): void {
    setEditing(property.name);
    setDraft(displayValue(property.value));
    setKind(kindOfHostValue(property.value));
    setFieldError(null);
    setWarning([]);
  }

  async function commit(name: string, value: PropertyValue): Promise<void> {
    const outcome = await service.applyPropertyEdit(source, name, value);
    if (!outcome.ok) {
      setError(outcome.error);
      return;
    }
    setError(null);
    if (!outcome.value.changed) {
      setEditing(null);
      return;
    }
    onApply(source, outcome.value.source, `${name}`);
    setEditing(null);
  }

  async function commitTyped(): Promise<void> {
    if (editing === null) {
      return;
    }
    const parsed: ValueParse = parseInput(kind, draft);
    if (!parsed.ok) {
      setFieldError(ERROR_KEYS[parsed.reason] ?? GENERIC_FIELD_ERROR);
      return;
    }
    setFieldError(null);
    await commit(editing, parsed.value);
  }

  async function changeKind(next: PropertyKind): Promise<void> {
    if (editing === null) {
      setKind(next);
      return;
    }
    const current = fields.find((property) => property.name === editing);
    const values = current === undefined ? [] : [displayValue(current.value)];
    const blocked = incompatibleValues(kind, next, values);
    setWarning(blocked);
    setKind(next);
    if (blocked.length === 0) {
      setDraft(values[0] ?? '');
    }
  }

  async function addProperty(): Promise<void> {
    const name = newName.trim();
    if (!isEditableName(name)) {
      setFieldError('properties.error.name');
      return;
    }
    setFieldError(null);
    await commit(name, toPropertyValue({ text: '' }));
    setNewName('');
  }

  return (
    <aside
      className="property-panel"
      aria-labelledby="property-heading"
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="property-head">
        <h2 id="property-heading">{translator('properties.title')}</h2>
        <button
          type="button"
          className="property-close"
          aria-label={translator('properties.close')}
          title={translator('properties.close')}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>

      <p className="property-status" role="status">
        {loading ? translator('properties.loading') : null}
      </p>
      {error !== null ? (
        <div className="property-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
          <p>{translator('properties.yamlPaused')}</p>
        </div>
      ) : null}

      {!loading && fields.length === 0 && error === null ? (
        <p className="property-empty" role="status">
          {translator('properties.empty')}
        </p>
      ) : null}

      {fields.length > 0 ? (
        <ul className="property-list">
          {fields.map((property) => {
            const currentKind = kindOfHostValue(property.value);
            return (
              <li key={property.name} className="property-row">
                <div className="property-label">
                  <span>{property.name}</span>
                  <small>{translator(KIND_LABEL_KEYS[currentKind])}</small>
                  {property.unknown ? (
                    <small className="property-unknown">{translator('properties.unknown')}</small>
                  ) : null}
                </div>
                {editing === property.name ? (
                  <div className="property-editor">
                    <label>
                      <span className="visually-hidden">{translator('properties.value')}</span>
                      <input
                        autoFocus
                        type="text"
                        value={draft}
                        aria-invalid={fieldError !== null ? 'true' : undefined}
                        aria-describedby={fieldError !== null ? 'property-field-error' : undefined}
                        onChange={(event) => setDraft(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
                            event.preventDefault();
                            void commitTyped();
                          }
                        }}
                      />
                    </label>
                    <label>
                      <span>{translator('properties.type')}</span>
                      <select
                        value={kind}
                        onChange={(event) => void changeKind(event.target.value as PropertyKind)}
                      >
                        {KINDS.map((option) => (
                          <option key={option} value={option}>
                            {translator(KIND_LABEL_KEYS[option])}
                          </option>
                        ))}
                      </select>
                    </label>
                    {fieldError !== null ? (
                      <p id="property-field-error" className="property-error-text" role="alert">
                        {translator(fieldError)}
                      </p>
                    ) : null}
                    {warning.length > 0 ? (
                      <p className="property-warning" role="status">
                        {translator('properties.conversionBlocked', { values: warning.join(', ') })}
                      </p>
                    ) : null}
                    <div className="property-actions">
                      <button
                        type="button"
                        className="primary"
                        disabled={readOnly}
                        onClick={() => void commitTyped()}
                      >
                        {translator('properties.save')}
                      </button>
                      <button
                        type="button"
                        className="secondary"
                        onClick={() => {
                          setEditing(null);
                          setFieldError(null);
                          setWarning([]);
                        }}
                      >
                        {translator('properties.cancel')}
                      </button>
                    </div>
                  </div>
                ) : (
                  <button
                    type="button"
                    className="property-value"
                    disabled={readOnly}
                    title={readOnly ? translator('workspace.vault.readOnly') : undefined}
                    onClick={() => beginEdit(property)}
                  >
                    <span>{displayValue(property.value)}</span>
                    {property.value === 'unset' ? (
                      <small>{translator('properties.unset')}</small>
                    ) : null}
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      ) : null}

      {!readOnly ? (
        <form
          className="property-add"
          onSubmit={(event) => {
            event.preventDefault();
            void addProperty();
          }}
        >
          <h3>{translator('properties.add')}</h3>
          <label>
            <span>{translator('properties.name')}</span>
            <input
              type="text"
              value={newName}
              onChange={(event) => setNewName(event.target.value)}
            />
          </label>
          <label>
            <span>{translator('properties.type')}</span>
            <select value={newKind} onChange={() => setNewKind('text')}>
              <option value="text">{translator(KIND_LABEL_KEYS.text)}</option>
            </select>
          </label>
          <button type="submit" className="secondary">
            <Plus size={14} aria-hidden="true" />
            {translator('properties.add')}
          </button>
        </form>
      ) : (
        <p className="property-hint">{translator('workspace.vault.readOnly')}</p>
      )}

      <p className="property-hint">{translator('properties.unknownHint')}</p>
    </aside>
  );
}
