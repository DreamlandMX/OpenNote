import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { FileText, X } from 'lucide-react';
import type { MessageKey, Translator } from '../presentation-i18n';
import {
  breadcrumbOf,
  quickOpenTitle,
  rankQuickOpen,
  truncateQuery,
  type QuickOpenCandidate,
} from './quick-open';
import {
  orderByRecent,
  type CommandAvailability,
  type CommandDefinition,
} from './commands';
import { handleTrappedTab } from './focus-trap';

const MAX_ROWS = 50;

interface QuickOpenProps {
  notes: readonly { noteId: string; relativePath: string }[];
  recentIds: readonly string[];
  translator: Translator;
  onOpen(noteId: string): void;
  onClose(): void;
  onRecent(noteId: string): void;
}

/**
 * S20 quick open. Ctrl+P; matches titles (and paths as a fallback) without
 * touching a file name (F13-R01), prints every path so same-titled notes stay
 * distinguishable (F13-R02), and shows an empty state instead of inventing a
 * note (F13-R04). Focus returns to the trigger on close (F13-A03).
 */
export function QuickOpen({
  notes,
  recentIds,
  translator,
  onOpen,
  onClose,
  onRecent,
}: QuickOpenProps) {
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const openerRef = useRef<Element | null>(null);

  const candidates = useMemo<QuickOpenCandidate[]>(
    () =>
      notes.map((note) => ({
        noteId: note.noteId,
        relativePath: note.relativePath,
        title: quickOpenTitle(note.relativePath),
      })),
    [notes],
  );

  const capped = truncateQuery(query);
  const ranked = useMemo(() => {
    const matches = rankQuickOpen(candidates, capped.value);
    if (capped.value.trim() === '') {
      return orderByRecent(matches, recentIds, (candidate) => candidate.noteId);
    }
    return matches;
  }, [candidates, capped.value, recentIds]);
  const rows = ranked.slice(0, MAX_ROWS);
  const active = rows[Math.min(activeIndex, Math.max(0, rows.length - 1))];

  useEffect(() => {
    openerRef.current = document.activeElement;
    inputRef.current?.focus();
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);

  function open(noteId: string | undefined): void {
    if (noteId === undefined) {
      return;
    }
    onRecent(noteId);
    onOpen(noteId);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === 'Tab') {
      if (handleTrappedTab(event.currentTarget, document.activeElement, event.shiftKey)) {
        event.preventDefault();
      }
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (rows.length === 0) {
        return;
      }
      event.preventDefault();
      const step = event.key === 'ArrowDown' ? 1 : -1;
      setActiveIndex((index) => {
        const current = Math.min(index, rows.length - 1);
        return (current + step + rows.length) % rows.length;
      });
      return;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      open(active?.noteId);
    }
  }

  return (
    <div className="command-overlay" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) {
        onClose();
      }
    }}>
      <div
        className="command-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="quick-open-heading"
        onKeyDown={handleKeyDown}
      >
        <h2 id="quick-open-heading" className="command-heading">
          {translator('quickOpen.title')}
        </h2>
        <div className="command-field">
          <input
            ref={inputRef}
            type="text"
            value={query}
            autoComplete="off"
            spellCheck={false}
            role="combobox"
            aria-expanded={rows.length > 0 ? 'true' : 'false'}
            aria-controls="quick-open-results"
            aria-autocomplete="list"
            aria-activedescendant={active !== undefined ? `quick-open-${active.noteId}` : undefined}
            aria-label={translator('quickOpen.inputLabel')}
            placeholder={translator('quickOpen.placeholder')}
            onChange={(event) => {
              setQuery(event.target.value);
              setActiveIndex(0);
            }}
          />
          {query !== '' ? (
            <button
              type="button"
              className="command-clear"
              aria-label={translator('quickOpen.clear')}
              title={translator('quickOpen.clear')}
              onClick={() => {
                setQuery('');
                setActiveIndex(0);
                inputRef.current?.focus();
              }}
            >
              <X size={18} aria-hidden="true" />
            </button>
          ) : null}
        </div>
        {capped.truncated ? (
          <p className="command-note" role="status">
            {translator('quickOpen.truncated')}
          </p>
        ) : null}
        {rows.length === 0 ? (
          <p className="command-empty" role="status">
            {translator('quickOpen.empty')}
          </p>
        ) : (
          <ul className="command-list" id="quick-open-results" role="listbox" aria-label={translator('quickOpen.resultsLabel')}>
            {rows.map((candidate, index) => (
              <li
                key={candidate.noteId}
                id={`quick-open-${candidate.noteId}`}
                role="option"
                aria-selected={index === Math.min(activeIndex, rows.length - 1) ? 'true' : 'false'}
                className="command-row"
                onMouseEnter={() => setActiveIndex(index)}
                onClick={() => open(candidate.noteId)}
              >
                <FileText size={18} aria-hidden="true" />
                <span className="command-row-main">
                  <strong>{candidate.title}</strong>
                  <small>{breadcrumbOf(candidate.relativePath)}</small>
                </span>
                {recentIds.includes(candidate.noteId) && query.trim() === '' ? (
                  <span className="command-badge">{translator('quickOpen.recentBadge')}</span>
                ) : null}
              </li>
            ))}
          </ul>
        )}
        <footer className="command-footer">
          <span><kbd>Enter</kbd> {translator('quickOpen.open')}</span>
          <span><kbd>Esc</kbd> {translator('quickOpen.close')}</span>
        </footer>
      </div>
    </div>
  );
}

interface CommandPaletteProps {
  commands: readonly CommandDefinition[];
  recentIds: readonly string[];
  availability: CommandAvailability;
  translator: Translator;
  onRun(command: CommandDefinition): void;
  onRecent(commandId: string): void;
  onClose(): void;
}

/**
 * S21 command palette. Searches localized names, stable ids and English aliases
 * (F13-R01), shows why a command is unavailable instead of pretending to run
 * it (F13-R03), and never fabricates a command for a failed query (F13-R04).
 */
export function CommandPalette({
  commands,
  recentIds,
  availability,
  translator,
  onRun,
  onRecent,
  onClose,
}: CommandPaletteProps) {
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const openerRef = useRef<Element | null>(null);

  const search = useMemo(
    () =>
      commands.map((command) => ({
        command,
        label: translator(command.labelKey),
        reason: command.enabledReason(availability),
      })),
    [commands, availability, translator],
  );

  type PaletteEntry = { command: CommandDefinition; label: string; reason: MessageKey | null };
  const ranked = useMemo<PaletteEntry[]>(() => {
    const needle = query.trim().toLocaleLowerCase();
    if (needle === '') {
      return orderByRecent(search, recentIds, (entry) => entry.command.id);
    }
    const matches: { entry: PaletteEntry; score: number }[] = [];
    for (const entry of search) {
      let best: number | null = null;
      for (const candidate of [entry.label, ...entry.command.aliases, entry.command.id]) {
        const haystack = candidate.toLocaleLowerCase();
        const index = haystack.indexOf(needle);
        if (index >= 0) {
          const score = haystack === needle ? 0 : index === 0 ? 1 : 2;
          if (best === null || score < best) {
            best = score;
          }
        }
      }
      if (best !== null) {
        matches.push({ entry, score: best });
      }
    }
    matches.sort((left, right) => {
      if (left.score !== right.score) {
        return left.score - right.score;
      }
      return left.entry.command.id.localeCompare(right.entry.command.id);
    });
    return matches.map((match) => match.entry);
  }, [search, query, recentIds]);

  const rows = ranked.slice(0, MAX_ROWS);
  const active = rows[Math.min(activeIndex, Math.max(0, rows.length - 1))];

  useEffect(() => {
    openerRef.current = document.activeElement;
    inputRef.current?.focus();
    return () => {
      const opener = openerRef.current;
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus();
      }
    };
  }, []);

  function run(entry: PaletteEntry | undefined): void {
    if (entry === undefined || entry.reason !== null) {
      return;
    }
    onRecent(entry.command.id);
    onRun(entry.command);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === 'Tab') {
      if (handleTrappedTab(event.currentTarget, document.activeElement, event.shiftKey)) {
        event.preventDefault();
      }
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (rows.length === 0) {
        return;
      }
      event.preventDefault();
      const step = event.key === 'ArrowDown' ? 1 : -1;
      setActiveIndex((index) => {
        const current = Math.min(index, rows.length - 1);
        return (current + step + rows.length) % rows.length;
      });
      return;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      run(active);
    }
  }

  return (
    <div className="command-overlay" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) {
        onClose();
      }
    }}>
      <div
        className="command-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="command-palette-heading"
        onKeyDown={handleKeyDown}
      >
        <h2 id="command-palette-heading" className="command-heading">
          {translator('commandPalette.title')}
        </h2>
        <div className="command-field">
          <input
            ref={inputRef}
            type="text"
            value={query}
            autoComplete="off"
            spellCheck={false}
            role="combobox"
            aria-expanded={rows.length > 0 ? 'true' : 'false'}
            aria-controls="command-palette-results"
            aria-autocomplete="list"
            aria-activedescendant={active !== undefined ? `command-${active.command.id}` : undefined}
            aria-label={translator('commandPalette.inputLabel')}
            placeholder={translator('commandPalette.placeholder')}
            onChange={(event) => {
              setQuery(event.target.value);
              setActiveIndex(0);
            }}
          />
        </div>
        {rows.length === 0 ? (
          <p className="command-empty" role="status">
            {translator('commandPalette.empty')}
          </p>
        ) : (
          <ul className="command-list" id="command-palette-results" role="listbox" aria-label={translator('commandPalette.resultsLabel')}>
            {rows.map((entry, index) => {
              const unavailable = entry.reason !== null;
              return (
                <li
                  key={entry.command.id}
                  id={`command-${entry.command.id}`}
                  role="option"
                  aria-selected={index === Math.min(activeIndex, rows.length - 1) ? 'true' : 'false'}
                  aria-disabled={unavailable ? 'true' : undefined}
                  className="command-row"
                  data-disabled={unavailable ? 'true' : undefined}
                  onMouseEnter={() => setActiveIndex(index)}
                  onClick={() => run(entry)}
                >
                  <span className="command-row-main">
                    <strong>{entry.label}</strong>
                    {entry.reason !== null ? (
                      <small>{translator(entry.reason)}</small>
                    ) : null}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
        <footer className="command-footer">
          <span><kbd>Enter</kbd> {translator('commandPalette.run')}</span>
          <span><kbd>Esc</kbd> {translator('commandPalette.close')}</span>
        </footer>
      </div>
    </div>
  );
}
