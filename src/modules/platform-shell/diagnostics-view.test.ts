import { describe, expect, it } from 'vitest';
import type { DiagnosticScope, DiagnosticsPreview } from '../editor';
import {
  capabilitiesFromProbe,
  correlationText,
  countsFromProbe,
  statusesFromProbe,
  declaresRequiredOmissions,
  displayFields,
  fieldText,
  includesLog,
  isHealthy,
  isUnknownValue,
  orderScopes,
  redactionSummary,
  REQUIRED_OMISSIONS,
  scopeKey,
  statusKey,
} from './diagnostics-view';

function preview(overrides: Partial<DiagnosticsPreview> = {}): DiagnosticsPreview {
  return {
    correlationId: 'cid-1',
    fields: [],
    statuses: [],
    redactions: [],
    omitted: [
      { key: 'notePaths', reasonKey: 'r1' },
      { key: 'noteContent', reasonKey: 'r2' },
      { key: 'credentials', reasonKey: 'r3' },
    ],
    includesLog: false,
    ...overrides,
  };
}

describe('status labels', () => {
  it('only ready is drawn as healthy', () => {
    expect(isHealthy({ state: 'ready' })).toBe(true);
    expect(isHealthy({ state: 'degraded', reasonKey: 'x' })).toBe(false);
    expect(isHealthy({ state: 'unknown', reasonKey: 'x' })).toBe(false);
  });

  it('an unknown status has its own label, not the healthy one', () => {
    // Status unknown must never read as ready.
    expect(statusKey({ state: 'unknown', reasonKey: 'x' })).toBe('diagnostics.status.unknown');
    expect(statusKey({ state: 'ready' })).not.toBe('diagnostics.status.unknown');
  });
});

describe('scope labels', () => {
  it('gives every subsystem its own localized key', () => {
    const scopes: DiagnosticScope[] = [
      'storage',
      'network',
      'entitlement',
      'backup',
      'compile',
      'application',
    ];
    const keys = scopes.map(scopeKey);
    expect(keys).toEqual([
      'diagnostics.scope.storage',
      'diagnostics.scope.network',
      'diagnostics.scope.entitlement',
      'diagnostics.scope.backup',
      'diagnostics.scope.compile',
      'diagnostics.scope.application',
    ]);
    expect(new Set(keys).size).toBe(scopes.length);
  });
});

describe('field values', () => {
  it('renders an unreadable value as a label, never as zero', () => {
    // F44-R04: a broken counter must not look like an idle system.
    expect(fieldText({ type: 'unknown' }, 'Unknown')).toBe('Unknown');
    expect(fieldText({ type: 'integer', value: 0 }, 'Unknown')).toBe('0');
    expect(fieldText({ type: 'unknown' }, 'Unknown')).not.toBe('0');
  });

  it('renders each value type and flags the unknown one', () => {
    expect(fieldText({ type: 'text', value: 'dev' }, 'u')).toBe('dev');
    expect(fieldText({ type: 'boolean', value: true }, 'u')).toBe('✓');
    expect(fieldText({ type: 'boolean', value: false }, 'u')).toBe('—');
    expect(isUnknownValue({ type: 'unknown' })).toBe(true);
    expect(isUnknownValue({ type: 'integer', value: 1 })).toBe(false);
  });

  it('resolves a field list for display', () => {
    const fields = displayFields(
      [
        { key: 'appVersion', value: { type: 'text', value: '0.1.0' } },
        { key: 'count.notes', value: { type: 'unknown' } },
      ],
      'Unknown',
    );
    expect(fields).toEqual([
      { key: 'appVersion', text: '0.1.0', unknown: false },
      { key: 'count.notes', text: 'Unknown', unknown: true },
    ]);
  });
});

describe('scope ordering', () => {
  it('puts degraded first, then unknown, then ready', () => {
    // F44-R01: a problem must not be buried under healthy rows.
    const ordered = orderScopes([
      { scope: 'storage', status: { state: 'ready' } },
      { scope: 'network', status: { state: 'unknown', reasonKey: 'x' } },
      { scope: 'backup', status: { state: 'degraded', reasonKey: 'y' } },
    ]);
    expect(ordered.map((entry) => entry.scope)).toEqual(['backup', 'network', 'storage']);
  });

  it('leaves an already ordered list stable', () => {
    const ordered = orderScopes([
      { scope: 'storage', status: { state: 'ready' } },
      { scope: 'compile', status: { state: 'ready' } },
    ]);
    expect(ordered.map((entry) => entry.scope)).toEqual(['storage', 'compile']);
  });
});

describe('redactions and omissions', () => {
  it('sums redaction counts and lists each kind once', () => {
    const summary = redactionSummary(
      preview({
        redactions: [
          { kind: 'path', count: 3 },
          { kind: 'path', count: 2 },
          { kind: 'email', count: 1 },
        ],
      }),
    );
    expect(summary.total).toBe(6);
    expect(summary.kinds).toEqual(['path', 'email']);
  });

  it('reports nothing removed when there were no redactions', () => {
    expect(redactionSummary(preview())).toEqual({ total: 0, kinds: [] });
  });

  it('requires the three mandatory omissions to be declared', () => {
    expect(declaresRequiredOmissions(preview())).toBe(true);
    const missing = preview({
      omitted: [{ key: 'notePaths', reasonKey: 'r1' }],
    });
    expect(declaresRequiredOmissions(missing)).toBe(false);
    expect(REQUIRED_OMISSIONS).toHaveLength(3);
  });

  it('surfaces the log inclusion and the correlation id', () => {
    expect(includesLog(preview({ includesLog: true }))).toBe(true);
    expect(correlationText(preview())).toBe('cid-1');
  });
});

describe('probe conversion', () => {
  it('turns an unreadable probe value into unknown, never zero', () => {
    // F44-R04: null must not become 0.
    const counts = countsFromProbe({
      notes: null,
      trashEntries: 0,
      drafts: 3,
      compileRunning: null,
      compileQueued: null,
      searchDocuments: null,
    });
    expect(counts[0]).toEqual({ kind: 'notes', value: { state: 'unknown' } });
    expect(counts[1]).toEqual({ kind: 'trashEntries', value: { state: 'known', value: 0 } });
    expect(counts[2]).toEqual({ kind: 'drafts', value: { state: 'known', value: 3 } });
    // A cold index is unknown, not "0 documents".
    expect(counts[3]).toEqual({ kind: 'searchDocuments', value: { state: 'unknown' } });
  });

  it('leaves an unprobed capability unknown instead of claiming it works', () => {
    const capabilities = capabilitiesFromProbe({
      compileStatus: { state: 'unknown', reasonKey: 'diagnostics.noToolchain' },
      storageStatus: { state: 'ready' },
      searchStatus: { state: 'ready' },
    });
    // Search now reflects the real index status rather than a placeholder.
    const search = capabilities.find((entry) => entry.kind === 'search');
    expect(search?.status.state).toBe('ready');
    const compile = capabilities.find((entry) => entry.kind === 'latexCompile');
    expect(compile?.status.state).toBe('unknown');
  });

  it('reports every scope, keeping the unprobed ones unknown', () => {
    const statuses = statusesFromProbe({
      storageStatus: { state: 'ready' },
      compileStatus: { state: 'unknown', reasonKey: 'diagnostics.noToolchain' },
      searchStatus: { state: 'unknown', reasonKey: 'diagnostics.indexCold' },
    });
    expect(statuses.map((entry) => entry.scope)).toEqual([
      'storage',
      'compile',
      'application',
      'network',
      'entitlement',
      'backup',
    ]);
    expect(statuses.find((entry) => entry.scope === 'storage')?.status.state).toBe('ready');
    expect(statuses.find((entry) => entry.scope === 'backup')?.status.state).toBe('unknown');
  });
});
