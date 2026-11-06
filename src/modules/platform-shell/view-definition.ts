import type { DatabaseFilter, DatabaseFilterOp, DatabaseSort } from '../editor';
import { DATABASE_VIEWS, type DatabaseViewType } from './database-views';

/**
 * F22/F23 saved database view: a `.onview.json` file in the vault.
 *
 * Version 1 holds the settings the database panel can already apply: the folder
 * source, view type, filters, sort, and the board/calendar field choices. Fields
 * this version does not know are kept and written back unchanged, a newer
 * `formatVersion` is opened read-only, and an unknown view type or operator is
 * refused instead of being guessed into something else.
 */
export const VIEW_FORMAT_VERSION = 1;
export const VIEW_SUFFIX = '.onview.json';

export interface ViewDefinition {
  formatVersion: number;
  name: string;
  source: { folder: string; recursive: boolean };
  viewType: DatabaseViewType;
  filters: DatabaseFilter[];
  sort: DatabaseSort | null;
  groupKey: string;
  dateKey: string;
  /** Top-level fields this version does not know, kept verbatim. */
  extra: Record<string, unknown>;
}

export type ViewParseResult =
  | { ok: true; value: ViewDefinition; readOnly: boolean }
  | { ok: false; reason: 'json' | 'shape' };

const KNOWN = new Set(['formatVersion', 'name', 'source', 'viewType', 'filters', 'sort', 'groupKey', 'dateKey']);
const OPS: readonly DatabaseFilterOp[] = ['equals', 'notEquals', 'contains', 'isSet', 'isUnset'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(value: unknown, fallback: string): string | null {
  if (value === undefined) return fallback;
  return typeof value === 'string' ? value : null;
}

export function parseViewDefinition(text: string): ViewParseResult {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return { ok: false, reason: 'json' };
  }
  if (!isRecord(raw)) return { ok: false, reason: 'shape' };
  const version = raw.formatVersion;
  if (typeof version !== 'number' || !Number.isInteger(version) || version < 1) {
    return { ok: false, reason: 'shape' };
  }
  const name = str(raw.name, '');
  const viewType = str(raw.viewType, 'table');
  const groupKey = str(raw.groupKey, '');
  const dateKey = str(raw.dateKey, '');
  const source = raw.source === undefined ? {} : raw.source;
  if (name === null || viewType === null || groupKey === null || dateKey === null || !isRecord(source)) {
    return { ok: false, reason: 'shape' };
  }
  if (!DATABASE_VIEWS.includes(viewType as DatabaseViewType)) return { ok: false, reason: 'shape' };
  const folder = str(source.folder, '');
  const recursive = source.recursive === undefined ? true : source.recursive;
  if (folder === null || typeof recursive !== 'boolean') return { ok: false, reason: 'shape' };

  const filters: DatabaseFilter[] = [];
  const rawFilters = raw.filters === undefined ? [] : raw.filters;
  if (!Array.isArray(rawFilters)) return { ok: false, reason: 'shape' };
  for (const entry of rawFilters) {
    if (!isRecord(entry) || typeof entry.key !== 'string' || typeof entry.op !== 'string'
      || !OPS.includes(entry.op as DatabaseFilterOp)) {
      return { ok: false, reason: 'shape' };
    }
    const value = str(entry.value, '');
    if (value === null) return { ok: false, reason: 'shape' };
    filters.push({ key: entry.key, op: entry.op as DatabaseFilterOp, value });
  }

  let sort: DatabaseSort | null = null;
  if (raw.sort !== undefined && raw.sort !== null) {
    const candidate = raw.sort;
    if (!isRecord(candidate) || typeof candidate.key !== 'string'
      || (candidate.direction !== 'ascending' && candidate.direction !== 'descending')) {
      return { ok: false, reason: 'shape' };
    }
    sort = { key: candidate.key, direction: candidate.direction };
  }

  const extra: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(raw)) {
    if (!KNOWN.has(key)) extra[key] = value;
  }
  // Keep unknown fields nested in `source` too.
  const sourceExtra: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(source)) {
    if (key !== 'folder' && key !== 'recursive') sourceExtra[key] = value;
  }
  if (Object.keys(sourceExtra).length > 0) extra['\u0000source'] = sourceExtra;

  return {
    ok: true,
    readOnly: version > VIEW_FORMAT_VERSION,
    value: {
      formatVersion: version,
      name,
      source: { folder, recursive },
      viewType: viewType as DatabaseViewType,
      filters,
      sort,
      groupKey,
      dateKey,
      extra,
    },
  };
}

/** Stable JSON for a definition; unknown fields are written back unchanged. */
export function serializeViewDefinition(definition: ViewDefinition): string {
  const { ['\u0000source']: sourceExtra, ...extra } = definition.extra;
  const output: Record<string, unknown> = {
    formatVersion: definition.formatVersion,
    name: definition.name,
    source: {
      folder: definition.source.folder,
      recursive: definition.source.recursive,
      ...(isRecord(sourceExtra) ? sourceExtra : {}),
    },
    viewType: definition.viewType,
    filters: definition.filters.map((filter) => ({ key: filter.key, op: filter.op, value: filter.value })),
    sort: definition.sort === null ? null : { key: definition.sort.key, direction: definition.sort.direction },
    groupKey: definition.groupKey,
    dateKey: definition.dateKey,
    ...extra,
  };
  return `${JSON.stringify(output, null, 2)}\n`;
}

/** A `.onview.json` path from a typed name; null for empty or traversing names. */
export function viewPathFromName(raw: string): string | null {
  const trimmed = raw.trim().replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  if (trimmed === '' || trimmed.split('/').some((part) => part === '' || part === '.' || part === '..')) {
    return null;
  }
  return trimmed.toLowerCase().endsWith(VIEW_SUFFIX) ? trimmed : `${trimmed}${VIEW_SUFFIX}`;
}

/** A display name for a view file path. */
export function viewNameFromPath(path: string): string {
  const last = path.split('/').pop() ?? path;
  return last.toLowerCase().endsWith(VIEW_SUFFIX) ? last.slice(0, -VIEW_SUFFIX.length) : last;
}
