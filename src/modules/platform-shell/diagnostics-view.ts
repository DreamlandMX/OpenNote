import type {
  CapabilityKind,
  CountKind,
  DiagnosticField,
  DiagnosticFieldValue,
  DiagnosticScope,
  DiagnosticStatus,
  DiagnosticsPreview,
  MeasuredCount,
} from '../editor';

/**
 * S54/S60 diagnostics presentation.
 *
 * M11 owns the allowlist and the redaction; this module only decides what is
 * shown. The rule it owns: an unknown value must read as "unknown", never as a
 * zero or as "healthy", because a broken counter that displays 0 looks exactly
 * like an idle system (F44-R04).
 */

export type StatusKey =
  | 'diagnostics.status.ready'
  | 'diagnostics.status.degraded'
  | 'diagnostics.status.unknown';

export function statusKey(status: DiagnosticStatus): StatusKey {
  if (status.state === 'ready') {
    return 'diagnostics.status.ready';
  }
  if (status.state === 'degraded') {
    return 'diagnostics.status.degraded';
  }
  return 'diagnostics.status.unknown';
}

export type ScopeKey =
  | 'diagnostics.scope.storage'
  | 'diagnostics.scope.network'
  | 'diagnostics.scope.entitlement'
  | 'diagnostics.scope.backup'
  | 'diagnostics.scope.compile'
  | 'diagnostics.scope.application';

/** The localized label for a subsystem row (F44-R05). */
export function scopeKey(scope: DiagnosticScope): ScopeKey {
  switch (scope) {
    case 'storage':
      return 'diagnostics.scope.storage';
    case 'network':
      return 'diagnostics.scope.network';
    case 'entitlement':
      return 'diagnostics.scope.entitlement';
    case 'backup':
      return 'diagnostics.scope.backup';
    case 'compile':
      return 'diagnostics.scope.compile';
    case 'application':
      return 'diagnostics.scope.application';
  }
}

/** Only `ready` may be drawn as healthy. */
export function isHealthy(status: DiagnosticStatus): boolean {
  return status.state === 'ready';
}

/** A field value as text, with an explicit label for the unknown case. */
export function fieldText(value: DiagnosticFieldValue, unknownLabel: string): string {
  switch (value.type) {
    case 'text':
      return value.value;
    case 'integer':
      return String(value.value);
    case 'boolean':
      return value.value ? '✓' : '—';
    case 'unknown':
    default:
      return unknownLabel;
  }
}

/** True when the value could not be read. */
export function isUnknownValue(value: DiagnosticFieldValue): boolean {
  return value.type === 'unknown';
}

/** A field with its display text resolved, for rendering and testing. */
export interface DisplayField {
  key: string;
  text: string;
  unknown: boolean;
}

export function displayFields(
  fields: readonly DiagnosticField[],
  unknownLabel: string,
): DisplayField[] {
  return fields.map((field) => ({
    key: field.key,
    text: fieldText(field.value, unknownLabel),
    unknown: isUnknownValue(field.value),
  }));
}

/**
 * The scope rows to show, ordered so a problem is visible first.
 *
 * F44-R01: each subsystem is reported separately, so the order puts anything not
 * ready at the top instead of burying it under healthy rows.
 */
export function orderScopes(
  statuses: DiagnosticsPreview['statuses'],
): DiagnosticsPreview['statuses'] {
  const rank = (status: DiagnosticStatus): number => {
    if (status.state === 'degraded') {
      return 0;
    }
    if (status.state === 'unknown') {
      return 1;
    }
    return 2;
  };
  return [...statuses].sort((left, right) => rank(left.status) - rank(right.status));
}

/** A one-line redaction notice, or null when nothing was removed. */
export function redactionSummary(preview: DiagnosticsPreview): {
  total: number;
  kinds: readonly string[];
} {
  let total = 0;
  const kinds: string[] = [];
  for (const redaction of preview.redactions) {
    total += redaction.count;
    if (!kinds.includes(redaction.kind)) {
      kinds.push(redaction.kind);
    }
  }
  return { total, kinds };
}

/** Whether the report included a log, so the UI can warn before export. */
export function includesLog(preview: DiagnosticsPreview): boolean {
  return preview.includesLog;
}

/** The correlation id, formatted for copying into a bug report. */
export function correlationText(preview: DiagnosticsPreview): string {
  return preview.correlationId;
}

/** Omission keys that must always be shown, per F44 §4. */
export const REQUIRED_OMISSIONS = ['notePaths', 'noteContent', 'credentials'] as const;

/** True when the report names every omission it is required to name. */
export function declaresRequiredOmissions(preview: DiagnosticsPreview): boolean {
  const keys = preview.omitted.map((omission) => omission.key);
  return REQUIRED_OMISSIONS.every((required) => keys.includes(required));
}

/**
 * Turns real probe values into report fields.
 *
 * A null probe means the value could not be read, so it becomes `unknown` rather
 * than `known(0)` (F44-R04). This conversion lives here so the panel cannot
 * accidentally coerce null to zero.
 */
export function countsFromProbe(probe: {
  notes: number | null;
  trashEntries: number | null;
  drafts: number | null;
  compileRunning: number | null;
  compileQueued: number | null;
  searchDocuments: number | null;
}): { kind: CountKind; value: MeasuredCount }[] {
  const measured = (value: number | null): MeasuredCount =>
    value === null ? { state: 'unknown' } : { state: 'known', value };
  return [
    { kind: 'notes', value: measured(probe.notes) },
    { kind: 'trashEntries', value: measured(probe.trashEntries) },
    { kind: 'drafts', value: measured(probe.drafts) },
    // A cold index reports null, which becomes unknown rather than "0 documents".
    { kind: 'searchDocuments', value: measured(probe.searchDocuments) },
  ];
}

/** Capability rows from the probe, keeping an unprobed one unknown. */
export function capabilitiesFromProbe(probe: {
  compileStatus: DiagnosticStatus;
  storageStatus: DiagnosticStatus;
  searchStatus: DiagnosticStatus;
}): { kind: CapabilityKind; status: DiagnosticStatus }[] {
  return [
    // The search capability now comes from the real index status.
    { kind: 'search', status: probe.searchStatus },
    { kind: 'sync', status: { state: 'unknown', reasonKey: 'diagnostics.noSync' } },
    { kind: 'cloudBackup', status: { state: 'unknown', reasonKey: 'diagnostics.noSync' } },
    { kind: 'latexCompile', status: probe.compileStatus },
    { kind: 'telemetry', status: { state: 'unknown', reasonKey: 'diagnostics.telemetryOff' } },
  ];
}

/** Scope rows from the probe. */
export function statusesFromProbe(probe: {
  storageStatus: DiagnosticStatus;
  compileStatus: DiagnosticStatus;
  searchStatus: DiagnosticStatus;
}): DiagnosticsPreview['statuses'] {
  return [
    { scope: 'storage', status: probe.storageStatus },
    { scope: 'compile', status: probe.compileStatus },
    // Search runs locally and has a real probe, so its state replaces the
    // "no probe" placeholder it used to report.
    { scope: 'application', status: probe.searchStatus },
    { scope: 'network', status: { state: 'unknown', reasonKey: 'diagnostics.noSync' } },
    { scope: 'entitlement', status: { state: 'unknown', reasonKey: 'diagnostics.noSync' } },
    { scope: 'backup', status: { state: 'unknown', reasonKey: 'diagnostics.noProbe' } },
  ];
}
