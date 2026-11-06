import { useCallback, useEffect, useId, useMemo, useReducer, useRef, useState } from 'react';
import { ArrowRightLeft, Download, FileText, FolderOpen, LayoutDashboard, MoreHorizontal, Plus, Redo2, RotateCcw, Save, SaveAll, TextCursorInput, Trash2, Undo2 } from 'lucide-react';
import { PanelHeader, PanelSteps } from './PanelHeader';
import { WorkspaceActions } from './WorkspaceActions';
import { useOverlayFocus } from './use-overlay-focus';
import { usePanelExitGuard, type RegisterPanelGuard } from './panel-guards';
import { newOperationId } from './tauri-note-service';
import type { CanvasDocument, NoteService } from '../editor';
import type { ServiceError } from '../domain';
import type { MessageKey, MessageParams, Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import {
  arrowDelta,
  beginDrag,
  canvasPathFromName,
  canvasSaveState,
  commitDrag,
  connectNodes,
  connectionPaths,
  dragRect,
  exportSummary,
  listItems,
  moveNode,
  pickConnection,
  newTextNode,
  reassociateNode,
  removeNodeLocally,
  reduceCanvasHistory,
  type CanvasNoteChoice,
  type DragState,
  type Rect,
} from './canvas-view';

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** Notes that exist, so a broken card can be labelled and repaired. */
  knownNotes: readonly CanvasNoteChoice[];
  /** Lists unsaved work in the window's exit and vault-switch warning. */
  registerExitGuard?: RegisterPanelGuard;
  onClose(): void;
}

const EMPTY_CANVAS: CanvasDocument = { formatVersion: '1.0', nodes: [], edges: [] };

/**
 * S30/S31 canvas.
 *
 * The document is validated by M06. Canvases are `.canvas` files in the vault:
 * opened by path and saved with a compare-and-swap on the hash they were loaded
 * with, so a file edited elsewhere is reported as a conflict instead of being
 * overwritten. Closing with unsaved changes needs a second, deliberate close.
 */
export function CanvasPanel({ service, translator, locale, knownNotes, registerExitGuard, onClose }: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [history, dispatch] = useReducer(reduceCanvasHistory, { document: EMPTY_CANVAS, past: [], future: [] });
  const document = history.document;
  const setDocument = useCallback((change: (current: CanvasDocument) => CanvasDocument): void => {
    dispatch({ type: 'edit', change });
  }, []);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [preview, setPreview] = useState<Rect | null>(null);
  const activePointer = useRef<number | null>(null);
  const arrowId = useId();
  const [error, setError] = useState<ServiceError | null>(null);
  const [status, setStatus] = useState<{ key: MessageKey; params?: MessageParams } | null>(null);
  const [text, setText] = useState('');
  const [referenceId, setReferenceId] = useState('');
  // Connect mode: the first card clicked is the start, the second the end.
  const [connecting, setConnecting] = useState(false);
  const [connectStart, setConnectStart] = useState<string | null>(null);
  // The one input strip shown under the toolbar, if any.
  const [drawer, setDrawer] = useState<'text' | 'note' | 'saveAs' | 'open' | null>(null);
  const [exportText, setExportText] = useState<string | null>(null);
  const [files, setFiles] = useState<readonly string[]>([]);
  const [file, setFile] = useState<{ path: string; sha256: string } | null>(null);
  const [savedDocument, setSavedDocument] = useState<CanvasDocument>(EMPTY_CANVAS);
  const [newName, setNewName] = useState('');
  const [openChoice, setOpenChoice] = useState('');
  const [saving, setSaving] = useState(false);
  const [conflict, setConflict] = useState(false);
  const [closeArmed, setCloseArmed] = useState(false);
  const dirty = document !== savedDocument;
  const saveState = canvasSaveState(file, dirty, saving);

  const refreshFiles = useCallback(async (): Promise<void> => {
    const listed = await service.listCanvases();
    if (listed.ok) setFiles(listed.value);
  }, [service]);
  useEffect(() => { void refreshFiles(); }, [refreshFiles]);

  const openFile = useCallback(async (path: string): Promise<void> => {
    if (dirty && !closeArmed) {
      setStatus({ key: 'canvas.file.discardFirst' });
      setCloseArmed(true);
      return;
    }
    const loaded = await service.readCanvas(path);
    if (!loaded.ok) {
      setError(loaded.error);
      return;
    }
    dispatch({ type: 'load', document: loaded.value.document });
    setSavedDocument(loaded.value.document);
    setFile({ path: loaded.value.relativePath, sha256: loaded.value.sha256 });
    setConflict(false);
    setCloseArmed(false);
    setError(null);
    setStatus({ key: 'canvas.file.opened', params: { name: loaded.value.relativePath } });
  }, [service, dirty, closeArmed]);

  const save = useCallback(async (target: string, expected: string | null): Promise<boolean> => {
    if (saving) return false;
    setSaving(true);
    const snapshot = document;
    const result = await service.saveCanvas(target, snapshot, expected, newOperationId());
    setSaving(false);
    if (!result.ok) {
      if (result.error.code === 'VERSION_CONFLICT') {
        setConflict(true);
        setStatus({ key: 'canvas.file.conflict' });
      } else {
        setError(result.error);
      }
      return false;
    }
    // Only the saved snapshot is clean; edits made while saving stay dirty.
    setSavedDocument(snapshot);
    setFile({ path: result.value.relativePath, sha256: result.value.sha256 });
    setConflict(false);
    setCloseArmed(false);
    setError(null);
    setStatus({ key: 'canvas.file.saved', params: { name: result.value.relativePath } });
    void refreshFiles();
    return true;
  }, [saving, document, service, refreshFiles]);

  // An exported rescue copy stops the exit warning until the next edit.
  const [rescuedDocument, setRescuedDocument] = useState<CanvasDocument | null>(null);
  usePanelExitGuard(registerExitGuard, 'canvas', {
    active: dirty && document !== rescuedDocument,
    label: file?.path ?? translator('canvas.file.untitled'),
    status: saving ? 'saving' : conflict ? 'conflict' : 'dirty',
    save: () => (file === null || conflict ? Promise.resolve(false) : save(file.path, file.sha256)),
    draft: () => ({
      relativePath: file?.path ?? 'untitled.canvas',
      source: `${JSON.stringify(document, null, 2)}
`,
    }),
    rescued: () => setRescuedDocument(document),
  });

  // Reloading after a conflict deliberately replaces local edits with the file.
  const reloadFromDisk = useCallback(async (path: string): Promise<void> => {
    const loaded = await service.readCanvas(path);
    if (!loaded.ok) {
      setError(loaded.error);
      return;
    }
    dispatch({ type: 'load', document: loaded.value.document });
    setSavedDocument(loaded.value.document);
    setFile({ path: loaded.value.relativePath, sha256: loaded.value.sha256 });
    setConflict(false);
    setCloseArmed(false);
    setStatus({ key: 'canvas.file.reloaded' });
  }, [service]);

  // Any new edit re-arms the unsaved-changes warning.
  useEffect(() => { setCloseArmed(false); }, [document]);

  const requestClose = useCallback((): void => {
    if (dirty && !closeArmed) {
      setCloseArmed(true);
      setStatus({ key: 'canvas.file.unsavedClose' });
      return;
    }
    onClose();
  }, [dirty, closeArmed, onClose]);

  // A drag ends on pointerup anywhere, so the handler lives on the window rather
  // than on the node: releasing outside a card must still finish the drag.
  useEffect(() => {
    if (drag === null) {
      return;
    }
    const cancelDrag = (): void => {
      activePointer.current = null;
      setDrag(null);
      setPreview(null);
    };
    const move = (event: PointerEvent): void => {
      if (event.pointerId === activePointer.current) {
        setPreview(dragRect(drag, event.clientX, event.clientY));
      }
    };
    const finish = (event: PointerEvent): void => {
      if (event.pointerId !== activePointer.current) return;
      setDocument((current) =>
        commitDrag(current, drag, dragRect(drag, event.clientX, event.clientY)),
      );
      cancelDrag();
    };
    const pointerCancel = (event: PointerEvent): void => {
      if (event.pointerId === activePointer.current) cancelDrag();
    };
    const cancel = (event: KeyboardEvent): void => {
      if (event.key === 'Escape' && !event.isComposing) {
        event.preventDefault();
        cancelDrag();
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', finish);
    window.addEventListener('pointercancel', pointerCancel);
    window.addEventListener('blur', cancelDrag);
    window.addEventListener('keydown', cancel);
    return () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', finish);
      window.removeEventListener('pointercancel', pointerCancel);
      window.removeEventListener('blur', cancelDrag);
      window.removeEventListener('keydown', cancel);
    };
  }, [drag, setDocument]);

  const knownNoteIds = useMemo(
    () => knownNotes.map((note) => note.noteId),
    [knownNotes],
  );
  const items = useMemo(() => listItems(document, knownNoteIds), [document, knownNoteIds]);
  const labels = useMemo(() => new Map(items.map((item) => [item.id, item.label])), [items]);
  const connections = useMemo(() => connectionPaths(document,
    drag !== null && preview !== null ? { nodeId: drag.nodeId, rect: preview } : null), [document, drag, preview]);
  const summary = exportSummary(document);

  const addText = useCallback((): void => {
    if (text.trim() === '') {
      setStatus({ key: 'canvas.textNeeded' });
      return;
    }
    setStatus(null);
    const id = newOperationId();
    setDocument((current) =>
      ({
        ...current,
        nodes: [
          ...current.nodes,
          newTextNode(current, id, text.trim()),
        ],
      }) as CanvasDocument,
    );
    setText('');
  }, [text, setDocument]);

  const addReference = useCallback(
    (choice: CanvasNoteChoice): void => {
      const id = newOperationId();
      setDocument((current) =>
        ({
          ...current,
          nodes: [
            ...current.nodes,
            {
              ...newTextNode(current, id, ''),
              type: 'note',
              noteId: choice.noteId,
              // The label is kept with the card so a later missing target still
              // leaves a readable card (F21-A03).
              label: choice.title,
              text: null,
            },
          ],
        }) as CanvasDocument,
      );
      setStatus(null);
    },
    [setDocument],
  );

  const reassociate = useCallback(
    (nodeId: string, label: string, noteId: string): void => {
      setDocument((current) => reassociateNode(current, nodeId, noteId));
      setStatus({ key: 'canvas.reassociated', params: { name: label } });
    },
    [setDocument],
  );

  // In connect mode a card is a target, not something to drag: the first one
  // picked is the start and the next distinct one ends the connection.
  const pickForConnection = useCallback((nodeId: string): void => {
    const next = pickConnection(connectStart, nodeId);
    if (next.connect !== null) {
      const id = newOperationId();
      const { from, to } = next.connect;
      setDocument((current) => connectNodes(current, id, from, to));
    }
    setConnectStart(next.start);
  }, [connectStart, setDocument]);

  const stopConnecting = useCallback((): void => {
    setConnecting(false);
    setConnectStart(null);
  }, []);

  async function exportCanvas(): Promise<void> {
    const outcome = await service.serializeCanvas(document);
    if (!outcome.ok) {
      setError(outcome.error);
      return;
    }
    setError(null);
    setExportText(outcome.value);
    setStatus({ key: 'canvas.exported' });
  }

  return (
    <aside
      className="canvas-panel"
      aria-labelledby="canvas-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        const target = event.target;
        const editingText = target instanceof HTMLElement
          && (target.isContentEditable || target.closest('input, textarea, select') !== null);
        if (drag === null && !editingText && !event.nativeEvent.isComposing
          && (event.ctrlKey || event.metaKey) && !event.altKey) {
          const key = event.key.toLowerCase();
          if (key === 'z' || key === 'y') {
            event.preventDefault();
            event.stopPropagation();
            dispatch({ type: key === 'y' || event.shiftKey ? 'redo' : 'undo' });
            return;
          }
        }
        if (event.key === 'Escape' && !event.nativeEvent.isComposing && drag === null) {
          event.preventDefault();
          if (connecting) stopConnecting();
          else if (drawer !== null) setDrawer(null);
          else requestClose();
        }
      }}
    >
      <PanelHeader
        id="canvas-heading"
        icon={LayoutDashboard}
        title={translator('canvas.title')}
        purpose={translator('canvas.purpose')}
        closeLabel={translator('canvas.close')}
        onClose={requestClose}
        status={
          <span className="canvas-file" role="group" aria-label={translator('canvas.file.label')}>
            <span className="canvas-file-name">{file?.path ?? translator('canvas.file.untitled')}</span>
            <span className={`panel-badge canvas-file-state canvas-file-${saveState}`} role="status">
              {translator(`canvas.file.state.${saveState}`)}
            </span>
          </span>
        }
      />

      <div className="panel-toolbar canvas-toolbar" role="toolbar" aria-label={translator('canvas.toolbar')}>
        <button type="button" className="panel-tool" aria-pressed={drawer === 'text' ? 'true' : 'false'}
          onClick={() => setDrawer(current => (current === 'text' ? null : 'text'))}>
          <TextCursorInput size={15} aria-hidden="true" />{translator('canvas.tool.text')}
        </button>
        <button type="button" className="panel-tool" aria-pressed={drawer === 'note' ? 'true' : 'false'} disabled={knownNotes.length === 0}
          onClick={() => setDrawer(current => (current === 'note' ? null : 'note'))}>
          <FileText size={15} aria-hidden="true" />{translator('canvas.tool.note')}
        </button>
        <button type="button" className="panel-tool" aria-pressed={connecting ? 'true' : 'false'} disabled={!connecting && document.nodes.length < 2}
          onClick={() => {
            if (connecting) {
              stopConnecting();
            } else {
              setConnecting(true);
              setDrawer(null);
            }
          }}>
          <ArrowRightLeft size={15} aria-hidden="true" />{translator('canvas.tool.connect')}
        </button>
        <button type="button" className="panel-tool icon-only" disabled={drag !== null || history.past.length === 0}
          aria-label={translator('canvas.undo')} title={translator('canvas.undo')}
          aria-keyshortcuts="Control+z Meta+z" onClick={() => dispatch({ type: 'undo' })}>
          <Undo2 size={15} aria-hidden="true" />
        </button>
        <button type="button" className="panel-tool icon-only" disabled={drag !== null || history.future.length === 0}
          aria-label={translator('canvas.redo')} title={translator('canvas.redo')}
          aria-keyshortcuts="Control+Shift+z Meta+Shift+z Control+y" onClick={() => dispatch({ type: 'redo' })}>
          <Redo2 size={15} aria-hidden="true" />
        </button>
        <span className="spacer" />
        <button type="button" className="panel-tool" disabled={saving || (file !== null && (!dirty || conflict))}
          onClick={() => {
            if (file !== null) void save(file.path, file.sha256);
            else setDrawer('saveAs');
          }}>
          <Save size={15} aria-hidden="true" />{translator('canvas.file.save')}
        </button>
        <WorkspaceActions label={translator('canvas.more')} triggerClassName="panel-tool icon-only"
          icon={<MoreHorizontal size={15} aria-hidden="true" />}>
          <button type="button" className="secondary menu-row" onClick={() => setDrawer('saveAs')}>
            <SaveAll size={15} aria-hidden="true" />{translator(file === null ? 'canvas.file.saveAs' : 'canvas.file.saveCopy')}
          </button>
          {files.length > 0 ? (
            <button type="button" className="secondary menu-row" onClick={() => setDrawer('open')}>
              <FolderOpen size={15} aria-hidden="true" />{translator('canvas.file.open')}
            </button>
          ) : null}
          <button type="button" className="secondary menu-row" onClick={() => void exportCanvas()}>
            <Download size={15} aria-hidden="true" />{translator('canvas.export')}
          </button>
          {conflict && file !== null ? (
            <button type="button" className="secondary menu-row" onClick={() => void reloadFromDisk(file.path)}>
              <RotateCcw size={15} aria-hidden="true" />{translator('canvas.file.reload')}
            </button>
          ) : null}
        </WorkspaceActions>
      </div>

      {drawer === 'text' ? (
        <div className="canvas-drawer">
          <label className="canvas-text">
            <span className="visually-hidden">{translator('canvas.newText')}</span>
            <input
              type="text"
              autoFocus
              value={text}
              placeholder={translator('canvas.textPlaceholder')}
              onChange={(event) => setText(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  addText();
                }
              }}
            />
          </label>
          <button type="button" className="panel-tool" onClick={addText}>
            <Plus size={15} aria-hidden="true" /> {translator('canvas.addCard')}
          </button>
        </div>
      ) : null}
      {drawer === 'note' ? (
        <div className="canvas-drawer">
          <label>
            <span className="visually-hidden">{translator('canvas.referenceFrom')}</span>
            <select autoFocus value={referenceId} onChange={(event) => setReferenceId(event.target.value)}>
              <option value="">{translator('canvas.chooseNote')}</option>
              {knownNotes.map((note) => (
                <option key={note.noteId} value={note.noteId}>{note.title}</option>
              ))}
            </select>
          </label>
          <button
            type="button"
            className="panel-tool"
            disabled={referenceId === ''}
            onClick={() => {
              const choice = knownNotes.find((note) => note.noteId === referenceId);
              if (choice !== undefined) {
                addReference(choice);
              }
              setReferenceId('');
            }}
          >
            <Plus size={15} aria-hidden="true" /> {translator('canvas.addReference')}
          </button>
        </div>
      ) : null}
      {drawer === 'saveAs' ? (
        <div className="canvas-drawer">
          <label className="canvas-text">
            <span className="visually-hidden">{translator('canvas.file.newName')}</span>
            <input type="text" autoFocus value={newName} placeholder={translator('canvas.file.newName')}
              onChange={(event) => setNewName(event.target.value)} />
          </label>
          <button
            type="button"
            className="panel-tool"
            disabled={saving || canvasPathFromName(newName) === null}
            onClick={() => {
              const target = canvasPathFromName(newName);
              if (target !== null) {
                void save(target, null);
                setDrawer(null);
              }
            }}
          >
            <Save size={15} aria-hidden="true" /> {translator(file === null ? 'canvas.file.saveAs' : 'canvas.file.saveCopy')}
          </button>
        </div>
      ) : null}
      {drawer === 'open' ? (
        <div className="canvas-drawer">
          <label>
            <span className="visually-hidden">{translator('canvas.file.open')}</span>
            <select
              autoFocus
              value={openChoice}
              onChange={(event) => {
                setOpenChoice(event.target.value);
                if (event.target.value !== '') {
                  void openFile(event.target.value);
                  setDrawer(null);
                }
              }}
            >
              <option value="">{translator('canvas.choose')}</option>
              {files.map((path) => <option key={path} value={path}>{path}</option>)}
            </select>
          </label>
        </div>
      ) : null}
      {connecting ? (
        <p className="canvas-connect-hint" role="status">
          {connectStart === null
            ? translator('canvas.connectPickStart')
            : translator('canvas.connectPickEnd', { name: labels.get(connectStart) ?? connectStart })}
        </p>
      ) : null}

      {status !== null ? (
        <p className="canvas-status" role="status">
          {translator(status.key, status.params)}
        </p>
      ) : null}
      {error !== null ? (
        <div className="canvas-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
        </div>
      ) : null}

      <div
        className="canvas-surface"
        dir="ltr"
        role="group"
        aria-label={translator('canvas.surfaceLabel')}
        data-connecting={connecting ? 'true' : undefined}
      >
        {connections.paths.length > 0 ? (
          <svg className="canvas-edges" width={connections.width} height={connections.height} aria-hidden="true" focusable="false">
            <defs>
              <marker id={arrowId} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto">
                <path d="M 0 0 L 10 5 L 0 10 Z" fill="currentColor" />
              </marker>
            </defs>
            {connections.paths.map((edge) => (
              <path className="canvas-edge" key={edge.id} data-edge-id={edge.id} d={edge.path} markerEnd={`url(#${arrowId})`} />
            ))}
          </svg>
        ) : null}
        {document.nodes.length === 0 ? (
          <div className="canvas-empty">
            <PanelSteps steps={[translator('canvas.step.add'), translator('canvas.step.move'), translator('canvas.step.connect')]} />
          </div>
        ) : (
          document.nodes.map((node) => {
            const label = node.label ?? node.text ?? node.noteId ?? node.id;
            const rect = drag?.nodeId === node.id && preview !== null ? preview : node;
            return (
              <div
                key={node.id}
                className="canvas-node"
                dir="auto"
                role="group"
                tabIndex={0}
                aria-label={label}
                aria-keyshortcuts="ArrowUp ArrowDown ArrowLeft ArrowRight"
                data-kind={node.type}
                data-connect-start={connectStart === node.id ? 'true' : undefined}
                style={{
                  // JSON Canvas x/y are physical offsets; logical insets would
                  // mirror the whole canvas under RTL.
                  left: rect.x,
                  top: rect.y,
                  width: rect.width,
                  height: rect.height,
                }}
                onKeyDown={(event) => {
                  // Arrow keys are the keyboard alternative to dragging (F30-R03);
                  // during a pointer drag or an IME composition they stay inert.
                  if (drag !== null || event.nativeEvent.isComposing) {
                    return;
                  }
                  if (connecting && (event.key === 'Enter' || event.key === ' ')) {
                    event.preventDefault();
                    pickForConnection(node.id);
                    return;
                  }
                  const delta = arrowDelta(event.key, event.shiftKey);
                  if (delta === null) {
                    return;
                  }
                  event.preventDefault();
                  setDocument((current) =>
                    moveNode(current, node.id, delta.dx, delta.dy),
                  );
                }}
                onClick={() => { if (connecting) pickForConnection(node.id); }}
                onPointerDown={(event) => {
                  if (connecting) return;
                  if (event.button !== 0 || !event.isPrimary || activePointer.current !== null) {
                    return;
                  }
                  event.currentTarget.setPointerCapture(event.pointerId);
                  activePointer.current = event.pointerId;
                  setDrag(beginDrag(node, event.clientX, event.clientY));
                  setPreview(node);
                }}
                onLostPointerCapture={(event) => {
                  if (event.pointerId === activePointer.current) {
                    activePointer.current = null;
                    setDrag(null);
                    setPreview(null);
                  }
                }}
              >
                <span className="canvas-node-label">
                  {node.label ?? node.text ?? node.noteId ?? node.id}
                </span>
              </div>
            );
          })
        )}
      </div>

      <ul className="canvas-list" aria-label={translator('canvas.listLabel')}>
        {items.map((item) => (
          <li key={item.id} className="canvas-row" data-broken={item.broken ? 'true' : undefined}>
            <span className="canvas-row-label">{item.label}</span>
            <span className="canvas-row-meta">
              {item.kind}
              {item.reference !== null ? ` · ${item.reference}` : ''}
              {item.broken ? ` · ${translator('canvas.broken')}` : ''}
            </span>
            {item.broken ? (
              <select
                className="canvas-reassociate"
                aria-label={translator('canvas.reassociate', { name: item.label })}
                defaultValue=""
                disabled={knownNotes.length === 0}
                onChange={(event) => {
                  if (event.target.value === '') {
                    return;
                  }
                  reassociate(item.id, item.label, event.target.value);
                }}
              >
                <option value="">{translator('canvas.reassociateChoose')}</option>
                {knownNotes.map((note) => (
                  <option key={note.noteId} value={note.noteId}>{note.title}</option>
                ))}
              </select>
            ) : null}
            <button
              type="button"
              className="canvas-remove"
              aria-label={translator('canvas.removeCard', { name: item.label })}
              title={translator('canvas.removeCard', { name: item.label })}
              onClick={() => setDocument((current) => removeNodeLocally(current, item.id))}
            >
              <Trash2 size={14} aria-hidden="true" />
            </button>
          </li>
        ))}
      </ul>

      {document.edges.length > 0 ? (
        <>
          <p className="canvas-connections-title">{translator('canvas.connectionsLabel')}</p>
          <ul className="canvas-connections" aria-label={translator('canvas.connectionsLabel')}>
            {document.edges.map((edge) => (
              <li key={edge.id}>
                <span>{translator('canvas.connection', { from: labels.get(edge.from) ?? edge.from, to: labels.get(edge.to) ?? edge.to })}</span>
                {edge.label ? <span className="canvas-connection-label">{edge.label}</span> : null}
              </li>
            ))}
          </ul>
        </>
      ) : null}

      {exportText !== null ? (
        <>
          <p className="canvas-note">{translator('canvas.exportNote', { format: 'JSON Canvas', version: summary.formatVersion })}</p>
          <pre className="canvas-export" aria-label={translator('canvas.exportPreview')}>
            {exportText}
          </pre>
          <button type="button" className="secondary" onClick={() => setExportText(null)}>
            {translator('canvas.hideExport')}
          </button>
        </>
      ) : null}

      <p className="canvas-summary" role="status">
        {translator('canvas.summary', { nodes: summary.nodes, edges: summary.edges })}
      </p>
      <p className="panel-note">{translator('canvas.footer')}</p>
    </aside>
  );
}
