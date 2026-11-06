import { useEffect, useMemo, useState } from 'react';
import { List, Minus, Network, Plus } from 'lucide-react';
import { PanelHeader } from './PanelHeader';
import { useOverlayFocus } from './use-overlay-focus';
import type { GraphReport, NoteService } from '../editor';
import type { ServiceError } from '../domain';
import type { Translator } from '../presentation-i18n';
import { serviceErrorMessage, type SupportedLocaleId } from '../presentation-i18n';
import { NODE_RADIUS, edgeLine, layoutGraph, summarizeGraph } from './graph-view';

/** The local graph reaches 1 to 3 links out from the open note. */
const MIN_DEPTH = 1;
const MAX_DEPTH = 3;

interface Props {
  service: NoteService;
  translator: Translator;
  locale: SupportedLocaleId;
  /** The open note, which centres the local graph; null shows the whole vault. */
  focusNoteId: string | null;
  onOpenNote(noteId: string): void;
  onClose(): void;
}

/**
 * S22/S23 relationship graph.
 *
 * The picture is a deterministic circular layout, and every fact it shows is
 * also available in the list alternative with real buttons: F15-R04 forbids
 * relying on colour or the mouse alone. A missing target is drawn and listed as
 * a missing node rather than being hidden (F15-R03).
 */
export function GraphPanel({
  service,
  translator,
  locale,
  focusNoteId,
  onOpenNote,
  onClose,
}: Props) {
  const panelRef = useOverlayFocus<HTMLElement>();
  const [report, setReport] = useState<GraphReport | null>(null);
  const [error, setError] = useState<ServiceError | null>(null);
  const [loading, setLoading] = useState(false);
  const [depth, setDepth] = useState(1);
  const [local, setLocal] = useState(focusNoteId !== null);
  const [asList, setAsList] = useState(false);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void service
      .graphReport(local ? focusNoteId : null, local ? depth : 1, 500)
      .then((outcome) => {
        if (cancelled) {
          return;
        }
        setLoading(false);
        if (outcome.ok) {
          setReport(outcome.value);
          setError(null);
        } else {
          setError(outcome.error);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [service, local, focusNoteId, depth]);

  const filtered = useMemo(() => {
    if (report === null) {
      return null;
    }
    const trimmed = query.trim().toLocaleLowerCase();
    if (trimmed === '') {
      return report;
    }
    const nodes = report.nodes.filter(
      (node) =>
        node.title.toLocaleLowerCase().includes(trimmed) ||
        node.relativePath.toLocaleLowerCase().includes(trimmed),
    );
    const ids = new Set(nodes.map((node) => node.id));
    return {
      ...report,
      nodes,
      edges: report.edges.filter((edge) => ids.has(edge.from) && ids.has(edge.to)),
    };
  }, [report, query]);

  const layout = useMemo(() => (filtered === null ? null : layoutGraph(filtered)), [filtered]);
  const summary = filtered === null ? null : summarizeGraph(filtered);
  const selectedNode = layout?.nodes.find((node) => node.id === selected) ?? null;
  // With no links at all the drawing is one lonely dot; the empty state says more.
  const noLinks = filtered !== null && filtered.edges.length === 0 && query.trim() === '' && !loading;

  return (
    <aside
      className="graph-panel"
      aria-labelledby="graph-heading"
      ref={panelRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <PanelHeader id="graph-heading" icon={Network} title={translator('graph.title')}
        purpose={translator('graph.purpose')} closeLabel={translator('graph.close')} onClose={onClose} />

      <div className="panel-toolbar graph-controls">
        <div className="panel-segmented" role="group" aria-label={translator('graph.scope')}>
          <button type="button" aria-pressed={local ? 'true' : 'false'} disabled={focusNoteId === null} onClick={() => setLocal(true)}>
            {translator('graph.aroundNote')}
          </button>
          <button type="button" aria-pressed={local ? 'false' : 'true'} onClick={() => setLocal(false)}>
            {translator('graph.wholeVault')}
          </button>
        </div>
        {local ? (
          <div className="graph-depth" role="group" aria-label={translator('graph.depth')}>
            <button type="button" className="panel-tool icon-only" aria-label={translator('graph.fewer')} title={translator('graph.fewer')}
              disabled={depth <= MIN_DEPTH} onClick={() => setDepth(current => Math.max(MIN_DEPTH, current - 1))}>
              <Minus size={14} aria-hidden="true" />
            </button>
            <output aria-live="polite">{translator('graph.layers', { count: depth })}</output>
            <button type="button" className="panel-tool icon-only" aria-label={translator('graph.more')} title={translator('graph.more')}
              disabled={depth >= MAX_DEPTH} onClick={() => setDepth(current => Math.min(MAX_DEPTH, current + 1))}>
              <Plus size={14} aria-hidden="true" />
            </button>
          </div>
        ) : null}
        <label className="graph-search">
          <span className="visually-hidden">{translator('graph.filter')}</span>
          <input
            type="search"
            role="searchbox"
            value={query}
            placeholder={translator('graph.filterShort')}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <button
          type="button"
          className="panel-tool"
          aria-pressed={asList ? 'true' : 'false'}
          onClick={() => setAsList((current) => !current)}
        >
          <List size={15} aria-hidden="true" /> {translator('graph.listView')}
        </button>
      </div>

      {loading ? <p role="status">{translator('graph.loading')}</p> : null}
      {error !== null ? (
        <div className="graph-error" role="alert">
          <p>{serviceErrorMessage(locale, error.code)}</p>
        </div>
      ) : null}
      {report !== null && report.unreadable.length > 0 ? (
        <p className="graph-warning" role="status">
          {translator('graph.incomplete', { count: report.unreadable.length })}
        </p>
      ) : null}
      {summary !== null && summary.truncated ? (
        <p className="graph-warning" role="status">
          {translator('graph.truncated')}
        </p>
      ) : null}

      {summary !== null ? (
        <p className="graph-summary-line">
          {translator('graph.summary', {
            notes: summary.notes,
            missing: summary.missing,
            links: summary.links,
            embeds: summary.embeds,
          })}
        </p>
      ) : null}

      {noLinks ? (
        <div className="panel-empty" role="status">
          <strong>{translator(local ? 'graph.emptyNote' : 'graph.emptyVault')}</strong>
          <p>{translator('graph.emptyBody')}</p>
        </div>
      ) : null}

      {layout !== null && !asList && !noLinks ? (
        <svg
          className="graph-canvas"
          viewBox={`${-layout.radius - 40} ${-layout.radius - 40} ${layout.radius * 2 + 80} ${layout.radius * 2 + 80}`}
          role="img"
          aria-label={translator('graph.canvasLabel')}
        >
          {layout.edges.map((edge) => {
            const from = layout.nodes.find((node) => node.id === edge.from);
            const to = layout.nodes.find((node) => node.id === edge.to);
            if (from === undefined || to === undefined) {
              return null;
            }
            return (
              <path
                key={`${edge.from}->${edge.to}:${edge.kind}`}
                d={edgeLine(from, to)}
                className={edge.kind === 'embed' ? 'graph-edge embed' : 'graph-edge'}
                strokeDasharray={edge.kind === 'embed' ? '4 3' : undefined}
                data-dangling={edge.dangling ? 'true' : undefined}
              />
            );
          })}
          {layout.nodes.map((node) => (
            <g
              key={node.id}
              className="graph-node"
              data-kind={node.kind}
              data-selected={selected === node.id ? 'true' : undefined}
              data-anchor={node.anchor ? 'true' : undefined}
              transform={`translate(${node.x} ${node.y})`}
            >
              <circle r={node.anchor ? NODE_RADIUS + 3 : NODE_RADIUS} />
              <text y={NODE_RADIUS + 14} textAnchor="middle">
                {node.title}
              </text>
              {/* A real hit target so the graph is usable by keyboard and touch. */}
              <circle
                r={18}
                className="graph-hit"
                role="button"
                tabIndex={0}
                aria-label={
                  node.kind === 'missing'
                    ? translator('graph.missingNode', { name: node.title })
                    : translator('graph.openNode', { name: node.title })
                }
                onClick={() => {
                  setSelected(node.id);
                  if (node.kind === 'note') {
                    onOpenNote(node.id);
                  }
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    setSelected(node.id);
                    if (node.kind === 'note') {
                      onOpenNote(node.id);
                    }
                  }
                }}
              />
            </g>
          ))}
        </svg>
      ) : null}
      {layout !== null && !asList && !noLinks ? (
        <ul className="graph-legend" aria-label={translator('graph.legend')}>
          <li><span className="graph-swatch" data-kind="anchor" aria-hidden="true" />{translator('graph.legend.current')}</li>
          <li><span className="graph-swatch" data-kind="note" aria-hidden="true" />{translator('graph.legend.linked')}</li>
          <li><span className="graph-swatch" data-kind="missing" aria-hidden="true" />{translator('graph.legend.missing')}</li>
          <li><span className="graph-swatch" data-kind="embed" aria-hidden="true" />{translator('graph.legend.embed')}</li>
        </ul>
      ) : null}

      {layout !== null ? (
        <ul className="graph-list" aria-label={translator('graph.listLabel')}>
          {layout.nodes.map((node) => (
            <li key={node.id} className="graph-row">
              <button
                type="button"
                className="graph-row-open"
                disabled={node.kind === 'missing'}
                aria-describedby={`graph-meta-${node.id}`}
                onClick={() => onOpenNote(node.id)}
              >
                {node.title}
              </button>
              <span id={`graph-meta-${node.id}`} className="graph-row-meta">
                {node.kind === 'missing'
                  ? translator('graph.missing')
                  : translator('graph.hops', { count: node.depth })}
                {node.relativePath !== '' ? ` · ${node.relativePath}` : ''}
              </span>
            </li>
          ))}
        </ul>
      ) : null}

      {selectedNode !== null ? (
        <p className="graph-selected" role="status">
          {translator('graph.selected', {
            name: selectedNode.title,
            count: selectedNode.depth,
          })}
        </p>
      ) : null}
      <p className="panel-note">{translator('graph.howTo')}</p>
    </aside>
  );
}
