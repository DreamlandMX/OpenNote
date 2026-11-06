import { describe, expect, it } from 'vitest';
import type { AttachmentImportItem, AttachmentPlan, PlannedAttachment } from '../editor';
import type { Translator } from '../presentation-i18n';
import {
  attachmentReasonKey,
  attachmentStatusKey,
  insertableReferences,
  formatBytes,
  isBlocking,
  issueKey,
  issueParams,
  issueText,
  kindKey,
  looksLikeSha256,
  parseFileList,
  partitionPlan,
  planVerdict,
  previewKey,
  willStore,
} from './attachment-view';

const translator: Translator = ((key: string) => key) as Translator;

function attachment(overrides: Partial<PlannedAttachment> = {}): PlannedAttachment {
  return {
    destination: 'assets/a.png',
    kind: 'image',
    preview: 'inline',
    size: 1024,
    sha256: 'aa',
    storesBytes: true,
    cachesThumbnail: true,
    exceedsCloudLimit: false,
    reference: '![](assets/a.png)',
    issues: [],
    ...overrides,
  };
}

function plan(attachments: PlannedAttachment[], overrides: Partial<AttachmentPlan> = {}): AttachmentPlan {
  return { attachments, totalBytes: 1024, unsynced: 0, ...overrides };
}

describe('issue classification', () => {
  it('blocks only the issues that would lose the file or place it wrongly', () => {
    expect(isBlocking({ kind: 'emptyName' })).toBe(true);
    expect(isBlocking({ kind: 'unsafePath', path: '../x' })).toBe(true);
    expect(isBlocking({ kind: 'tooLarge', size: 9, limit: 1 })).toBe(true);
    // An empty file is still a file; a duplicate simply needs no new bytes.
    expect(isBlocking({ kind: 'empty', path: 'x' })).toBe(false);
    expect(isBlocking({ kind: 'duplicateContent', existing: 'y' })).toBe(false);
    // A name collision keeps both files, so it is a notice, not a refusal.
    expect(isBlocking({ kind: 'renamedForCollision', from: 'a.png', to: 'a (1).png' })).toBe(false);
  });

  it('maps each issue to a key and parameters', () => {
    expect(issueKey({ kind: 'emptyName' })).toBe('attachment.issue.emptyName');
    expect(issueParams({ kind: 'tooLarge', size: 10, limit: 5 })).toEqual({ size: 10, limit: 5 });
    expect(issueParams({ kind: 'duplicateContent', existing: 'a.png' })).toEqual({
      existing: 'a.png',
    });
    expect(issueKey({ kind: 'renamedForCollision', from: 'a.png', to: 'a (1).png' })).toBe(
      'attachment.issue.renamed',
    );
    expect(issueParams({ kind: 'renamedForCollision', from: 'a.png', to: 'a (1).png' })).toEqual({
      from: 'a.png',
      to: 'a (1).png',
    });
    expect(issueText({ kind: 'unsafePath', path: 'x' }, translator)).toBe(
      'attachment.issue.unsafePath',
    );
  });
});

describe('plan partitioning', () => {
  it('separates stored, duplicate and refused files', () => {
    const value = plan([
      attachment(),
      attachment({ destination: 'assets/b.png', storesBytes: false, issues: [{ kind: 'duplicateContent', existing: 'assets/a.png' }] }),
      attachment({ destination: 'assets/c.png', issues: [{ kind: 'tooLarge', size: 9, limit: 1 }] }),
    ]);
    const split = partitionPlan(value);
    expect(split.storing.map((entry) => entry.destination)).toEqual(['assets/a.png']);
    expect(split.duplicates.map((entry) => entry.destination)).toEqual(['assets/b.png']);
    expect(split.refused.map((entry) => entry.destination)).toEqual(['assets/c.png']);
  });

  it('treats a duplicate as not stored but not refused', () => {
    // Importing the same bytes twice is not a failure, so it must not read as one.
    const duplicate = attachment({
      storesBytes: false,
      issues: [{ kind: 'duplicateContent', existing: 'assets/a.png' }],
    });
    expect(willStore(duplicate)).toBe(false);
    expect(duplicate.issues.some(isBlocking)).toBe(false);
  });

  it('keeps a renamed file in the storing group with a visible notice', () => {
    const renamed = attachment({
      destination: 'assets/a (1).png',
      reference: '![](assets/a (1).png)',
      issues: [{ kind: 'renamedForCollision', from: 'assets/a.png', to: 'assets/a (1).png' }],
    });
    const split = partitionPlan(plan([renamed]));
    expect(split.storing.map((entry) => entry.destination)).toEqual(['assets/a (1).png']);
    expect(willStore(renamed)).toBe(true);
  });

  it('a refused file is never stored even when bytes were planned', () => {
    expect(willStore(attachment({ issues: [{ kind: 'unsafePath', path: '../x' }] }))).toBe(false);
  });
});

describe('plan verdict', () => {
  it('reports an empty plan before anything was planned', () => {
    expect(planVerdict(null)).toBe('attachment.verdict.empty');
  });

  it('distinguishes nothing-new from blocked', () => {
    // Everything already present is not the same as everything refused.
    expect(
      planVerdict(
        plan([
          attachment({
            storesBytes: false,
            issues: [{ kind: 'duplicateContent', existing: 'assets/a.png' }],
          }),
        ]),
      ),
    ).toBe('attachment.verdict.nothingNew');
    expect(
      planVerdict(plan([attachment({ issues: [{ kind: 'tooLarge', size: 9, limit: 1 }] })])),
    ).toBe('attachment.verdict.blocked');
    expect(planVerdict(plan([attachment()]))).toBe('attachment.verdict.ready');
  });

});

describe('labels', () => {
  it('names kinds and previews', () => {
    expect(kindKey('image')).toBe('attachment.kind.image');
    expect(kindKey('unknown')).toBe('attachment.kind.unknown');
    expect(previewKey('systemOpen')).toBe('attachment.preview.systemOpen');
    expect(previewKey('download')).toBe('attachment.preview.download');
  });

  it('formats sizes with the right unit', () => {
    expect(formatBytes(512, translator)).toBe('attachment.bytes');
    expect(formatBytes(2048, translator)).toBe('attachment.kib');
    expect(formatBytes(5 * 1024 * 1024, translator)).toBe('attachment.mib');
    expect(formatBytes(3 * 1024 * 1024 * 1024, translator)).toBe('attachment.gib');
  });
});

describe('pasted file list', () => {
  it('reads name, size and hash per line', () => {
    expect(parseFileList('a.png | 1024 | abcdef\n')).toEqual({
      files: [{ name: 'a.png', size: 1024, sha256: 'abcdef' }],
      rejected: 0,
    });
  });

  it('rejects a line without a hash rather than guessing one', () => {
    // A wrong hash would skip a real write, so a missing one is not inferred.
    const parsed = parseFileList('a.png | 1024\nb.png | 2048 | abcdef\n');
    expect(parsed.files).toHaveLength(1);
    expect(parsed.rejected).toBe(1);
  });

  it('rejects a non-numeric or negative size', () => {
    expect(parseFileList('a.png | big | abc\n').rejected).toBe(1);
    expect(parseFileList('a.png | -1 | abc\n').rejected).toBe(1);
  });

  it('ignores blank lines and comments', () => {
    expect(parseFileList('\n# note\na.png | 10 | abc\n')).toEqual({
      files: [{ name: 'a.png', size: 10, sha256: 'abc' }],
      rejected: 0,
    });
  });

  it('returns nothing for empty input', () => {
    expect(parseFileList('')).toEqual({ files: [], rejected: 0 });
  });

  it('recognises a sha256 only when it is sixty-four hex characters', () => {
    expect(looksLikeSha256('a'.repeat(64))).toBe(true);
    expect(looksLikeSha256('a'.repeat(63))).toBe(false);
    expect(looksLikeSha256('z'.repeat(64))).toBe(false);
  });
});

describe('real attachment import results', () => {
  const item = (status: AttachmentImportItem['status'], destination: string, reference = `![](${destination})`): AttachmentImportItem =>
    ({ name: destination, destination, reference, status, errorKey: status === 'failed' ? 'error.attachmentPathOccupied' : null });

  it('references only files that exist on disk, once each', () => {
    const text = insertableReferences([
      item('stored', 'assets/a.png'),
      item('alreadyPresent', 'assets/b.png'),
      item('alreadyPresent', 'assets/a.png'),
      item('failed', 'assets/c.png'),
      item('refused', 'assets/d.png'),
      item('stored', 'assets/e.png', ''),
    ]);
    expect(text).toBe('![](assets/a.png)\n![](assets/b.png)');
  });

  it('maps host error keys to short reasons and never to nothing', () => {
    expect(attachmentReasonKey('error.attachmentPathOccupied')).toBe('attachment.reason.occupied');
    expect(attachmentReasonKey('error.pathTraversal')).toBe('attachment.reason.unsafe');
    expect(attachmentReasonKey('error.diskFull')).toBe('attachment.reason.diskFull');
    expect(attachmentReasonKey('something.new')).toBe('attachment.reason.other');
    expect(attachmentReasonKey(null)).toBeNull();
    expect(attachmentStatusKey('alreadyPresent')).toBe('attachment.result.alreadyPresent');
  });
});
