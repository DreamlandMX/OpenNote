import { describe, expect, it, vi } from 'vitest';
import {
  attachmentCandidates,
  attachmentMarkdown,
  createImageLoader,
  isImageFileName,
  itemMarkdown,
  pastedFileName,
  pdfCitation,
  pdfLinkTarget,
  relativeToNote,
  rewriteImageEmbeds,
} from './attachment-images';
import { liveImageTarget } from './live-preview';

describe('attachment references', () => {
  it('looks beside the note, then from the vault root', () => {
    expect(attachmentCandidates('notes/day/a.md', 'pic.png')).toEqual(['notes/day/pic.png', 'pic.png', 'assets/pic.png']);
    expect(attachmentCandidates('notes/a.md', '../assets/pic.png')).toEqual(['assets/pic.png']);
    expect(attachmentCandidates('a.md', 'assets/pic.png')).toEqual(['assets/pic.png']);
  });

  it('decodes what the Markdown parser encoded and accepts the angle form', () => {
    expect(attachmentCandidates('a.md', 'assets/my%20%E5%9B%BE.png')[0]).toBe('assets/my 图.png');
    expect(attachmentCandidates('a.md', '<assets/my pic.png>')[0]).toBe('assets/my pic.png');
    expect(attachmentCandidates('a.md', 'report.pdf#page=3')[0]).toBe('report.pdf');
  });

  it('never resolves a remote or absolute target, or one outside the vault', () => {
    for (const target of ['https://x.test/a.png', '//x.test/a.png', 'data:image/png;base64,AA', 'C:/a.png', '/a.png', '']) {
      expect(attachmentCandidates('notes/a.md', target)).toEqual([]);
    }
    for (const path of attachmentCandidates('a.md', '../../secret.png')) expect(path).not.toContain('..');
  });

  it('still finds a root attachment after the note moved folders', () => {
    expect(attachmentCandidates('a.md', '../assets/x.png')).toContain('assets/x.png');
    expect(attachmentCandidates('deep/er/a.md', '../assets/x.png')).toContain('assets/x.png');
  });

  it('reads a PDF link and its page', () => {
    expect(pdfLinkTarget('assets/r.pdf#page=3')).toEqual({ target: 'assets/r.pdf', page: 3 });
    expect(pdfLinkTarget('<a b.PDF>')).toEqual({ target: 'a b.PDF', page: null });
    expect(pdfLinkTarget('r.pdf#nameddest')).toEqual({ target: 'r.pdf', page: null });
    expect(pdfLinkTarget('https://x.test/r.pdf')).toBeNull();
    expect(pdfLinkTarget('note.md')).toBeNull();
  });

  it('writes a citation other tools can follow', () => {
    expect(pdfCitation('n/a.md', 'assets/r.pdf', 3, '', 'r.pdf, page 3')).toBe('[r.pdf, page 3](../assets/r.pdf#page=3)');
    expect(pdfCitation('a.md', 'assets/my r.pdf', 2, ' one\ntwo ', 'my r.pdf, page 2')).toBe('> one\n> two\n\n[my r.pdf, page 2](<assets/my r.pdf#page=2>)');
    const written = pdfCitation('a.md', 'assets/r.pdf', 9, '', 'x');
    expect(pdfLinkTarget(written.slice(written.indexOf('(') + 1, -1))).toEqual({ target: 'assets/r.pdf', page: 9 });
  });

  it('writes a path relative to the note', () => {
    expect(relativeToNote('a.md', 'assets/x.png')).toBe('assets/x.png');
    expect(relativeToNote('notes/a.md', 'assets/x.png')).toBe('../assets/x.png');
    expect(relativeToNote('assets/a.md', 'assets/x.png')).toBe('x.png');
    expect(relativeToNote('a/b/c.md', 'a/d/x.png')).toBe('../d/x.png');
  });

  it('wraps a path Markdown would otherwise cut short', () => {
    expect(attachmentMarkdown('a.md', 'assets/x.png', true)).toBe('![](assets/x.png)');
    expect(attachmentMarkdown('n/a.md', 'assets/my pic (1).png', true)).toBe('![](<../assets/my pic (1).png>)');
    expect(attachmentMarkdown('a.md', 'assets/r[1].pdf', false)).toBe('[r\\[1\\].pdf](assets/r[1].pdf)');
  });

  it('keeps the kind the host decided', () => {
    const item = { name: 'x', destination: 'assets/x.png', status: 'stored' as const, errorKey: null };
    expect(itemMarkdown('n/a.md', { ...item, reference: '![](assets/x.png)' })).toBe('![](../assets/x.png)');
    expect(itemMarkdown('n/a.md', { ...item, reference: '[assets/x.png](assets/x.png)' })).toBe('[x.png](../assets/x.png)');
  });

  it('names a generic clipboard image by time and keeps a real name', () => {
    const now = new Date(2026, 10, 4, 9, 5, 7);
    expect(pastedFileName('image.png', 'image/png', now)).toBe('pasted-20261104-090507.png');
    expect(pastedFileName('', 'image/jpeg', now)).toBe('pasted-20261104-090507.jpg');
    expect(pastedFileName('diagram.png', 'image/png', now)).toBe('diagram.png');
  });

  it('turns only image embeds into image syntax', () => {
    const body = 'a ![[x y.png]] b ![[note]] c';
    const spans = [
      { from: 2, to: 14, target: 'x y.png' },
      { from: 17, to: 26, target: 'note' },
    ];
    expect(rewriteImageEmbeds(body, spans)).toBe('a ![](<x y.png>) b ![[note]] c');
    expect(isImageFileName('A.JPEG')).toBe(true);
    expect(isImageFileName('a.pdf')).toBe(false);
  });
});

describe('Live image syntax', () => {
  it('reads local targets and leaves remote ones alone', () => {
    expect(liveImageTarget('![cat](assets/cat.png)')).toEqual({ alt: 'cat', target: 'assets/cat.png' });
    expect(liveImageTarget('![](<my pic (1).png>)')).toEqual({ alt: '', target: 'my pic (1).png' });
    expect(liveImageTarget('![a\\]b](x.png "title")')).toEqual({ alt: 'a\\]b', target: 'x.png' });
    expect(liveImageTarget('![](https://x.test/a.png)')).toBeNull();
    expect(liveImageTarget('![](data:image/png;base64,AA)')).toBeNull();
    expect(liveImageTarget('![]()')).toBeNull();
  });
});

describe('image loader', () => {
  const info = (relativePath: string, mediaType: string | null, sha256 = 'h') => ({ ok: true as const, value: { relativePath, size: 1, sha256, mediaType } });
  const missing = { ok: false as const, error: { code: 'OBJECT_NOT_FOUND', messageKey: 'error.attachmentMissing', params: {} } };

  it('tries the next place only when a file is missing and reuses identical bytes', async () => {
    const attachmentInfo = vi.fn()
      .mockResolvedValueOnce(missing)
      .mockResolvedValue(info('assets/a.png', 'image/png'));
    const readAttachment = vi.fn().mockResolvedValue({ ok: true, value: new Uint8Array([1]) });
    const create = vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:one');
    const loader = createImageLoader({ attachmentInfo, readAttachment } as never);
    expect(await loader.load('n/a.md', 'a.png')).toEqual({ url: 'blob:one' });
    expect(await loader.load('a.md', 'assets/a.png')).toEqual({ url: 'blob:one' });
    expect(readAttachment).toHaveBeenCalledTimes(1);
    create.mockRestore();
  });

  it('refuses bytes that are not an image, whatever the name says', async () => {
    const attachmentInfo = vi.fn().mockResolvedValue(info('a.png', null));
    const readAttachment = vi.fn();
    const loader = createImageLoader({ attachmentInfo, readAttachment } as never);
    expect(await loader.load('a.md', 'a.png')).toBeNull();
    expect(readAttachment).not.toHaveBeenCalled();
  });

  it('does not ask the host about a remote image', async () => {
    const attachmentInfo = vi.fn();
    const loader = createImageLoader({ attachmentInfo, readAttachment: vi.fn() } as never);
    expect(await loader.load('a.md', 'https://x.test/a.png')).toBeNull();
    expect(attachmentInfo).not.toHaveBeenCalled();
  });

  it('releases the oldest URL past the limit', async () => {
    let hash = 0;
    const attachmentInfo = vi.fn().mockImplementation(async () => info('a.png', 'image/png', `h${hash++}`));
    const readAttachment = vi.fn().mockResolvedValue({ ok: true, value: new Uint8Array([1]) });
    let next = 0;
    const create = vi.spyOn(URL, 'createObjectURL').mockImplementation(() => `blob:${next++}`);
    const revoke = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => undefined);
    const loader = createImageLoader({ attachmentInfo, readAttachment } as never, 1);
    await loader.load('a.md', 'a.png');
    await loader.load('a.md', 'b.png');
    expect(revoke).toHaveBeenCalledWith('blob:0');
    create.mockRestore();
    revoke.mockRestore();
  });
});
