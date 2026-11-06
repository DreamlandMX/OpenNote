import { useEffect, useRef } from 'react';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { Compartment, EditorState } from '@codemirror/state';
import { EditorView, highlightActiveLine, keymap, lineNumbers } from '@codemirror/view';
import { latexHighlighting } from './latex-highlight';
import { DEFAULT_FIND_KEYS, findReplace, type FindKeys } from '../editor/find-replace';
import type { Translator } from '../presentation-i18n';

export interface LatexSourceEditorProps {
  /** Identifies the open file; a new one replaces the text and the undo history. */
  fileKey: string;
  value: string;
  readOnly: boolean;
  label: string;
  translator: Translator;
  /** Find and replace keys from Settings. */
  findKeys?: FindKeys;
  /** A line to put the caret on; the nonce makes a repeat request count. */
  jump: { line: number; nonce: number } | null;
  onChange(text: string): void;
  /** Ctrl/Cmd+Enter. */
  onBuild(): void;
  /** Ctrl/Cmd+J: find the caret's line in the PDF. */
  onLocate(line: number): void;
  /** The one-based line the caret is on, whenever it moves. */
  onCaretLine(line: number): void;
  /** Ctrl/Cmd+S. */
  onSave(): void;
}

/** The LaTeX source pane: CodeMirror with line numbers and LaTeX colouring. */
export function LatexSourceEditor(props: LatexSourceEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const editable = useRef(new Compartment());

  // One view per file, so undo never crosses into another file.
  useEffect(() => {
    if (host.current === null) return;
    const created = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: latest.current.value,
        extensions: [
          lineNumbers(),
          highlightActiveLine(),
          history(),
          findReplace(latest.current.translator, () => latest.current.findKeys ?? DEFAULT_FIND_KEYS),
          EditorView.lineWrapping,
          latexHighlighting(),
          editable.current.of(EditorState.readOnly.of(latest.current.readOnly)),
          keymap.of([
            { key: 'Mod-Enter', run: () => { latest.current.onBuild(); return true; } },
            { key: 'Mod-s', run: () => { latest.current.onSave(); return true; } },
            { key: 'Mod-j', run: (target) => { latest.current.onLocate(target.state.doc.lineAt(target.state.selection.main.head).number); return true; } },
            indentWithTab,
            ...defaultKeymap,
            ...historyKeymap,
          ]),
          EditorView.contentAttributes.of({ 'aria-label': latest.current.label, spellcheck: 'false' }),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) latest.current.onChange(update.state.doc.toString());
            if (update.docChanged || update.selectionSet) latest.current.onCaretLine(update.state.doc.lineAt(update.state.selection.main.head).number);
          }),
        ],
      }),
    });
    view.current = created;
    return () => {
      created.destroy();
      view.current = null;
    };
  }, [props.fileKey]);

  // Text replaced from outside (a reload after a conflict) without losing the view.
  useEffect(() => {
    const current = view.current;
    if (current === null || current.state.doc.toString() === props.value) return;
    current.dispatch({ changes: { from: 0, to: current.state.doc.length, insert: props.value } });
  }, [props.value]);

  useEffect(() => {
    view.current?.dispatch({ effects: editable.current.reconfigure(EditorState.readOnly.of(props.readOnly)) });
  }, [props.readOnly]);

  useEffect(() => {
    const current = view.current;
    if (current === null || props.jump === null) return;
    const line = current.state.doc.line(Math.min(Math.max(1, props.jump.line), current.state.doc.lines));
    current.dispatch({ selection: { anchor: line.from }, effects: EditorView.scrollIntoView(line.from, { y: 'center' }) });
    current.focus();
  }, [props.jump]);

  return <div className="latex-ws-editor" ref={host} />;
}
