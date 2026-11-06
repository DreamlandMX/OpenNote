import type { MessageKey, Translator } from '../presentation-i18n';
import { visibleSlashItems, type FragmentKind, type SlashItem } from './insert';

interface Props {
  /** Query typed after `/`, owned by the editor. */
  query: string;
  selectedIndex: number;
  translator: Translator;
  onSelect(item: SlashItem): void;
  onHover(index: number): void;
}

export const SLASH_LABEL_KEYS: Record<FragmentKind, MessageKey> = {
  text: 'insert.text',
  heading: 'insert.heading',
  task: 'insert.task',
  table: 'insert.table',
  equation: 'insert.equation',
  code: 'insert.code',
  link: 'insert.link',
};

export function slashLabels(translator: Translator): Record<FragmentKind, string> {
  return Object.fromEntries(
    (Object.keys(SLASH_LABEL_KEYS) as FragmentKind[]).map((kind) => [
      kind,
      translator(SLASH_LABEL_KEYS[kind]),
    ]),
  ) as Record<FragmentKind, string>;
}

/**
 * S11 slash menu. Presentation only: the editor owns the `/query` range and the
 * keyboard (arrows, digits, Enter, Escape) and applies the insertion as one
 * transaction, so one undo restores the original text (F06-A03) and an IME
 * composition never triggers it.
 */
export function SlashMenu({ query, selectedIndex, translator, onSelect, onHover }: Props) {
  const labels = slashLabels(translator);
  const items = visibleSlashItems(query, labels);
  const activeIndex = Math.min(selectedIndex, Math.max(0, items.length - 1));

  return (
    <div className="slash-menu" role="presentation">
      <div
        className="slash-dialog"
        role="dialog"
        aria-label={translator('insert.title')}
        aria-describedby="slash-hint"
      >
        <p className="slash-heading">{translator('insert.title')}</p>
        {items.length === 0 ? (
          <p className="slash-empty" role="status">
            {translator('insert.empty')}
          </p>
        ) : (
          <ul className="slash-list" role="listbox" aria-label={translator('insert.title')}>
            {items.map((item, index) => (
              <li
                key={item.kind}
                data-slash={item.kind}
                role="option"
                aria-selected={index === activeIndex ? 'true' : 'false'}
                className="slash-row"
                onMouseEnter={() => onHover(index)}
                onClick={() => onSelect(item)}
              >
                <span>{labels[item.kind]}</span>
                <kbd>{item.digit}</kbd>
              </li>
            ))}
          </ul>
        )}
        <p className="slash-hint" id="slash-hint">
          {translator('insert.hint')}
        </p>
      </div>
    </div>
  );
}
