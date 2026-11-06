import { expect, it, vi } from 'vitest';
import { SaveController } from '../editor/save-controller';
import type { CatalogRefresh, ServiceOutcome } from '../editor/ports';
import type { NoteDocument } from '../domain';
import { createCatalogSync } from './catalog-sync';

const original: NoteDocument = { noteId: 'note', vaultId: 'vault', relativePath: 'Old.md', source: 'original text', localRevision: 1, contentHash: 'h1', readOnly: false };
const external: NoteDocument = { ...original, relativePath: 'Moved.md', source: 'external text', localRevision: 2, contentHash: 'h2' };
function catalog(document = external): CatalogRefresh { return { notes: [document], changes: [], unavailableNoteIds: [], findings: [], fingerprint: document.contentHash, deferred: false, persisted: true, writable: true }; }
function harness() {
  const controller = new SaveController({ saveNote: vi.fn() }, { uuid: () => 'operation' }); controller.registerDocument(original);
  let settle!: (value: ServiceOutcome<NoteDocument>) => void;
  const read = vi.fn(() => new Promise<ServiceOutcome<NoteDocument>>(resolve => { settle = resolve; }));
  let document = original;
  const updated = vi.fn((value: NoteDocument) => { document = value; });
  const failed = vi.fn();
  const sync = createCatalogSync({ service: { readNote: read }, controller, allowed: () => true, currentSummary: () => document,
    document: updated, metadata: value => { document = { ...document, relativePath: value.relativePath, readOnly: value.readOnly }; }, failed });
  return { controller, sync, read, updated, failed, settle: (value: NoteDocument) => settle({ ok: true, value }) };
}
it('refreshes a clean document once and does not loop if its actual read is newer than the scan', async () => {
  const h = harness(); const pending = h.sync.apply(catalog()); expect(h.read).toHaveBeenCalledOnce();
  const newer = { ...external, localRevision: 3, contentHash: 'h3', source: 'even newer' };
  h.settle(newer); await pending; expect(h.controller.getBufferText('note')).toBe('even newer');
  await h.sync.apply(catalog()); expect(h.read).toHaveBeenCalledOnce(); expect(h.updated).toHaveBeenCalledWith(newer);
});
it('marks a dirty local buffer as conflicted and never reads over it', async () => {
  const h = harness(); h.controller.setBufferText('note', 'my unsaved text'); await h.sync.apply(catalog());
  expect(h.read).not.toHaveBeenCalled(); expect(h.controller.getBufferText('note')).toBe('my unsaved text');
  expect(h.controller.snapshot('note')?.status).toBe('conflict');
});
it('rechecks local edits made while an external read was pending', async () => {
  const h = harness(); const pending = h.sync.apply(catalog()); h.controller.setBufferText('note', 'typed during read');
  h.settle(external); await pending; expect(h.controller.getBufferText('note')).toBe('typed during read');
  expect(h.controller.snapshot('note')?.status).toBe('conflict'); expect(h.updated).not.toHaveBeenCalled();
});
it('does not adopt a late reply after leaving the workspace', async () => {
  const h = harness(); const pending = h.sync.apply(catalog()); h.sync.dispose(); h.settle(external); await pending;
  expect(h.controller.getBufferText('note')).toBe('original text'); expect(h.updated).not.toHaveBeenCalled();
});
it('keeps a missing dirty note as a conflict and preserves its text', async () => {
  const h = harness(); h.controller.setBufferText('note', 'keep this');
  await h.sync.apply({ ...catalog(), unavailableNoteIds: ['note'] });
  expect(h.controller.getBufferText('note')).toBe('keep this'); expect(h.controller.snapshot('note')?.status).toBe('conflict');
  expect(h.read).not.toHaveBeenCalled();
});


it('drops an older catalog read when a newer catalog result is already waiting', async () => {
  const h = harness(); const older = h.sync.apply(catalog());
  const newer = { ...external, localRevision: 3, contentHash: 'newest', source: 'newest text' };
  await h.sync.apply(catalog(newer)); h.settle(external); await older;
  expect(h.updated).not.toHaveBeenCalled(); expect(h.read).toHaveBeenCalledTimes(2);
  h.settle(newer); for (let i = 0; i < 6; i += 1) await Promise.resolve();
  expect(h.controller.getBufferText('note')).toBe('newest text'); expect(h.updated).toHaveBeenCalledWith(newer);
});
