import { useEffect, useState } from 'react';
import { Columns2, Rows2 } from 'lucide-react';
import type { Translator } from '../presentation-i18n';
import { readDevicePreferences, writeDevicePreferences } from './device-prefs';
import {
  closePane,
  focusOtherPane,
  focusPane,
  initialLayout,
  isNarrow,
  MAX_PANES,
  sharesNote,
  splitPane,
  toggleOrientation,
  visibleColumns,
  type PaneLayout,
} from './panes';

interface Props {
  translator: Translator;
  /** The note the active editor pane currently shows. */
  noteId: string | null;
  /**
   * F04: the workspace renders the panes this component owns. Reporting every
   * layout change keeps the controls and the editors on one source of truth
   * without moving the device-only layout state into the document model.
   */
  onLayoutChange?: (layout: PaneLayout) => void;
  /** F04: a pane header asked to become the active editor. */
  focusRequest?: { paneId: string; nonce: number } | null;
}

/**
 * F04 split-pane controls.
 *
 * The layout is device state, so it lives here rather than in the document. The
 * workspace renders one editor per pane from `onLayoutChange`; both editors share
 * the same buffer and save controller, and only the active pane is editable, so
 * two views never become two writers for one file. Each pane keeps its own
 * CodeMirror selection and undo history (F04-R01).
 */
export function SplitControls({ translator, noteId, onLayoutChange, focusRequest }: Props) {
  const [layout, setLayout] = useState<PaneLayout>(() => {
    const base = initialLayout(noteId);
    // F04-R04: layout is device state and is not required sync data, so it is
    // restored from local preferences rather than from a note or a synced record.
    const stored = readDevicePreferences().splitOrientation;
    return stored === null ? base : { ...base, orientation: stored };
  });
  const [width, setWidth] = useState(() =>
    typeof window === 'undefined' ? 1280 : window.innerWidth,
  );

  // The active note follows the editor, and both panes stay on that note:
  // F04-R01 is multiple views of one note, never two different files in one
  // split. Opening a note in either pane therefore keeps the pair together.
  useEffect(() => {
    if (noteId !== null && layout.panes.some(pane => pane.noteId !== noteId)) {
      setLayout(current => ({
        ...current,
        panes: current.panes.map(pane => ({ ...pane, noteId })),
      }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [noteId]);

  useEffect(() => {
    function onResize(): void {
      setWidth(window.innerWidth);
    }
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  useEffect(() => {
    if (focusRequest !== null && focusRequest !== undefined) {
      setLayout(current => focusPane(current, focusRequest.paneId));
    }
    // The nonce is the trigger; the same pane may be requested twice.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusRequest?.nonce]);

  useEffect(() => {
    onLayoutChange?.(layout);
  }, [layout, onLayoutChange]);

  const narrow = isNarrow(width);
  const columns = visibleColumns(layout, width);
  const shared = sharesNote(layout);

  return (
    <div className="split-controls" role="group" aria-label={translator('split.label')}>
      <button
        type="button"
        className="split-button"
        aria-pressed={layout.panes.length > 1 ? 'true' : 'false'}
        title={translator('split.split')}
        onClick={() => setLayout((current) => splitPane(current, noteId))}
      >
        <Columns2 size={16} aria-hidden="true" />
        {translator('split.split')}
      </button>
      <button
        type="button"
        className="split-button"
        title={translator('split.orientation')}
        onClick={() =>
          setLayout((current) => {
            const next = toggleOrientation(current);
            // Persist the choice so reopening the app keeps the direction.
            writeDevicePreferences({ splitOrientation: next.orientation });
            return next;
          })
        }
      >
        {layout.orientation === 'horizontal' ? (
          <Rows2 size={16} aria-hidden="true" />
        ) : (
          <Columns2 size={16} aria-hidden="true" />
        )}
        {layout.orientation === 'horizontal'
          ? translator('split.orientation.horizontal')
          : translator('split.orientation.vertical')}
      </button>
      <button
        type="button"
        className="split-button"
        disabled={layout.panes.length < 2}
        title={translator('split.focusOther')}
        onClick={() => setLayout((current) => focusOtherPane(current))}
      >
        {translator('split.focusOther')}
      </button>
      <button
        type="button"
        className="split-button"
        disabled={layout.panes.length < 2}
        title={translator('split.close')}
        onClick={() => setLayout((current) => closePane(current, current.activePaneId))}
      >
        {translator('split.close')}
      </button>
      <span className="split-status" role="status">
        {narrow
          ? translator('split.merged', { columns, total: layout.panes.length })
          : translator('split.active', {
              columns,
              total: layout.panes.length,
              max: MAX_PANES,
            })}
        {layout.panes.length > 1 && shared ? ` · ${translator('split.sharedNote')}` : ''}
      </span>
      {/* Stating the active-pane rule is better than letting a click in the
          read-only pane look broken. */}
      {layout.panes.length > 1 ? (
        <span className="split-note">{translator('split.notEditable')}</span>
      ) : null}
      <span className="split-panes">
        {layout.panes.map((pane, index) => (
          <button
            key={pane.paneId}
            type="button"
            className="split-pane-chip"
            data-active={pane.active || undefined}
            aria-pressed={pane.active}
            title={translator('split.focusOther')}
            onClick={() => setLayout((current) => focusPane(current, pane.paneId))}
          >
            {translator('split.pane', { index: index + 1 })}
          </button>
        ))}
      </span>
    </div>
  );
}
