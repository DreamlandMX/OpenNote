import { EditorState, type Extension } from '@codemirror/state';
import { keymap, ViewPlugin, type EditorView } from '@codemirror/view';
import { openSearchPanel, search, searchKeymap } from '@codemirror/search';
import type { MessageKey, Translator } from '../presentation-i18n';

/** The search panel's words, by the English text CodeMirror looks them up with. */
const PHRASES: Readonly<Record<string, MessageKey>> = {
  Find: 'find.find',
  Replace: 'find.replace',
  next: 'find.next',
  previous: 'find.previous',
  all: 'find.all',
  'match case': 'find.matchCase',
  'by word': 'find.byWord',
  regexp: 'find.regexp',
  replace: 'find.replaceOne',
  'replace all': 'find.replaceAll',
  close: 'find.close',
  'current match': 'find.currentMatch',
  'on line': 'find.onLine',
  'replaced $ matches': 'find.replacedMany',
  'replaced match on line $': 'find.replacedOne',
  'Go to line': 'find.goToLine',
  go: 'find.go',
};

export function findPhrases(translator: Translator): Record<string, string> {
  return Object.fromEntries(Object.entries(PHRASES).map(([phrase, key]) => [phrase, translator(key)]));
}

/** The replace command: the same panel, with the caret already in the replace box. */
function openReplace(view: EditorView): boolean {
  if (view.state.readOnly) return false;
  openSearchPanel(view);
  view.dom.querySelector<HTMLInputElement>('.cm-search input[name=replace]')?.focus();
  return true;
}

/**
 * Whether a key press runs a command: `armed` is the first step of a two-step
 * chord, which is consumed and waits for the second step.
 */
export type KeyMatch = (event: KeyboardEvent) => 'complete' | 'armed' | 'none';

/** The user's keys for find and replace (rebindable in Settings). */
export interface FindKeys {
  find: KeyMatch;
  replace: KeyMatch;
}

function plainChord(key: string): KeyMatch {
  return (event) =>
    (event.ctrlKey || event.metaKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === key
      ? 'complete'
      : 'none';
}

/** Ctrl/Cmd+F and Ctrl/Cmd+H, used when no binding is supplied. */
export const DEFAULT_FIND_KEYS: FindKeys = { find: plainChord('f'), replace: plainChord('h') };

/**
 * Find and replace in the open text, on the keys `keys()` returns at the time
 * of the press, so a rebinding applies without reopening the note. A replace is
 * one ordinary edit, so undo restores it, and a read-only editor offers no
 * replace box. The keys also work from inside the search panel's inputs.
 */
export function findReplace(translator: Translator, keys: () => FindKeys = () => DEFAULT_FIND_KEYS): Extension {
  const keydown = ViewPlugin.define((view) => {
    const listener = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || event.isComposing) return;
      const current = keys();
      for (const [match, run] of [[current.find, openSearchPanel], [current.replace, openReplace]] as const) {
        const result = match(event);
        if (result === 'none') continue;
        if (result === 'complete' && !run(view)) return;
        event.preventDefault();
        event.stopPropagation();
        return;
      }
    };
    view.dom.addEventListener('keydown', listener, true);
    return { destroy: () => view.dom.removeEventListener('keydown', listener, true) };
  });
  return [
    search({ top: true }),
    EditorState.phrases.of(findPhrases(translator)),
    keydown,
    // CodeMirror's own Mod-f is replaced by the binding above.
    keymap.of(searchKeymap.filter((binding) => binding.key !== 'Mod-f')),
  ];
}
