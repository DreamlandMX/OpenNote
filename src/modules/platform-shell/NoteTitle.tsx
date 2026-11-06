import { useEffect, useRef, useState } from 'react';
import type { Translator } from '../presentation-i18n';
import { folderOf, titleOf, titleToPath } from './note-creation';

interface Props {
  relativePath: string;
  editable: boolean;
  /** Selects the title once when it changes, so a new note can be named by typing. */
  focusToken: number | null;
  translator: Translator;
  /** Renames to the given path; false leaves the shown title as it was. */
  onRename(toPath: string): Promise<boolean>;
}

/**
 * The note's file name above the editor, renamed in place: Enter or leaving the
 * field applies it, Escape restores it. A title that leaves nothing usable is
 * restored rather than sent.
 */
export function NoteTitle({ relativePath, editable, focusToken, translator, onRename }: Props) {
  const input = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState(() => titleOf(relativePath));
  const [busy, setBusy] = useState(false);
  // Escape restores the title, and the blur that follows must not apply it.
  const skipBlur = useRef(false);

  useEffect(() => { setDraft(titleOf(relativePath)); }, [relativePath]);
  useEffect(() => {
    if (focusToken === null) return;
    input.current?.focus();
    input.current?.select();
  }, [focusToken]);

  async function commit(): Promise<void> {
    const current = titleOf(relativePath);
    const target = titleToPath(draft, folderOf(relativePath));
    if (draft.trim() === current || target === null || target === relativePath) {
      setDraft(current);
      return;
    }
    setBusy(true);
    const applied = await onRename(target);
    setBusy(false);
    if (!applied) setDraft(current);
  }

  return (
    <input
      ref={input}
      className="note-title"
      type="text"
      value={draft}
      readOnly={!editable || busy}
      aria-label={translator('workspace.title.label')}
      placeholder={translator('workspace.title.untitled')}
      spellCheck={false}
      onChange={(event) => setDraft(event.target.value)}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === 'Enter') {
          event.preventDefault();
          event.currentTarget.blur();
        } else if (event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          skipBlur.current = true;
          setDraft(titleOf(relativePath));
          event.currentTarget.blur();
        }
      }}
      onBlur={() => {
        if (skipBlur.current) { skipBlur.current = false; return; }
        if (editable && !busy) void commit();
      }}
    />
  );
}
