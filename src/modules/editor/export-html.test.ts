import { describe, expect, it, vi } from 'vitest';
import { fillImageSlots, noteDocument, noteTitle, renderNoteForExport } from './export-html';

const CAPTIONS = {
  frontmatterLabel: 'F',
  blockedImageRemote: 'remote blocked',
  blockedImageLocal: (name: string) => `no ${name}`,
  openEmbeddedSource: (path: string) => path,
};

describe('single-note HTML', () => {
  it('fills an image, or its caption when it cannot be shown', async () => {
    const html = '<p><span class="md-image-slot" data-attachment-src="a&amp;b.png" data-image-alt="x &quot;y&quot;" data-image-failed="no a.png"></span>'
      + '<span class="md-image-slot" data-attachment-src="c.png" data-image-alt="" data-image-failed="no c.png"></span></p>';
    const seen: string[] = [];
    const out = await fillImageSlots(html, async (target) => {
      seen.push(target);
      return target === 'c.png' ? null : 'data:image/png;base64,AA';
    });
    expect(seen).toEqual(['a&b.png', 'c.png']);
    expect(out).toContain('<img class="md-image" src="data:image/png;base64,AA" alt="x &quot;y&quot;">');
    expect(out).toContain('<span class="md-image-blocked">[no c.png]</span>');
    expect(out).not.toContain('md-image-slot');
  });

  it('is a page that cannot run anything or reach the network', () => {
    const page = noteDocument('a <b>', '<p>x</p>', '', 'zh-CN');
    expect(page).toContain("default-src 'none'");
    expect(page).toContain('<title>a &lt;b&gt;</title>');
    expect(page).not.toContain('<script');
    expect(page).not.toMatch(/https?:\/\//);
  });

  it('keeps raw HTML from a note inert', async () => {
    const service = { attachmentInfo: vi.fn(), readAttachment: vi.fn() };
    const out = await renderNoteForExport(service as never, 'a.md', '# t\n\n<script>alert(1)</script>\n\n![r](https://x.test/a.png)', CAPTIONS);
    expect(out.body).not.toContain('<script');
    expect(out.body).toContain('remote blocked');
    expect(service.attachmentInfo).not.toHaveBeenCalled();
  });

  it('embeds a local image found beside the note', async () => {
    const service = {
      attachmentInfo: vi.fn().mockResolvedValue({ ok: true, value: { relativePath: 'n/x.png', size: 1, sha256: 'h', mediaType: 'image/png' } }),
      readAttachment: vi.fn().mockResolvedValue({ ok: true, value: new Uint8Array([104, 105]) }),
    };
    const out = await renderNoteForExport(service as never, 'n/a.md', '![[x.png]]', CAPTIONS);
    expect(out.body).toContain('src="data:image/png;base64,aGk="');
  });

  it('names the file after the note', () => {
    expect(noteTitle('a/b/My note.md')).toBe('My note');
  });
});
