import type { NoteService, ParsedHeading, ServiceOutcome } from '../editor/ports';
import { serviceErrorFromThrown } from './port';

export interface OutlineSnapshot {
  noteId: string;
  source: string;
  outcome: ServiceOutcome<readonly ParsedHeading[]>;
}

export function currentOutline(snapshot: OutlineSnapshot | null, noteId: string | null, source: string): OutlineSnapshot['outcome'] | null {
  return snapshot?.noteId === noteId && snapshot.source === source ? snapshot.outcome : null;
}

export function observeOutline(service: Pick<NoteService, 'parseHeadings'>, noteId: string, source: string, publish: (snapshot: OutlineSnapshot) => void): () => void {
  let active = true;
  const timer = setTimeout(async () => {
    try {
      const outcome = await service.parseHeadings(source);
      if (active) publish({ noteId, source, outcome });
    } catch (error) {
      if (active) publish({ noteId, source, outcome: { ok: false, error: serviceErrorFromThrown(error) } });
    }
  }, 150);
  return () => {
    active = false;
    clearTimeout(timer);
  };
}
