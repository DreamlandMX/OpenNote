import type { Translator } from '../presentation-i18n';
import type { SnapshotReferenceReport } from './ports';

export function ReferenceIssues({ report, t }: { report: SnapshotReferenceReport; t: Translator }) {
  if (report.issues.length === 0) return null;
  return <section className="notice-panel" aria-live="polite">
    <ul>{report.issues.map(issue => <li key={`${issue.noteId}:${issue.reason}:${issue.reference}`}><strong>{t(issue.reason === 'missing' ? 'backup.referencesMissing' : 'backup.referencesUnsafe')}</strong> · {issue.notePath} · <code>{issue.reference}</code></li>)}</ul>
  </section>;
}
