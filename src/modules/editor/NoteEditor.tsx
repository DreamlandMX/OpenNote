import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from 'react';
import { Compartment, EditorState, StateEffect, StateField, Transaction, type Extension } from '@codemirror/state';
import { EditorView, keymap, placeholder } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, isolateHistory } from '@codemirror/commands';
import { liveMarkdownSyntaxExtensions } from './markdown-language';
import { liveImageHost, liveInteractions, mathPreviewEffect, mathResultsField } from './live-preview';
import { pdfLinkTarget, rewriteImageEmbeds, type AttachmentHost } from './attachment-images';
import { DEFAULT_FIND_KEYS, findReplace, type FindKeys } from './find-replace';
import { ReadFindBar } from './ReadFindBar';
import { ImageViewer } from './ImageViewer';
import { formulaKey, type FormulaInput, type FormulaNotice } from './math-render';
import { useFormulaPreview } from './use-formula-preview';
import { EquationDialog, formulaErrorText } from './EquationDialog';
import 'katex/dist/katex.min.css';
import { LinkPicker } from './LinkPicker';
import { SlashMenu, slashLabels } from './SlashMenu';
import { EditorContextMenu, type ContextMenuEntry } from './EditorContextMenu';
import {
  clearInline,
  convertBlock,
  externalLink,
  internalLink,
  plainText,
  toggleInline,
  type BlockKind,
  type Edit,
  type InlineMarker,
} from './format';
import {
  Bold, ClipboardPaste, ClipboardType, Code, CodeXml, Copy, Eraser, ExternalLink, Heading1, Heading2,
  Heading3, Highlighter, Italic, Link, List, ListOrdered, ListPlus, ListTodo, Paintbrush, Percent, Pilcrow, Quote, Scissors,
  Sigma, SquareCode, SquareDashed, Strikethrough, Table, Text, Type,
} from 'lucide-react';
import {
  bodySafeSelection,
  clampSelection,
  decideModeSwitch,
  type EditorSelection,
  type ModeSnapshot,
} from './mode-switch';
import { readPaneTarget, type HeadingAnchor } from './read-anchor';
import { EXTERNAL_REPLACE_EVENT, externalChange, externalReplacement } from './external-sync';
import {
  codeInsertion,
  detectSlashQuery,
  equationInsertion,
  headingInsertion,
  linkInsertion,
  rankSlashItems,
  tableInsertion,
  taskInsertion,
  textInsertion,
  type Insertion,
  type Selection,
  type SlashItem,
} from './insert';
import {
  anchorTargetFor as pickerAnchorTargetFor,
  applyAnchorInsertion,
  applyLinkInsertion,
  detectLinkQuery,
  type AnchorCandidate,
  type LinkCandidate,
  type LinkQuery,
} from './link-picker';
import type { EmbedTree, EmbedNode } from './embed-resolution';
import { renderSafeMarkdown, renderWithEmbeds, splitFrontMatter, markdownFormulas, type RenderedEmbed } from './markdown-render';
import { embedBlock, embedUnavailable, scanEmbeds, type EmbedSpan } from './embed-render';
import type { MessageKey, SupportedLocaleId, Translator } from '../presentation-i18n';
import { dirForLocale } from '../presentation-i18n';
import './ui/editor.css';

export type EditorMode = 'live' | 'source' | 'read';

export function initialEditorPosition(source: string, mode: EditorMode): number {
  if (mode === 'source') return 0;
  const { frontMatter, body } = splitFrontMatter(source);
  return frontMatter === null ? 0 : source.slice(0, source.length - body.length).replace(/\r\n?/g, '\n').length;
}

/** Applies computed edits, given as ranges over the current document text. */
export type AssistApplier = (edits: readonly { start: number; end: number; text: string }[], target?: {
  noteId: string;
  source: string;
  selection: { from: number; to: number };
  caret: number;
}) => boolean;

export interface NoteEditorProps {
  noteId: string;
  pathName: string;
  source: string;
  docVersion: number;
  readOnly: boolean;
  mode: EditorMode;
  locale: SupportedLocaleId;
  translator: Translator;
  onDocChanged: (noteId: string, text: string) => void;
  onExplicitSave: (noteId: string) => void;
  onCompositionChange: (noteId: string, composing: boolean) => void;
  /** Find and replace keys from Settings; Ctrl/Cmd+F and +H when omitted. */
  findKeys?: FindKeys;
  /**
   * Opens the source note of an inline embed. Without it the read overlay still
   * shows the source path, but the row is not actionable.
   */
  onOpenEmbed?: (noteId: string) => void;
  /** Ctrl/Cmd+click on a Live `[[target]]`; receives the target without `#anchor` or `|label`. */
  onFollowLink?: (target: string) => void;
  /** S17 link targets. Omitted means the picker offers nothing. */
  linkCandidates?: readonly LinkCandidate[];
  /**
   * Anchors of a note, used once the user types `#` in the picker. Returns the
   * headings and block ids of the note that owns `relativePath`.
   */
  anchorsOf?: (noteId: string) => readonly AnchorCandidate[];
  /**
   * Resolved inline embeds, keyed `target#anchor`. Read mode inlines whatever is
   * present; `null` means the expansion request failed, and a missing entry
   * still renders its loading placeholder.
   */
  embedExpansions?: EmbedTree;
  /**
   * F12 jump target: UTF-16 offset of the first search hit. The effect runs
   * once per `revealNonce` change, so editing does not pull the caret back.
   */
  revealOffset?: number | null;
  revealNonce?: number;
  /**
   * F04: focuses a newly created split pane. Only the split-created pane opts in,
   * so opening a note normally does not steal focus from the workspace.
   */
  autoFocus?: boolean;
  /**
   * F14: one-based line of the caret, reported only when it moves. The outline
   * uses it to mark the current section without reading editor state.
   */
  onCaretLine?: (line: number) => void;
  /** F18: the current selection, so an insert can replace it. */
  onSelectionChange?: (selection: { from: number; to: number }) => void;
  /**
   * F47: registers a function the caller can use to apply computed edits.
   *
   * The editor owns the buffer and the undo stack, so an assist action is applied
   * here as one transaction rather than by replacing the whole document, which
   * would make one action many undo steps.
   */
  onReady?: (apply: AssistApplier) => void;
  /**
   * F05-A03: a mode switch asked for during an IME composition. `null` reports
   * that a switch was deferred; a mode reports that it may now be applied.
   */
  onDeferredMode?: (mode: EditorMode | null) => void;
  /**
   * F05: headings with source line numbers, in document order. The reading pane
   * uses the same list to scroll to the caret's section, because sanitized HTML
   * has no source offsets to map.
   */
  headingAnchors?: readonly HeadingAnchor[];
  /** F19: shows local images and stores pasted or dropped files. */
  attachments?: AttachmentHost;
  /** F20: a link to a PDF was followed. */
  onOpenPdf?: (target: string, page: number | null) => void;
}

/**
 * Where a pasted file's link will go. The position moves with later edits, and a
 * note switch leaves it behind in the other note's state, so a slow save can
 * never insert into the wrong note.
 */
const addUploadSlot = StateEffect.define<{ id: number; at: number }>();
const removeUploadSlot = StateEffect.define<number>();
const uploadSlots = StateField.define<ReadonlyMap<number, number>>({
  create: () => new Map(),
  update(value, transaction) {
    let next = value;
    if (transaction.docChanged && value.size > 0) {
      next = new Map([...value].map(([id, at]) => [id, transaction.changes.mapPos(at, 1)]));
    }
    for (const effect of transaction.effects) {
      if (effect.is(addUploadSlot)) next = new Map(next).set(effect.value.id, effect.value.at);
      if (effect.is(removeUploadSlot)) {
        const without = new Map(next);
        without.delete(effect.value);
        next = without;
      }
    }
    return next;
  },
});
let nextUploadSlot = 1;

/** Keeps every image spelling in one form the renderer knows how to show. */
function withImageSyntax(text: string): string {
  return rewriteImageEmbeds(text, scanEmbeds(text));
}

/** Stable empty list so an absent prop never allocates per render. */
const EMPTY_CANDIDATES: readonly LinkCandidate[] = [];
const EMPTY_ANCHORS: readonly AnchorCandidate[] = [];
const EMPTY_EMBEDS: EmbedTree = {};
const EMPTY_HEADINGS: readonly HeadingAnchor[] = [];

/** Applies one context-menu edit as a single, undoable transaction. */
function dispatchEdit(view: EditorView, edit: Edit): void {
  view.dispatch({
    changes: { from: edit.from, to: edit.to, insert: edit.insert },
    selection: { anchor: edit.selection.from, head: edit.selection.to },
    annotations: Transaction.userEvent.of('input'),
  });
  view.focus();
}

function runInline(view: EditorView, marker: InlineMarker): boolean {
  if (view.state.readOnly) return false;
  const main = view.state.selection.main;
  dispatchEdit(view, toggleInline(view.state.doc.toString(), main, marker));
  return true;
}

/** `Ctrl+B` on Windows and Linux, `⌘B` on macOS. */
function shortcutText(key: string): string {
  return /Mac|iPhone|iPad/.test(globalThis.navigator?.userAgent ?? '') ? `⌘${key}` : `Ctrl+${key}`;
}

/**
 * Turns one resolved node (and its children) into the shape the HTML renderer
 * expects. A missing node is still loading, `null` is a finished failure, and a
 * nested span without a node was deliberately not expanded (depth limit).
 */
function renderedEmbed(
  node: EmbedNode | null | undefined,
  span: EmbedSpan,
  translator: Translator,
): RenderedEmbed {
  if (node === undefined || node === null) {
    const block = node === null ? embedUnavailable(span) : embedBlock(span, null);
    return {
      raw: span.raw,
      content: null,
      placeholder: block.messageKey === null ? null : translator(block.messageKey),
      targetPath: block.targetPath,
      targetId: null,
      children: [],
    };
  }
  const block = embedBlock(span, node.expansion);
  const expanded = block.kind === 'content' ? withImageSyntax(block.text) : null;
  const children: RenderedEmbed[] =
    expanded !== null
      ? scanEmbeds(expanded).map((childSpan) =>
          renderedEmbed(
            node.children[`${childSpan.target}#${childSpan.anchor ?? ''}`],
            childSpan,
            translator,
          ),
        )
      : [];
  return {
    raw: span.raw,
    content: expanded,
    placeholder:
      block.kind === 'placeholder' && block.messageKey !== null
        ? translator(block.messageKey)
        : null,
    targetPath: block.targetPath,
    targetId: node.expansion.targetId,
    children,
  };
}

interface NoteSession {
  state: EditorState;
  version: number;
}

export function NoteEditor(props: NoteEditorProps): ReactNode {
  const {
    noteId,
    pathName,
    source,
    docVersion,
    readOnly,
    mode,
    locale,
    translator,
    onDocChanged,
    onExplicitSave,
    onCompositionChange,
    onCaretLine,
    onDeferredMode,
    onSelectionChange,
    onReady,
    revealOffset,
    revealNonce,
  } = props;

  const hostRef = useRef<HTMLDivElement | null>(null);
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const sessionsRef = useRef<Map<string, NoteSession>>(new Map());
  const activeNoteIdRef = useRef<string>(noteId);
  const appliedVersionsRef = useRef<Map<string, number>>(new Map());
  const markdownCompartmentRef = useRef<Compartment | null>(null);
  const editableCompartmentRef = useRef<Compartment | null>(null);
  const baseExtensionsRef = useRef<Extension[] | null>(null);
  const previousModeRef = useRef<EditorMode | null>(null);
  const previousReadOnlyRef = useRef<boolean | null>(null);
  const composingRef = useRef(false);
  const pendingCompositionEndRef = useRef(false);
  // The note that owns the current composition lifecycle: a pending
  // composition-end release is always addressed to this note, never to the
  // note that happens to be active when the delayed callback runs.
  const compositionNoteRef = useRef<string>(noteId);
  const pendingExplicitSaveRef = useRef(false);
  const pendingExplicitNoteRef = useRef<string>(noteId);
  const compositionReleaseTimerRef = useRef<number | null>(null);
  const callbacksRef = useRef({
    onDocChanged,
    onExplicitSave,
    onCompositionChange,
    onCaretLine,
    onDeferredMode,
    onSelectionChange,
    onReady,
  });
  callbacksRef.current = {
    onDocChanged,
    onExplicitSave,
    onCompositionChange,
    onCaretLine,
    onDeferredMode,
    onSelectionChange,
    onReady,
  };
  const lastCaretLineRef = useRef<number | null>(null);
  /** Selection/scroll remembered for the next mode switch (F05 §6). */
  const modeSnapshotRef = useRef<ModeSnapshot | null>(null);
  /** A switch requested during composition, applied when it ends. */
  const pendingModeRef = useRef<EditorMode | null>(null);
  /** One-based caret line to restore when leaving read mode. */
  const readAnchorLineRef = useRef<number | null>(null);
  const propsRef = useRef(props);
  propsRef.current = props;

  // S17 link picker state. The query is derived from the document on every
  // update; nothing is inserted until the user chooses a candidate.
  const [linkQuery, setLinkQuery] = useState<LinkQuery | null>(null);
  const [slashQuery, setSlashQuery] = useState<{ from: number; query: string } | null>(null);
  const [equationDraft, setEquationDraft] = useState<{
    noteId: string; source: string; from: number; to: number; selectionFrom: number; selected: string;
  } | null>(null);
  const [equationStale, setEquationStale] = useState(false);
  const [slashIndex, setSlashIndex] = useState(0);
  // Window coordinates of the open editor right-click menu.
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null);
  const [viewer, setViewer] = useState<{ url: string; alt: string } | null>(null);
  // Find in the reading pane; the nonce refocuses an already open bar.
  const [readFind, setReadFind] = useState<number | null>(null);
  useEffect(() => { setReadFind(null); }, [mode, noteId]);
  // Focus outside any editor (e.g. on the mode button) still finds in the one
  // reading pane on screen; with two panes open, focus decides which.
  useEffect(() => {
    if (mode !== 'read') return;
    const listener = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || event.isComposing) return;
      const focused = document.activeElement;
      if (focused instanceof HTMLElement && (focused.closest('.editor-region') || focused.isContentEditable ||
        focused instanceof HTMLInputElement || focused instanceof HTMLTextAreaElement || focused instanceof HTMLSelectElement)) return;
      if (document.querySelectorAll(".editor-region[data-editor-mode='read']").length !== 1) return;
      const result = (propsRef.current.findKeys ?? DEFAULT_FIND_KEYS).find(event);
      if (result === 'none') return;
      event.preventDefault();
      if (result === 'complete') setReadFind((nonce) => (nonce ?? 0) + 1);
    };
    window.addEventListener('keydown', listener);
    return () => window.removeEventListener('keydown', listener);
  }, [mode]);
  const slashRef = useRef<{ from: number; query: string } | null>(null);
  slashRef.current = slashQuery;
  const linkQueryRef = useRef<LinkQuery | null>(null);
  linkQueryRef.current = linkQuery;
  const linkCandidates = props.linkCandidates ?? EMPTY_CANDIDATES;
  const anchorsOf = props.anchorsOf ?? (() => EMPTY_ANCHORS);
  const anchorTarget = pickerAnchorTargetFor(linkCandidates, linkQuery);
  const embedExpansions = props.embedExpansions ?? EMPTY_EMBEDS;

  const formulaInputs = useMemo(() => {
    if (mode === 'source') return [];
    const inputs = new Map<string, FormulaInput>();
    const add = (text: string): void => { for (const input of markdownFormulas(text)) inputs.set(formulaKey(input), input); };
    add(source);
    const visit = (tree: EmbedTree, depth: number): void => {
      if (depth > 3) return;
      for (const node of Object.values(tree)) {
        if (node?.expansion.status === 'ok' && node.expansion.content !== null) {
          add(node.expansion.content); visit(node.children, depth + 1);
        }
      }
    };
    if (mode === 'read') visit(embedExpansions, 1);
    return [...inputs.values()];
  }, [source, mode, embedExpansions]);
  const formulaResults = useFormulaPreview(formulaInputs, `${noteId}:${source}`);
  useEffect(() => {
    const view = viewRef.current;
    if (mode !== 'live' || !view || !view.state.field(mathResultsField, false)) return;
    view.dispatch({ effects: mathPreviewEffect.of(formulaResults.map(result => ({
      ...result, errorText: formulaErrorText(result, translator),
    }))) });
  }, [formulaResults, mode, noteId, translator]);

  /**
   * True when `offset` is inside a fenced code block. The picker must not open
   * for a `[[` example in a fence, which is the same rule the index uses
   * (F10-R01).
   */
  function inFenceAt(state: EditorState, offset: number): boolean {
    const lineNumber = state.doc.lineAt(offset).number;
    let fence: string | null = null;
    for (let number = 1; number <= lineNumber; number += 1) {
      const text = state.doc.line(number).text.trimStart();
      const marker = /^(`{3,}|~{3,})/.exec(text);
      if (marker === null) {
        continue;
      }
      const run = marker[1] as string;
      if (fence === null) {
        fence = run[0] as string;
      } else if (run[0] === fence) {
        fence = null;
      }
    }
    return fence !== null;
  }

  /** Applies a chosen candidate, replacing only the typed trigger. */
  function chooseLinkCandidate(candidate: LinkCandidate): void {
    const view = viewRef.current;
    const query = linkQueryRef.current;
    if (view === null || query === null) {
      return;
    }
    const source = view.state.doc.toString();
    const { source: next, caret } = applyLinkInsertion(source, query, candidate);
    view.dispatch({
      changes: { from: 0, to: source.length, insert: next },
      selection: { anchor: caret },
      userEvent: 'opennote.insert-link',
    });
    view.focus();
    setLinkQuery(null);
  }

  /** Applies a chosen anchor, keeping the note target the user already typed. */
  function chooseAnchorCandidate(target: LinkCandidate, anchor: AnchorCandidate): void {
    const view = viewRef.current;
    const query = linkQueryRef.current;
    if (view === null || query === null) {
      return;
    }
    const source = view.state.doc.toString();
    const { source: next, caret } = applyAnchorInsertion(source, query, target, anchor);
    view.dispatch({
      changes: { from: 0, to: source.length, insert: next },
      selection: { anchor: caret },
      userEvent: 'opennote.insert-link',
    });
    view.focus();
    setLinkQuery(null);
  }

  /**
   * Applies one insertion in a single transaction. The `/query` range and the
   * selection are replaced together, so one undo restores the original text and
   * the typed command never survives in the note (F06-A03).
   */
  function applyInsertion(insertion: Insertion): void {
    const view = viewRef.current;
    const slash = slashRef.current;
    if (!view) {
      return;
    }
    const from = slash === null ? view.state.selection.main.from : slash.from;
    const to = view.state.selection.main.to;
    view.dispatch({
      changes: { from, to, insert: insertion.text },
      selection: {
        anchor: insertion.selection.from,
        head: insertion.selection.to,
      },
      annotations: Transaction.userEvent.of('input'),
    });
    setSlashQuery(null);
    setSlashIndex(0);
    view.focus();
  }

  /** Builds the fragment for a menu item from the current selection text. */
  function insertionFor(kind: SlashItem['kind']): Insertion | null {
    const view = viewRef.current;
    if (!view) {
      return null;
    }
    const slash = slashRef.current;
    const from = slash === null ? view.state.selection.main.from : slash.from;
    const to = view.state.selection.main.to;
    const selection: Selection = { from, to };
    const selected = view.state.sliceDoc(
      view.state.selection.main.from,
      view.state.selection.main.to,
    );
    switch (kind) {
      case 'text':
        return textInsertion(selection, selected);
      case 'heading': {
        const levelMatch = /^(#{1,6})\s/.exec(view.state.doc.lineAt(from).text);
        const level = levelMatch?.[1]?.length ?? 1;
        return headingInsertion(selection, level, selected);
      }
      case 'task':
        return taskInsertion(selection, selected);
      case 'table': {
        const result = tableInsertion(selection, 3, 3);
        return result.ok ? result.insertion : null;
      }
      case 'equation': {
        const result = equationInsertion(selection, selected, false);
        return result.ok ? result.insertion : null;
      }
      case 'code':
        return codeInsertion(selection, '', selected);
      case 'link':
        // The target stays empty so the user supplies a real path; F19 owns
        // attachment paths and this helper never invents one.
        return linkInsertion(selection, selected, selected.length > 0 ? '' : '');
      default:
        return null;
    }
  }

  function chooseSlashItem(item: SlashItem): void {
    const view = viewRef.current;
    if (item.kind === 'equation' && view) {
      const selection = view.state.selection.main;
      setEquationDraft({ noteId: activeNoteIdRef.current, source: view.state.doc.toString(),
        from: slashRef.current?.from ?? selection.from, to: selection.to, selectionFrom: selection.from,
        selected: view.state.sliceDoc(selection.from, selection.to) });
      setEquationStale(false);
      setSlashQuery(null);
      return;
    }
    const insertion = insertionFor(item.kind);
    if (insertion !== null) applyInsertion(insertion);
  }

  /** Applies `make` to the live view, if the editor still has one. */
  function editWith(make: (source: string, range: { from: number; to: number }) => Edit): void {
    const view = viewRef.current;
    if (!view || view.state.readOnly) return;
    dispatchEdit(view, make(view.state.doc.toString(), view.state.selection.main));
  }

  async function copySelection(cut: boolean): Promise<void> {
    const view = viewRef.current;
    if (!view) return;
    const { from, to } = view.state.selection.main;
    try {
      await navigator.clipboard.writeText(view.state.sliceDoc(from, to));
    } catch {
      return;
    }
    if (cut && !view.state.readOnly) {
      view.dispatch({ changes: { from, to }, annotations: Transaction.userEvent.of('delete.cut') });
    }
    view.focus();
  }

  async function pasteClipboard(): Promise<void> {
    const view = viewRef.current;
    if (!view || view.state.readOnly) return;
    let text: string;
    try {
      text = plainText(await navigator.clipboard.readText());
    } catch {
      // The WebView refused clipboard access; nothing is pasted.
      view.focus();
      return;
    }
    const { from, to } = view.state.selection.main;
    view.dispatch({
      changes: { from, to, insert: text },
      selection: { anchor: from + text.length },
      annotations: Transaction.userEvent.of('input.paste'),
    });
    view.focus();
  }

  function contextMenuEntries(): ContextMenuEntry[] {
    const view = viewRef.current;
    const hasSelection = view !== null && !view.state.selection.main.empty;
    const selectAll = {
      key: 'selectAll', label: translator('editorMenu.selectAll'), icon: <SquareDashed size={16} />,
      run: () => { if (view) { view.dispatch({ selection: { anchor: 0, head: view.state.doc.length } }); view.focus(); } },
    };
    const copy = {
      key: 'copy', label: translator('editorMenu.copy'), icon: <Copy size={16} />, disabled: !hasSelection,
      run: () => { void copySelection(false); },
    };
    if (readOnly) return [copy, selectAll];

    const inline = (key: string, marker: InlineMarker, icon: ReactNode, shortcut?: string): ContextMenuEntry => ({
      key, label: translator(`editorMenu.${key}` as MessageKey), icon, shortcut, disabled: !hasSelection,
      run: () => editWith((text, range) => toggleInline(text, range, marker)),
    });
    const block = (key: BlockKind, icon: ReactNode): ContextMenuEntry => ({
      key, label: translator(`editorMenu.block.${key}` as MessageKey), icon,
      run: () => editWith((text, range) => convertBlock(text, range, key)),
    });
    const labels = slashLabels(translator);
    const insertIcons: Record<SlashItem['kind'], ReactNode> = {
      text: <Type size={16} />, heading: <Heading1 size={16} />, task: <ListTodo size={16} />,
      table: <Table size={16} />, equation: <Sigma size={16} />, code: <SquareCode size={16} />, link: <Link size={16} />,
    };

    return [
      { key: 'addLink', label: translator('editorMenu.addLink'), icon: <Link size={16} />, run: () => editWith(internalLink) },
      { key: 'addExternalLink', label: translator('editorMenu.addExternalLink'), icon: <ExternalLink size={16} />, run: () => editWith(externalLink) },
      'separator',
      { key: 'format', label: translator('editorMenu.format'), icon: <Paintbrush size={16} />, children: [
        inline('bold', '**', <Bold size={16} />, shortcutText('B')),
        inline('italic', '*', <Italic size={16} />, shortcutText('I')),
        inline('strikethrough', '~~', <Strikethrough size={16} />),
        inline('highlight', '==', <Highlighter size={16} />),
        'separator',
        inline('code', '`', <Code size={16} />),
        inline('math', '$', <Sigma size={16} />),
        inline('comment', '%%', <Percent size={16} />),
        'separator',
        { key: 'clearFormatting', label: translator('editorMenu.clearFormatting'), icon: <Eraser size={16} />,
          disabled: !hasSelection, run: () => editWith(clearInline) },
      ] },
      { key: 'paragraph', label: translator('editorMenu.paragraph'), icon: <Pilcrow size={16} />, children: [
        block('paragraph', <Text size={16} />),
        block('heading1', <Heading1 size={16} />),
        block('heading2', <Heading2 size={16} />),
        block('heading3', <Heading3 size={16} />),
        'separator',
        block('bullet', <List size={16} />),
        block('numbered', <ListOrdered size={16} />),
        block('task', <ListTodo size={16} />),
        'separator',
        block('quote', <Quote size={16} />),
        block('codeBlock', <CodeXml size={16} />),
      ] },
      { key: 'insert', label: translator('insert.title'), icon: <ListPlus size={16} />,
        children: rankSlashItems('', labels).map(item => ({
          key: `insert-${item.kind}`, label: labels[item.kind], icon: insertIcons[item.kind],
          run: () => {
            // A menu insertion replaces the selection, never an open `/query`.
            slashRef.current = null;
            chooseSlashItem(item);
          },
        })) },
      'separator',
      { key: 'cut', label: translator('editorMenu.cut'), icon: <Scissors size={16} />, disabled: !hasSelection,
        run: () => { void copySelection(true); } },
      copy,
      { key: 'paste', label: translator('editorMenu.paste'), icon: <ClipboardPaste size={16} />, run: () => { void pasteClipboard(); } },
      { key: 'pastePlain', label: translator('editorMenu.pastePlain'), icon: <ClipboardType size={16} />, run: () => { void pasteClipboard(); } },
      selectAll,
    ];
  }

  function insertEquation(tex: string, display: boolean): void {
    const view = viewRef.current;
    const captured = equationDraft;
    if (!view || !captured) return;
    if (readOnly || mode === 'read' || activeNoteIdRef.current !== captured.noteId ||
      view.state.doc.toString() !== captured.source || view.state.selection.main.to !== captured.to ||
      view.state.selection.main.from !== captured.selectionFrom) {
      setEquationStale(true);
      return;
    }
    const result = equationInsertion({ from: captured.from, to: captured.to }, tex, display);
    if (!result.ok) return;
    let { text, selection } = result.insertion;
    if (display) {
      const prefix = captured.from > 0 && captured.source[captured.from - 1] !== '\n' ? '\n' : '';
      const suffix = captured.to < captured.source.length && captured.source[captured.to] !== '\n' ? '\n' : '';
      text = prefix + text + suffix;
      selection = { from: selection.from + prefix.length, to: selection.to + prefix.length };
    }
    view.dispatch({ changes: { from: captured.from, to: captured.to, insert: text },
      selection: { anchor: selection.from, head: selection.to },
      annotations: [Transaction.userEvent.of('input'), isolateHistory.of('full')] });
    setEquationDraft(null);
    view.focus();
  }

  /** Keyboard for the slash menu; the editor keeps focus throughout. */
  function handleSlashKeyDown(event: ReactKeyboardEvent<HTMLDivElement>): boolean {
    if (slashRef.current === null || event.nativeEvent.isComposing) {
      return false;
    }
    const labels = slashLabels(translator);
    const items = rankSlashItems(slashRef.current.query, labels);
    if (items.length === 0 && event.key !== 'Escape') {
      return false;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      setSlashQuery(null);
      return true;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const step = event.key === 'ArrowDown' ? 1 : -1;
      setSlashIndex((index) => {
        const size = Math.max(1, items.length);
        return (((index + step) % size) + size) % size;
      });
      return true;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      const item = items[Math.min(slashIndex, items.length - 1)];
      if (item !== undefined) {
        chooseSlashItem(item);
      }
      return true;
    }
    if (/^[1-7]$/.test(event.key)) {
      const byDigit = items.find((item) => item.digit === Number(event.key));
      if (byDigit !== undefined) {
        event.preventDefault();
        chooseSlashItem(byDigit);
        return true;
      }
    }
    return false;
  }

  function cancelCompositionReleaseTimer(): void {
    const timer = compositionReleaseTimerRef.current;
    if (timer !== null) {
      window.clearTimeout(timer);
      compositionReleaseTimerRef.current = null;
    }
  }

  // A Ctrl+S saved during an active composition is released only with the
  // composition-end signal, after the final committed input has been
  // forwarded through onDocChanged.
  function releasePendingExplicitSave(): void {
    if (!pendingExplicitSaveRef.current) {
      return;
    }
    pendingExplicitSaveRef.current = false;
    callbacksRef.current.onExplicitSave(pendingExplicitNoteRef.current);
  }

  // The browser fires compositionend before the composition's final committed
  // input reaches the editor: CodeMirror flushes pending DOM changes in a
  // microtask (or on requestAnimationFrame on Android), so a listener that
  // reacts synchronously would observe half-composed text. The composition
  // state is therefore released either right after the next committed doc
  // update, or by a small scheduled fallback when no change follows. The
  // composition-end signal is released first, so a pending explicit save it
  // triggers is issued with the final text already in the controller.
  function releaseCompositionEnd(): void {
    cancelCompositionReleaseTimer();
    if (!pendingCompositionEndRef.current) {
      return;
    }
    pendingCompositionEndRef.current = false;
    callbacksRef.current.onCompositionChange(compositionNoteRef.current, false);
    releasePendingExplicitSave();
    // A switch requested during the composition is applied now. `props.mode`
    // already holds the requested value, so the editor applies it here instead
    // of waiting for another parent render that would never come (F05-A03).
    const pending: EditorMode | null = pendingModeRef.current;
    pendingModeRef.current = null;
    const view = viewRef.current;
    if (pending !== null && view) {
      previousModeRef.current = pending;
      applyCompartments(view);
      callbacksRef.current.onDeferredMode?.(pending);
    }
  }

  // The composition lifecycle belongs to the document it started on. A note
  // switch or unmount is a lifetime change: the delayed composition-end
  // callback is cancelled and any active/pending composition suspension is
  // released synchronously, addressed to the originating note, so neither it
  // nor a pending explicit save can ever fire into the new document.
  function releaseCompositionOnLifetimeEnd(): void {
    cancelCompositionReleaseTimer();
    const wasComposing = composingRef.current;
    const hadPendingRelease = pendingCompositionEndRef.current;
    composingRef.current = false;
    pendingCompositionEndRef.current = false;
    if (wasComposing || hadPendingRelease) {
      callbacksRef.current.onCompositionChange(compositionNoteRef.current, false);
    }
  }

  /** Stores files one by one and inserts each link where the drop or caret was. */
  function attachFiles(view: EditorView, files: readonly File[], at: number): void {
    const host = propsRef.current.attachments;
    if (host === undefined) return;
    const translate = propsRef.current.translator;
    if (propsRef.current.readOnly || composingRef.current || pendingCompositionEndRef.current) {
      host.report(translate('attachment.dropReadOnly'));
      return;
    }
    const notePath = propsRef.current.pathName;
    const id = nextUploadSlot++;
    view.dispatch({ effects: addUploadSlot.of({ id, at }) });
    void (async () => {
      let first = true;
      for (const file of files) {
        const stored = await host.storeFile(notePath, file);
        if (!stored.ok) {
          host.report(stored.message);
          continue;
        }
        const slot = viewRef.current === view ? view.state.field(uploadSlots).get(id) : undefined;
        if (slot === undefined || propsRef.current.readOnly || composingRef.current || pendingCompositionEndRef.current) {
          host.report(translate('attachment.notInserted', { name: file.name }));
          continue;
        }
        const insert = (first ? '' : '\n') + stored.markdown;
        first = false;
        view.dispatch({
          changes: { from: slot, insert },
          selection: { anchor: slot + insert.length },
          annotations: Transaction.userEvent.of('input.paste'),
        });
      }
      if (viewRef.current === view && view.state.field(uploadSlots).has(id)) {
        view.dispatch({ effects: removeUploadSlot.of(id) });
      }
    })();
  }

  function editableValue(): readonly Extension[] {
    return [
      EditorView.editable.of(!propsRef.current.readOnly),
      EditorState.readOnly.of(propsRef.current.readOnly),
    ];
  }

  function buildBaseExtensions(): Extension[] {
    const markdownCompartment = markdownCompartmentRef.current;
    const editableCompartment = editableCompartmentRef.current;
    if (!markdownCompartment || !editableCompartment) {
      throw new Error('Editor compartments were not initialized.');
    }
    return [
      EditorView.lineWrapping,
      // An empty note says how to start, as in other note apps.
      placeholder(() => {
        const hint = document.createElement('span');
        hint.textContent = propsRef.current.translator('editor.placeholder');
        return hint;
      }),
      history(),
      findReplace(propsRef.current.translator, () => propsRef.current.findKeys ?? DEFAULT_FIND_KEYS),
      keymap.of([
        {
          key: 'Mod-s',
          run: () => {
            if (
              composingRef.current ||
              pendingCompositionEndRef.current ||
              pendingExplicitSaveRef.current
            ) {
              // The save waits for the composition end release, which only
              // runs after the final committed input reached onDocChanged.
              pendingExplicitSaveRef.current = true;
              pendingExplicitNoteRef.current = activeNoteIdRef.current;
              return true;
            }
            callbacksRef.current.onExplicitSave(activeNoteIdRef.current);
            return true;
          },
        },
        { key: 'Mod-b', run: view => runInline(view, '**') },
        { key: 'Mod-i', run: view => runInline(view, '*') },
        ...historyKeymap,
        ...defaultKeymap,
      ]),
      uploadSlots,
      liveImageHost.of({
        load: async target => (await propsRef.current.attachments?.loadImage(propsRef.current.pathName, target))?.url ?? null,
        failedText: target => propsRef.current.translator('editor.read.imageBlockedLocal', { name: target }),
      }),
      markdownCompartment.of(
        propsRef.current.mode === 'live' ? liveMarkdownSyntaxExtensions() : [],
      ),
      editableCompartment.of(editableValue()),
      liveInteractions(
        target => propsRef.current.onFollowLink?.(target),
        () => composingRef.current || pendingCompositionEndRef.current,
        href => {
          const pdf = pdfLinkTarget(href);
          if (pdf === null || propsRef.current.onOpenPdf === undefined) return false;
          propsRef.current.onOpenPdf(pdf.target, pdf.page);
          return true;
        },
      ),
      // The note's own right-click menu replaces the WebView's (Back, Print…).
      // A click outside the selection moves the caret there first, so the
      // menu acts on what was clicked; one inside keeps the selection.
      EditorView.domEventHandlers({
        paste: (event, view) => {
          const files = Array.from(event.clipboardData?.files ?? []);
          // Office and browsers put a picture beside the text; the text is what was meant.
          if (files.length === 0 || (event.clipboardData?.getData('text/plain') ?? '') !== '') return false;
          event.preventDefault();
          attachFiles(view, files, view.state.selection.main.head);
          return true;
        },
        drop: (event, view) => {
          const files = Array.from(event.dataTransfer?.files ?? []);
          if (files.length === 0) return false;
          event.preventDefault();
          attachFiles(view, files, view.posAtCoords({ x: event.clientX, y: event.clientY }) ?? view.state.selection.main.head);
          return true;
        },
        dblclick: (event) => {
          const image = event.target instanceof HTMLImageElement ? event.target : null;
          if (image === null || !image.classList.contains('md-image')) return false;
          event.preventDefault();
          setViewer({ url: image.src, alt: image.alt });
          return true;
        },
        contextmenu: (event, view) => {
          if (composingRef.current || pendingCompositionEndRef.current) return false;
          event.preventDefault();
          const at = view.posAtCoords({ x: event.clientX, y: event.clientY });
          const main = view.state.selection.main;
          if (at !== null && (at < main.from || at > main.to)) {
            view.dispatch({ selection: { anchor: at } });
          }
          setContextMenu({ x: event.clientX, y: event.clientY });
          return true;
        },
      }),
      // F04: a read-only pane still holds its own selection, but undo must not
      // turn a mirror back into a second writer. Only the external-sync
      // transaction (which carries this annotation) may change its document.
      EditorState.transactionFilter.of((transaction) => {
        if (!transaction.docChanged || !propsRef.current.readOnly) {
          return transaction;
        }
        return transaction.isUserEvent(EXTERNAL_REPLACE_EVENT) ? transaction : [];
      }),
      EditorView.updateListener.of((update) => {
        const external = update.transactions.some(transaction =>
          transaction.isUserEvent(EXTERNAL_REPLACE_EVENT),
        );
        if (external) {
          // A mirror applying the active pane's edit is not local input: it
          // must not open a picker, release composition state, or feed the
          // external text back as a second save. Its mapped selection is still
          // reported so the pane's own selection survives the remote change.
          if (update.selectionSet) {
            callbacksRef.current.onSelectionChange?.({
              from: update.state.selection.main.from,
              to: update.state.selection.main.to,
            });
          }
          return;
        }
        if (update.docChanged) {
          callbacksRef.current.onDocChanged(
            activeNoteIdRef.current,
            update.state.doc.toString(),
          );
          if (pendingCompositionEndRef.current) {
            releaseCompositionEnd();
          } else if (!composingRef.current && pendingExplicitSaveRef.current) {
            releasePendingExplicitSave();
          }
        }
        // S17: open the picker whenever the caret sits inside an open `[[`.
        // A composition in progress is not a command, so it never opens it.
        if (update.docChanged || update.selectionSet) {
          if (composingRef.current) {
            setLinkQuery(null);
            setSlashQuery(null);
          } else {
            const { state } = update;
            const head = state.selection.main.head;
            const source = state.doc.toString();
            const next = detectLinkQuery(source, head, offset => {
              const line = state.doc.lineAt(offset);
              return /^\s{4,}|^\t/.test(line.text) || inFenceAt(state, offset);
            });
            setLinkQuery(next);
            // The slash menu and the link picker are mutually exclusive: a
            // dialog is not a second command surface over the same caret.
            const slash = next === null ? detectSlashQuery(source, head) : null;
            setSlashQuery(slash);
            if (slash === null) {
              setSlashIndex(0);
            }
            const main = state.selection.main;
            modeSnapshotRef.current = {
              selection: {
                anchor: main.anchor,
                head: main.head,
                reversed: main.head < main.anchor,
              },
              scrollTop: update.view.scrollDOM.scrollTop,
              scrollLine: state.doc.lineAt(update.view.viewport.from).number,
              caretLine: state.doc.lineAt(head).number,
            };
            callbacksRef.current.onSelectionChange?.({
              from: state.selection.main.from,
              to: state.selection.main.to,
            });
            const caretLine = state.doc.lineAt(head).number;
            if (caretLine !== lastCaretLineRef.current) {
              lastCaretLineRef.current = caretLine;
              callbacksRef.current.onCaretLine?.(caretLine);
            }
          }
        }
      }),
    ];
  }

  /**
   * Reconfigures the compartments for the current mode.
   *
   * F05-R01: only extensions are reconfigured, the document is never re-created.
   * The caret is preserved across live ↔ source (F05 §6) and clamped so an
   * out-of-range remembered position cannot throw, while live mode keeps the
   * caret out of the hidden frontmatter.
   */
  function applyCompartments(view: EditorView): void {
    const markdownCompartment = markdownCompartmentRef.current;
    const editableCompartment = editableCompartmentRef.current;
    if (!markdownCompartment || !editableCompartment) {
      return;
    }
    const mode = propsRef.current.mode;
    const main = view.state.selection.main;
    const remembered: EditorSelection =
      modeSnapshotRef.current?.selection ?? {
        anchor: main.anchor,
        head: main.head,
        reversed: main.head < main.anchor,
      };
    view.dispatch({
      effects: [
        markdownCompartment.reconfigure(
          mode === 'live' ? liveMarkdownSyntaxExtensions() : [],
        ),
        editableCompartment.reconfigure(editableValue()),
      ],
    });
    if (mode === 'read') {
      // Keep the caret line so the reading pane can land on the same section.
      readAnchorLineRef.current = modeSnapshotRef.current?.caretLine ?? 1;
      return;
    }
    // Returning from read mode: restore the viewport line the user was reading,
    // but only after the selection is applied, so the scroll is not overridden.
    const restoreLine = readAnchorLineRef.current;
    readAnchorLineRef.current = null;
    const bodyStart = initialEditorPosition(view.state.doc.toString(), mode);
    const clamped = mode === 'live'
      ? bodySafeSelection(clampSelection(remembered, view.state.doc.length), bodyStart)
      : clampSelection(remembered, view.state.doc.length);
    view.dispatch({
      selection: { anchor: clamped.anchor, head: clamped.head },
    });
    if (restoreLine !== null && restoreLine >= 1) {
      const line = Math.min(view.state.doc.lines, restoreLine);
      view.dispatch({
        effects: EditorView.scrollIntoView(view.state.doc.line(line).from, { y: 'start' }),
      });
    }
  }

  function createSessionState(): { state: EditorState; version: number } {
    const propsNow = propsRef.current;
    return {
      state: EditorState.create({
        doc: propsNow.source,
        selection: { anchor: initialEditorPosition(propsNow.source, propsNow.mode) },
        extensions: buildBaseExtensionsRef(),
      }),
      version: propsNow.docVersion,
    };
  }

  function buildBaseExtensionsRef(): Extension[] {
    const existing = baseExtensionsRef.current;
    if (existing) {
      return existing;
    }
    const built = buildBaseExtensions();
    baseExtensionsRef.current = built;
    return built;
  }

  useEffect(() => {
    if (!markdownCompartmentRef.current) {
      markdownCompartmentRef.current = new Compartment();
    }
    if (!editableCompartmentRef.current) {
      editableCompartmentRef.current = new Compartment();
    }
    const host = hostRef.current;
    if (!host) {
      return;
    }
    const session = sessionsRef.current.get(noteId);
    const appliedVersion = appliedVersionsRef.current.get(noteId);
    let entry = createSessionState();
    if (
      session &&
      appliedVersion !== undefined &&
      session.version >= appliedVersion &&
      session.state.doc.toString() === source
    ) {
      entry = { state: session.state, version: session.version };
    }
    appliedVersionsRef.current.set(noteId, entry.version);
    sessionsRef.current.delete(noteId);
    activeNoteIdRef.current = noteId;
    const view = new EditorView({
      parent: host,
      state: entry.state,
    });
    // F47: applying computed edits as one transaction keeps one action as one
    // undo step, and the ordering (back to front) protects earlier offsets.
    callbacksRef.current.onReady?.((edits, target) => {
      if (edits.length === 0 || viewRef.current !== view || propsRef.current.readOnly ||
          propsRef.current.mode === 'read' || view.state.readOnly || composingRef.current || pendingCompositionEndRef.current) {
        return false;
      }
      if (target && (target.noteId !== activeNoteIdRef.current ||
          target.source.replace(/\r\n?/g, '\n') !== view.state.doc.toString() ||
          target.selection.from !== view.state.selection.main.from || target.selection.to !== view.state.selection.main.to)) return false;
      const length = view.state.doc.length;
      for (const edit of edits) {
        if (!Number.isSafeInteger(edit.start) || !Number.isSafeInteger(edit.end) || edit.start < 0 || edit.end < edit.start || edit.end > length) {
          // A stale or invalid range is refused rather than applied approximately.
          return false;
        }
      }
      const ordered = [...edits].sort((left, right) => right.start - left.start);
      if (ordered.some((edit, index) => index > 0 && edit.end > ordered[index - 1]!.start)) return false;
      if (target && (!Number.isSafeInteger(target.caret) || target.caret < 0 ||
          target.caret > length + edits.reduce((delta, edit) => delta + edit.text.length - (edit.end - edit.start), 0))) return false;
      view.dispatch({
        changes: ordered.map((edit) => ({ from: edit.start, to: edit.end, insert: edit.text })),
        userEvent: 'opennote.assist',
        selection: target ? { anchor: target.caret } : undefined,
        annotations: target ? isolateHistory.of('full') : undefined,
      });
      return true;
    });
    viewRef.current = view;
    callbacksRef.current.onSelectionChange?.({
      from: view.state.selection.main.from,
      to: view.state.selection.main.to,
    });
    if (propsRef.current.autoFocus) {
      // F04: a fresh split pane is active immediately; the editor it replaces
      // must not keep the keyboard focus while the new pane waits for a click.
      view.focus();
    }
    // F14: the outline needs a current section before the user moves the caret.
    lastCaretLineRef.current = view.state.doc.lineAt(view.state.selection.main.head).number;
    callbacksRef.current.onCaretLine?.(lastCaretLineRef.current);

    const content = view.contentDOM;
    content.setAttribute('aria-label', translator('a11y.editor.region', { name: pathName }));

    const handleCompositionStart = (): void => {
      cancelCompositionReleaseTimer();
      pendingCompositionEndRef.current = false;
      composingRef.current = true;
      compositionNoteRef.current = activeNoteIdRef.current;
      callbacksRef.current.onCompositionChange(activeNoteIdRef.current, true);
    };
    const handleCompositionEnd = (): void => {
      composingRef.current = false;
      if (pendingCompositionEndRef.current) {
        return;
      }
      pendingCompositionEndRef.current = true;
      compositionReleaseTimerRef.current = window.setTimeout(() => {
        compositionReleaseTimerRef.current = null;
        releaseCompositionEnd();
      }, 0);
    };
    content.addEventListener('compositionstart', handleCompositionStart);
    content.addEventListener('compositionend', handleCompositionEnd);

    return () => {
      content.removeEventListener('compositionstart', handleCompositionStart);
      content.removeEventListener('compositionend', handleCompositionEnd);
      // Unmount is also a lifetime change: the pending composition state is
      // released addressed to its originating note so the save controller is
      // not left composing for a document that no longer has an editor.
      releaseCompositionOnLifetimeEnd();
      pendingExplicitSaveRef.current = false;
      const note = activeNoteIdRef.current;
      const version = appliedVersionsRef.current.get(note);
      if (version !== undefined) {
        sessionsRef.current.set(note, { state: view.state, version });
      }
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || noteId === activeNoteIdRef.current) {
      return;
    }
    const previousNoteId = activeNoteIdRef.current;
    const previousVersion = appliedVersionsRef.current.get(previousNoteId);
    if (previousVersion !== undefined) {
      sessionsRef.current.set(previousNoteId, { state: view.state, version: previousVersion });
    }
    activeNoteIdRef.current = noteId;

    const session = sessionsRef.current.get(noteId);
    const hadFocusBefore = view.hasFocus;
    const expectedVersion = appliedVersionsRef.current.get(noteId);
    let entry = createSessionState();
    if (
      session &&
      expectedVersion !== undefined &&
      session.version >= expectedVersion &&
      session.state.doc.toString() === source
    ) {
      entry = { state: session.state, version: session.version };
      sessionsRef.current.delete(noteId);
    }
    appliedVersionsRef.current.set(noteId, entry.version);
    view.setState(entry.state);
    callbacksRef.current.onSelectionChange?.({
      from: view.state.selection.main.from,
      to: view.state.selection.main.to,
    });
    lastCaretLineRef.current = view.state.doc.lineAt(view.state.selection.main.head).number;
    callbacksRef.current.onCaretLine?.(lastCaretLineRef.current);
    // Composition/save suspension state belongs to the previous document: it
    // is released synchronously and addressed to that document (never into
    // the fresh one), and the delayed composition-end callback is cancelled.
    releaseCompositionOnLifetimeEnd();
    pendingExplicitSaveRef.current = false;
    applyCompartments(view);
    if (hadFocusBefore && entry.state.facet(EditorView.editable)) {
      view.focus();
    }
  }, [noteId, source]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    const applied = appliedVersionsRef.current.get(noteId);
    const change = externalChange(view.state.doc.toString(), source);
    if (docVersion > (applied ?? docVersion) && change !== null) {
      // Keep the unchanged prefix and suffix so each pane's selection maps
      // through the remote edit instead of jumping to the end of the note.
      // The replacement is excluded from this pane's undo history (F07-R01).
      view.dispatch(externalReplacement(change));
    }
    appliedVersionsRef.current.set(noteId, docVersion);
  }, [docVersion, noteId, source]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    if (previousModeRef.current === null) {
      // First run: the mount effect already built the compartments, and only
      // `autoFocus` decides whether a newly opened editor takes the keyboard.
      // Focusing every mount would make the prop meaningless and steal focus
      // from the note list on startup.
      previousModeRef.current = mode;
      previousReadOnlyRef.current = readOnly;
      applyCompartments(view);
      return;
    }
    // Locale-only changes (e.g. a settings screen changing the language while
    // the editor stays mounted) must reconfigure labels without taking focus.
    const modeChanged = previousModeRef.current !== mode;
    if (
      modeChanged &&
      !decideModeSwitch(mode, previousModeRef.current, composingRef.current, modeSnapshotRef.current)
        .apply
    ) {
      // A composition owns the pending text: remember the request and let the
      // parent re-apply it after compositionend (F05-A03).
      pendingModeRef.current = mode;
      callbacksRef.current.onDeferredMode?.(null);
      return;
    }
    const inputsUnchanged = !modeChanged && previousReadOnlyRef.current === readOnly;
    previousModeRef.current = mode;
    previousReadOnlyRef.current = readOnly;
    applyCompartments(view);
    if (inputsUnchanged) {
      return;
    }
    if (mode === 'read') {
      overlayRef.current?.focus();
    } else if (!readOnly) {
      // F04: an inactive pane is read-only; it must not steal the keyboard
      // focus from the pane the user just activated. Activating a pane also
      // restores the outline row for that pane's own caret.
      view.focus();
      const line = view.state.doc.lineAt(view.state.selection.main.head).number;
      lastCaretLineRef.current = line;
      callbacksRef.current.onCaretLine?.(line);
    }
  }, [mode, readOnly, locale]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    // Reconfigured on locale/path changes only; setting the attribute never
    // touches editor state, so history is preserved.
    view.contentDOM.setAttribute('aria-label', translator('a11y.editor.region', { name: pathName }));
  }, [locale, pathName, translator]);

  useEffect(() => {
    // A note switch or mode change ends the slash query: its `from` offset
    // belongs to the previous document and must never be applied to a new one.
    setSlashQuery(null);
    setSlashIndex(0);
    setEquationDraft(null);
  }, [noteId, mode]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || revealOffset === undefined || revealOffset === null) {
      return;
    }
    // CodeMirror positions and the host's sourceOffset both count UTF-16 code
    // units, so a Turkish İ or an emoji ahead of the hit cannot skew this.
    const position = Math.max(0, Math.min(view.state.doc.length, revealOffset));
    view.dispatch({
      selection: { anchor: position },
      effects: EditorView.scrollIntoView(position, { y: 'center' }),
    });
    view.focus();
    // revealNonce is the trigger; revealOffset alone may repeat across hits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revealNonce]);

  const readCaptions = useMemo(
    () => ({
      frontmatterLabel: translator('editor.read.frontmatterLabel'),
      localImageSlots: props.attachments !== undefined,
      blockedImageRemote: translator('editor.read.imageBlockedRemote'),
      blockedImageLocal: (imageSource: string) =>
        translator('editor.read.imageBlockedLocal', { name: imageSource }),
      openEmbeddedSource: (pathName: string) => translator('embed.openSource', { path: pathName }),
      mathHint: (key: FormulaNotice, position?: number) => translator(key) +
        (position === undefined ? '' : ` ${translator('math.hint.position', { position })}`),
    }),
    [translator, props.attachments !== undefined],
  );
  const readHtml = useMemo(() => {
    if (mode !== 'read') {
      return '';
    }
    const written = splitFrontMatter(source).body;
    const body = withImageSyntax(written);
    const prepared = source.slice(0, source.length - written.length) + body;
    const spans = scanEmbeds(body);
    if (spans.length === 0) {
      // Whole source, not `body`: the renderer shows frontmatter as escaped
      // text, and splitting here only decides where embeds are looked for.
      return renderSafeMarkdown(readCaptions, prepared, formulaResults);
    }
    // Embeds are resolved by the caller and handed in; a missing entry is still
    // loading, an explicit null failed, and both keep the written syntax.
    const rendered: RenderedEmbed[] = [];
    for (const span of spans) {
      const node = embedExpansions[`${span.target}#${span.anchor ?? ''}`];
      rendered.push(renderedEmbed(node, span, translator));
    }
    return renderWithEmbeds(readCaptions, prepared, rendered, formulaResults);
  }, [mode, readCaptions, source, embedExpansions, translator, formulaResults]);

  // F19: fills the image slots in the sanitized HTML. Results for an older render
  // are dropped, so a slow image never lands in a different note.
  useEffect(() => {
    const host = props.attachments;
    const overlay = overlayRef.current;
    if (mode !== 'read' || host === undefined || overlay === null) return;
    let current = true;
    for (const slot of overlay.querySelectorAll<HTMLElement>('[data-attachment-src]')) {
      void host.loadImage(pathName, slot.dataset.attachmentSrc ?? '').then((loaded) => {
        if (!current) return;
        if (loaded === null) {
          slot.classList.add('md-image-blocked');
          slot.textContent = `[${slot.dataset.imageFailed ?? ''}]`;
          return;
        }
        const image = document.createElement('img');
        image.className = 'md-image';
        image.alt = slot.dataset.imageAlt ?? '';
        image.tabIndex = 0;
        image.src = loaded.url;
        slot.replaceChildren(image);
      });
    }
    return () => { current = false; };
  }, [mode, readHtml, pathName, props.attachments]);

  // F05: when the reading pane appears, put the caret's section at the top. The
  // pane is sanitized HTML, so the shared heading list is the only stable
  // mapping between the two representations.
  useEffect(() => {
    if (mode !== 'read') {
      return;
    }
    const overlay = overlayRef.current;
    const caretLine = readAnchorLineRef.current;
    if (!overlay || caretLine === null) {
      return;
    }
    const anchors = props.headingAnchors ?? EMPTY_HEADINGS;
    const elements = overlay.querySelectorAll<HTMLElement>('h1, h2, h3, h4, h5, h6');
    const target = readPaneTarget(anchors, elements.length, caretLine - 1);
    if (target < 0) {
      return;
    }
    elements[target]?.scrollIntoView({ block: 'start' });
  }, [mode, readHtml, props.headingAnchors]);

  // Markdown links must not navigate the WebView away from the app: the
  // reading pane only ever shows rendered markdown, so anchor activation is
  // prevented at the DOM level (no fake link-opening feature, no network). The
  // embed source row is the one intentional navigation, and it stays inside the
  // app: the host opens the target note through the same workspace path.
  function handleReadPaneClick(event: ReactMouseEvent<HTMLDivElement>): void {
    const element = event.target instanceof Element ? event.target : null;
    if (element instanceof HTMLImageElement && element.classList.contains('md-image')) {
      setViewer({ url: element.src, alt: element.alt });
      return;
    }
    const jump = element?.closest('[data-embed-open]');
    if (jump instanceof HTMLElement) {
      event.preventDefault();
      const target = jump.dataset.embedOpen;
      if (target !== undefined && target.length > 0) {
        props.onOpenEmbed?.(target);
      }
      return;
    }
    const anchor = element?.closest('a');
    if (anchor !== null && anchor !== undefined) {
      event.preventDefault();
      const pdf = pdfLinkTarget(anchor.getAttribute('href') ?? '');
      if (pdf !== null) props.onOpenPdf?.(pdf.target, pdf.page);
    }
  }

  return (
    <section
      className="editor-region"
      aria-label={translator('a11y.editor.region', { name: pathName })}
      data-editor-dir={dirForLocale(locale)}
      data-editor-mode={mode}
      onKeyDown={(event) => {
        if (mode !== 'read' || event.nativeEvent.isComposing) return;
        const result = (props.findKeys ?? DEFAULT_FIND_KEYS).find(event.nativeEvent);
        if (result === 'none') return;
        event.preventDefault();
        event.stopPropagation();
        if (result === 'complete') setReadFind((nonce) => (nonce ?? 0) + 1);
      }}
    >
      <div
        className="editor-host"
        ref={hostRef}
        inert={mode === 'read'}
        onKeyDownCapture={(event) => {
          if (handleSlashKeyDown(event)) {
            event.stopPropagation();
          }
        }}
      />
      {linkQuery !== null && mode !== 'read' ? (
        <LinkPicker
          query={linkQuery}
          candidates={linkCandidates}
          anchors={anchorTarget === null ? [] : anchorsOf(anchorTarget.noteId)}
          target={anchorTarget}
          translator={translator}
          onChoose={chooseLinkCandidate}
          onChooseAnchor={chooseAnchorCandidate}
          onClose={() => setLinkQuery(null)}
        />
      ) : null}
      {slashQuery !== null && mode !== 'read' ? (
        <SlashMenu
          query={slashQuery.query}
          selectedIndex={slashIndex}
          translator={translator}
          onSelect={chooseSlashItem}
          onHover={setSlashIndex}
        />
      ) : null}
      {contextMenu !== null && mode !== 'read' ? (
        <EditorContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          label={translator('editorMenu.label')}
          entries={contextMenuEntries()}
          onClose={() => {
            setContextMenu(null);
            viewRef.current?.focus();
          }}
        />
      ) : null}
      {equationDraft !== null ? (
        <EquationDialog initialSource={equationDraft.selected} translator={translator}
          stale={equationStale || equationDraft.noteId !== noteId || equationDraft.source !== source.replace(/\r\n?/g, '\n')}
          onInsert={insertEquation} onClose={() => setEquationDraft(null)} />
      ) : null}
      {mode === 'read' && readFind !== null ? (
        <ReadFindBar target={overlayRef} content={readHtml} translator={translator} focusNonce={readFind}
          onClose={() => { setReadFind(null); overlayRef.current?.focus(); }} />
      ) : null}
      {mode === 'read' ? (
        <div
          ref={overlayRef}
          className="read-overlay markdown-note"
          role="document"
          tabIndex={0}
          aria-label={translator('a11y.editor.readPane', { name: pathName })}
          dangerouslySetInnerHTML={{ __html: readHtml }}
          onClick={handleReadPaneClick}
          onKeyDown={(event) => {
            const target = event.target;
            if (event.key === 'Enter' && target instanceof HTMLImageElement && target.classList.contains('md-image')) {
              event.preventDefault();
              setViewer({ url: target.src, alt: target.alt });
            }
          }}
        />
      ) : null}
      {viewer !== null ? (
        <ImageViewer url={viewer.url} alt={viewer.alt} translator={translator} onClose={() => setViewer(null)} />
      ) : null}
    </section>
  );
}
