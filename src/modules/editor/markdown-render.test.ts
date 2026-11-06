import { describe, expect, it } from 'vitest';
import { renderFormula } from './math-render-core';
import {
  renderSafeMarkdown,
  markdownFormulas,
  renderWithEmbeds,
  splitFrontMatter,
  type RenderedEmbed,
  type SafeMarkdownCaptions,
} from './markdown-render';

const CAPTIONS: SafeMarkdownCaptions = {
  frontmatterLabel: 'Frontmatter',
  blockedImageRemote: 'Remote image blocked',
  blockedImageLocal: (source: string) => `Image ${source} blocked`,
  openEmbeddedSource: (path: string) => `Open source: ${path}`,
};

function render(source: string): string {
  return renderSafeMarkdown(CAPTIONS, source);
}

const CAPTIONS_FOR_SLOTS = {
  frontmatterLabel: 'F',
  blockedImageRemote: 'remote',
  blockedImageLocal: (name: string) => `local ${name}`,
  openEmbeddedSource: (path: string) => path,
};

describe('markdown rendering safety', () => {
  it('does not execute raw HTML, script tags or event handlers', () => {
    const cases = [
      '<script>alert(1)</script>',
      '<img src=x onerror="alert(1)">',
      '<div><p>x</p></div>',
      '<iframe src="https://evil.example"></iframe>',
    ];
    for (const source of cases) {
      const html = render(source);
      expect(html).not.toMatch(/<script\b/i);
      expect(html).not.toMatch(/<img\b/i);
      expect(html).not.toMatch(/<iframe\b/i);
      expect(html).not.toMatch(/<div\b/i);
      expect(html).not.toMatch(/<[a-z]+\s+on\w+=/i);
      expect(html).toContain('&lt;');
    }
  });

  it('does not execute or keep javascript, vbscript or data links', () => {
    const cases = ['[click](javascript:alert(1))', '[click](vbscript:msgbox)', '[click](data:text/html;base64,PHNjcmlwdD4=)'];
    for (const source of cases) {
      const html = render(source);
      expect(html).not.toMatch(/<a\b/i);
      expect(html).not.toMatch(/href="(javascript|vbscript|data):/i);
    }
  });

  it('never emits an <img> tag: remote images are blocked without loading them', () => {
    const html = render('![logo](https://evil.example/tracker.png)');
    expect(html).not.toMatch(/<img/i);
    expect(html).toContain('Remote image blocked');

    const relative = render('![note-local](images/photo.png)');
    expect(relative).not.toMatch(/<img/i);
    expect(relative).toContain('Image images/photo.png blocked');

    const mixed = render('![](HTTP://CDN.example/a.png "title")');
    expect(mixed).not.toMatch(/<img/i);
    expect(mixed).not.toMatch(/CDN\.example/);
  });

  it('announces blocked images through an escaped role=img aria-label built from caption and alt', () => {
    const remote = render('![alt "quoted" <tag>](https://cdn.example/x.png)');
    expect(remote).not.toMatch(/<img/i);
    expect(remote).toContain('role="img"');
    expect(
      remote.includes('aria-label="Remote image blocked alt &quot;quoted&quot; &lt;tag&gt;"'),
    ).toBe(true);

    const local = render('![alt (a) "b"](images/pic.png)');
    expect(
      local.includes('aria-label="Image images/pic.png blocked alt (a) &quot;b&quot;"'),
    ).toBe(true);

    const withoutAlt = render('![](https://cdn.example/y.png)');
    expect(withoutAlt.includes('aria-label="Remote image blocked"')).toBe(true);
  });

  it('still renders ordinary markdown text safely', () => {
    const html = render('# Title\n\nA **bold** idea with a [link](https://opennote.example/docs).\n');
    expect(html).toContain('Title');
    expect(html).toContain('<strong>bold</strong>');
    expect(html).toMatch(/<a href="(https:\/\/opennote\.example\/docs)"/);
    expect(html).not.toMatch(/<a href="([^"]*(javascript|vbscript|data):)/i);
  });
});

describe('frontmatter splitting', () => {
  it('hides the internal note identity in reading views without changing user properties or source', () => {
    const id = 'e9f47772-2b1e-4708-b86c-23c35b86fed4';
    const source = `---\nopennote-id: ${id}\n---\nBody`;
    for (const reader of [render, (text: string) => renderWithEmbeds(CAPTIONS, text, [])]) {
      const html = reader(source);
      expect(html).not.toContain('md-frontmatter');
      expect(html).not.toContain(id);
      expect(html).toContain('Body');
      const custom = reader(`---\nopennote-id: ${id}\ntitle: My note\n---\nBody`);
      expect(custom).toContain('title: My note');
      expect(custom).not.toContain(id);
      expect(reader('---\nopennote-id: user text\n---\nBody')).toContain('opennote-id: user text');
    }
    expect(splitFrontMatter(source).frontMatter).toContain(id);
    const embedded = renderWithEmbeds(CAPTIONS, '![[note]]', [{
      raw: '![[note]]', content: source, placeholder: null, targetPath: 'note.md', targetId: 'note', children: [],
    }]);
    expect(embedded).not.toContain(id);
    expect(embedded).toContain('Body');
  });

  it('splits leading frontmatter and renders it as escaped text', () => {
    const source = '---\ntitle: "Plan & <notes>"\ntags: [a,b]\n---\n\n# Body\n';
    const { frontMatter, body } = splitFrontMatter(source);
    expect(body).toBe('\n# Body\n');
    expect(frontMatter).not.toBeNull();
    const html = render(source);
    expect(html).toContain('Frontmatter');
    expect(html).toContain('&quot;Plan &amp; &lt;notes&gt;&quot;');
    expect(html).not.toMatch(/<meta/i);
    expect(html).toContain('Body');
  });

  it('treats non-frontmatter dashes as body text', () => {
    const { frontMatter, body } = splitFrontMatter('not frontmatter\n---\ncontent');
    expect(frontMatter).toBeNull();
    expect(body).toBe('not frontmatter\n---\ncontent');
  });

  it('handles a leading Unicode BOM in frontmatter documents and plain notes', () => {
    const source = '\uFEFF---\r\ntitle: Plan\r\n---\r\n\r\n# Body\r\n';
    const { frontMatter, body } = splitFrontMatter(source);
    expect(frontMatter).toBe('---\r\ntitle: Plan\r\n---');
    expect(body).toBe('\r\n# Body\r\n');
    expect(frontMatter).not.toContain('\uFEFF');
    const html = render(source);
    expect(html).toContain('Frontmatter');
    expect(html).not.toContain('\uFEFF');

    const plain = splitFrontMatter('\uFEFFplain & text');
    expect(plain.frontMatter).toBeNull();
    expect(plain.body).toBe('plain & text');
  });

  it('accepts an unindented closing --- with CRLF line endings', () => {
    const source = '---\r\ntitle: A\r\n---\r\n\r\n# Body\r\n';
    const { frontMatter, body } = splitFrontMatter(source);
    expect(frontMatter).toBe('---\r\ntitle: A\r\n---');
    expect(body).toBe('\r\n# Body\r\n');
    expect(render(source)).toContain('title: A');
  });

  it('accepts an unindented closing ... with CRLF line endings and at end of file', () => {
    const source = '---\r\ntitle: A\r\n...\r\n\r\n# Body\r\n';
    const { frontMatter, body } = splitFrontMatter(source);
    expect(frontMatter).toBe('---\r\ntitle: A\r\n...');
    expect(body).toBe('\r\n# Body\r\n');
    const html = render(source);
    expect(html).toContain('Frontmatter');
    expect(html).toContain('title: A');
    expect(html).toContain('Body');

    const atEof = splitFrontMatter('---\r\ntitle: A\r\n...');
    expect(atEof.frontMatter).toBe('---\r\ntitle: A\r\n...');
    expect(atEof.body).toBe('');
  });

  it('keeps an indented --- or ... inside a literal scalar as frontmatter text', () => {
    const source = '---\ntitle: |\n  a\n  ---\n  ...\nother: 1\n---\n\n# Body\n';
    const { frontMatter, body } = splitFrontMatter(source);
    expect(frontMatter).toBe('---\ntitle: |\n  a\n  ---\n  ...\nother: 1\n---');
    expect(body).toBe('\n# Body\n');
    const html = render(source);
    expect(html).toContain('Frontmatter');
    expect(html).toContain('title: |');
    expect(html).toContain('  ---');
    expect(html).toContain('  ...');
    expect(html).toContain('other: 1');
  });

  it('does not treat a later unindented --- beyond the real frontmatter as part of it', () => {
    const source = '---\ntitle: A\n---\n\nbody line\n\n---\nafter separator\n';
    const { frontMatter, body } = splitFrontMatter(source);
    expect(body).toBe('\nbody line\n\n---\nafter separator\n');
    expect(frontMatter).toBe('---\ntitle: A\n---');
  });
});

describe('inline embeds', () => {
  function renderEmbeds(source: string, embeds: RenderedEmbed[]): string {
    return renderWithEmbeds(CAPTIONS, source, embeds);
  }

  it('inlines a resolved embed as a nested section', () => {
    const html = renderEmbeds('before ![[b]] after', [
      {
        raw: '![[b]]',
        content: '## Two\n\nbody',
        placeholder: null,
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [],
      },
    ]);
    expect(html).toContain('class="md-embed"');
    expect(html).toContain('data-embed-path="notes/b.md"');
    expect(html).toContain('body');
    // The raw syntax is gone once it resolves.
    expect(html).not.toContain('![[b]]');
    // Surrounding prose is still rendered.
    expect(html).toContain('before');
    expect(html).toContain('after');
  });

  it('offers a keyboard-reachable source button for a resolved embed', () => {
    const html = renderEmbeds('![[b]]', [
      {
        raw: '![[b]]',
        content: 'body',
        placeholder: null,
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [],
      },
    ]);
    expect(html).toContain('class="md-embed-source"');
    expect(html).toContain('<button type="button"');
    expect(html).toContain('data-embed-open="note-b"');
    expect(html).toContain('Open source: notes/b.md');
  });

  it('keeps the placeholder jump button when the target identity is known', () => {
    const html = renderEmbeds('![[b#missing]]', [
      {
        raw: '![[b#missing]]',
        content: null,
        placeholder: 'The referenced heading or block was not found',
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [],
      },
    ]);
    expect(html).toContain('md-embed-placeholder');
    expect(html).toContain('data-embed-open="note-b"');
    expect(html).toContain('The referenced heading or block was not found');
  });

  it('does not fabricate a source button when a broken link has no identity', () => {
    const html = renderEmbeds('![[missing]]', [
      {
        raw: '![[missing]]',
        content: null,
        placeholder: 'The embedded note no longer exists',
        targetPath: null,
        targetId: null,
        children: [],
      },
    ]);
    expect(html).toContain('md-embed-source');
    expect(html).toContain('Open source: missing');
    expect(html).not.toContain('data-embed-open');
    expect(html).not.toContain('<button');
  });

  it('escapes the source path in the label and attribute', () => {
    const html = renderEmbeds('![[b]]', [
      {
        raw: '![[b]]',
        content: 'body',
        placeholder: null,
        targetPath: 'notes/<b>"&.md',
        targetId: 'note<b>',
        children: [],
      },
    ]);
    expect(html).not.toContain('<b>');
    expect(html).toContain('notes/&lt;b&gt;&quot;&amp;.md');
    expect(html).toContain('data-embed-path="notes/&lt;b&gt;&quot;&amp;.md"');
    expect(html).toContain('data-embed-open="note&lt;b&gt;"');
  });

  it('renders a placeholder for every failure and keeps the raw syntax visible', () => {
    const html = renderEmbeds('![[b]]', [
      {
        raw: '![[b]]',
        content: null,
        placeholder: 'Circular reference',
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [],
      },
    ]);
    expect(html).toContain('md-embed-placeholder');
    expect(html).toContain('Circular reference');
    // The user can still see what was written.
    expect(html).toContain('![[b]]');
  });

  it('keeps the frontmatter block when a note also has embeds', () => {
    const source = '---\ntitle: A\n---\n\n![[b]]\n';
    const html = renderEmbeds(source, [
      {
        raw: '![[b]]',
        content: 'body',
        placeholder: null,
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [],
      },
    ]);
    expect(html).toContain('md-frontmatter');
    expect(html).toContain('title: A');
    expect(html).toContain('class="md-embed"');
  });

  it('renders an unresolved embed as its raw text, not as an empty hole', () => {
    const html = renderEmbeds('![[b]]', []);
    expect(html).toContain('![[b]]');
    expect(html).not.toContain('md-embed');
  });

  it('does not trust embedded content: HTML and remote images stay blocked', () => {
    const html = renderEmbeds('![[b]]', [
      {
        raw: '![[b]]',
        content: '<script>alert(1)</script>\n\n![x](https://evil.example/t.png)',
        placeholder: null,
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [],
      },
    ]);
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('evil.example/t.png');
    expect(html).toContain('md-image-blocked');
    const slots = renderSafeMarkdown({ ...CAPTIONS_FOR_SLOTS, localImageSlots: true }, '![a b](assets/x%20y.png)\n\n![r](https://x.test/a.png)\n\n![d](data:image/png;base64,AA)');
    expect(slots).toContain('class="md-image-slot" data-attachment-src="assets/x%20y.png" data-image-alt="a b"');
    expect(slots.match(/md-image-slot/g)).toHaveLength(1);
    expect(slots.match(/md-image-blocked/g)).toHaveLength(2);
    expect(slots).not.toContain('<img');
  });

  it('leaves the source untouched when a note has no embeds', () => {
    expect(renderEmbeds('plain body', [])).toBe(render('plain body'));
  });

  it('does not inline an embed written inside a code fence', () => {
    const fence = '`'.repeat(3);
    const source = `${fence}\n![[b]]\n${fence}\n`;
    const html = renderEmbeds(source, [
      {
        raw: '![[b]]',
        content: 'SECRET',
        placeholder: null,
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [],
      },
    ]);
    expect(html).not.toContain('SECRET');
  });

  it('inlines a nested embed inside the embedded section', () => {
    const html = renderEmbeds('![[b]]', [
      {
        raw: '![[b]]',
        content: 'B top\n\n![[c]]\n',
        placeholder: null,
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [
          {
            raw: '![[c]]',
            content: 'leaf',
            placeholder: null,
            targetPath: 'notes/c.md',
            targetId: 'note-c',
            children: [],
          },
        ],
      },
    ]);
    expect(html.match(/class="md-embed"/g)).toHaveLength(2);
    expect(html).toContain('data-embed-open="note-c"');
    expect(html).toContain('leaf');
    expect(html).not.toContain('![[c]]');
  });

  it('renders a nested cycle as a placeholder without recursing', () => {
    const html = renderEmbeds('![[b]]', [
      {
        raw: '![[b]]',
        content: '![[a]]',
        placeholder: null,
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [
          {
            raw: '![[a]]',
            content: null,
            placeholder: 'Circular reference',
            targetPath: 'notes/a.md',
            targetId: 'note-a',
            children: [],
          },
        ],
      },
    ]);
    expect(html).toContain('md-embed-placeholder');
    expect(html).toContain('Circular reference');
    expect(html).toContain('![[a]]');
    expect(html).toContain('data-embed-open="note-a"');
  });

  it('renders escaped frontmatter inside an embedded note', () => {
    const html = renderEmbeds('![[b]]', [
      {
        raw: '![[b]]',
        content: '---\ntitle: B\n---\n\nbody',
        placeholder: null,
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [],
      },
    ]);
    expect(html).toContain('md-frontmatter');
    expect(html).toContain('title: B');
    expect(html).toContain('body');
  });

  it('stops expanding at the render depth limit', () => {
    const html = renderEmbeds('![[b]]', [
      {
        raw: '![[b]]',
        content: '![[c]]',
        placeholder: null,
        targetPath: 'notes/b.md',
        targetId: 'note-b',
        children: [
          {
            raw: '![[c]]',
            content: '![[d]]',
            placeholder: null,
            targetPath: 'notes/c.md',
            targetId: 'note-c',
            children: [
              {
                raw: '![[d]]',
                content: '![[e]]',
                placeholder: null,
                targetPath: 'notes/d.md',
                targetId: 'note-d',
                children: [
                  {
                    raw: '![[e]]',
                    content: 'TOO DEEP',
                    placeholder: null,
                    targetPath: 'notes/e.md',
                    targetId: 'note-e',
                    children: [],
                  },
                ],
              },
            ],
          },
        ],
      },
    ]);
    expect(html).toContain('![[e]]');
    expect(html).not.toContain('TOO DEEP');
  });
});

describe('math rendering', () => {
  it('collects visible formulas and applies worker results while preserving source', () => {
    const source = '---\nvalue: $secret$\n---\n**bold $x$**\n\n    $code$\n\n$$\nx+y\n$$';
    const inputs = markdownFormulas(source);
    expect(inputs).toEqual([{ tex: 'x', delimiter: 'inline' }, { tex: 'x+y', delimiter: 'block' }]);
    const html = renderSafeMarkdown(CAPTIONS, source, inputs.map(renderFormula));
    expect(html).toContain('<strong>bold <span');
    expect(html).toContain('class="katex"');
    expect(html).toContain('katex-display');
    expect(html).toContain('<code>$code$');
    expect(source).toContain('$x$');
  });

  it('uses the same result map for nested embedded formulas', () => {
    const embed: RenderedEmbed = { raw: '![[child]]', content: '$x$', placeholder: null, targetPath: 'child.md', targetId: 'child', children: [] };
    expect(renderWithEmbeds(CAPTIONS, '![[child]]', [embed], [renderFormula({ tex: 'x', delimiter: 'inline' })])).toContain('class="katex"');
  });

  it('recognizes standalone block delimiters with legal Markdown indentation', () => {
    expect(markdownFormulas('  $$\nx+y\n  $$')).toEqual([{ tex: 'x+y', delimiter: 'block' }]);
    expect(markdownFormulas('> $$\n> x+y\n> $$')).toEqual([{ tex: 'x+y', delimiter: 'block' }]);
  });

  it('escapes parse-error source and localizes its position', () => {
    const tex = String.raw`\frac{<img src=x onerror=alert(1)>`;
    const html = renderSafeMarkdown({ ...CAPTIONS, mathHint: (_key, position) => `Invalid at ${position}` }, `$${tex}$`,
      [renderFormula({ tex, delimiter: 'inline' })]);
    expect(html).not.toContain('<img');
    expect(html).toContain('&lt;img');
    expect(html).toContain('Invalid at');
  });
  it.each([
    ['**bold** $x$', '<strong>bold</strong>'],
    ['[site](https://example.com) $x$', '<a href="https://example.com">site</a>'],
    ['use `$code$` and $x$', '<code>$code$</code>'],
    ['**value $x$**', '<strong>value '],
  ])('preserves surrounding markup: %s', (source, expected) => {
    expect(render(source)).toContain(expected);
  });

  it('leaves math inside link destinations and indented code untouched', () => {
    expect(render('[a](https://example.com/$x$)')).not.toContain('md-math');
    expect(render('    $x$')).not.toContain('md-math');
  });
  it('renders a formula as a distinct element carrying its source', () => {
    // A reader must be able to tell a formula from a price.
    const html = render('the value $x + y$ here');
    expect(html).toContain('md-math-inline');
    expect(html).toContain('data-tex="x + y"');
  });

  it('leaves two prices as ordinary text', () => {
    // F06-R02: `$5 and $10` must not become a formula that swallows the words.
    const html = render('costs $5 and $10 today');
    expect(html).not.toContain('md-math');
    expect(html).toContain('$5 and $10');
  });

  it('does not treat a dollar inside code as a formula', () => {
    const html = render('use `$x$` literally');
    expect(html).not.toContain('md-math');
    expect(html).toContain('$x$');
  });

  it('marks an over-long formula as source rather than dropping it', () => {
    // F06-R09: shown with a hint, never removed and never rendered anyway.
    const longFormula = 'a'.repeat(8200);
    const html = render('$' + longFormula + '$');
    expect(html).toContain('md-math-source');
    expect(html).toContain('md-math-hint');
    expect(html).toContain('math.hint.tooLong');
  });

  it('renders a block formula with its own class', () => {
    const html = render('$$\na = b\n$$');
    expect(html).toContain('md-math-block');
  });
});

describe('highlight and comment syntax', () => {
  it('renders ==text== as <mark>, nested with emphasis', () => {
    expect(render('a ==b **c**== d')).toContain('<p>a <mark>b <strong>c</strong></mark> d</p>');
    expect(render('**==both==**')).toContain('<strong><mark>both</mark></strong>');
  });

  it('leaves unpaired or code equals signs alone', () => {
    expect(render('a == b')).toContain('<p>a == b</p>');
    expect(render('`==x==`')).toContain('<code>==x==</code>');
    expect(render('x === y')).not.toContain('<mark>');
  });

  it('escapes HTML inside a highlight', () => {
    const html = render('==<script>alert(1)</script>==');
    expect(html).toContain('<mark>&lt;script&gt;alert(1)&lt;/script&gt;</mark>');
    expect(html).not.toContain('<script>');
  });

  it('drops inline %% comments, including formulas and links inside them', () => {
    const html = render('keep %%secret $x$ [[Note]]%% this');
    expect(html).toContain('<p>keep  this</p>');
    expect(html).not.toContain('secret');
    expect(markdownFormulas('keep %%secret $x$%% this')).toEqual([]);
  });

  it('drops a block %% comment across lines and blank lines', () => {
    const html = render('before\n\n%%\n# hidden\n\nmore hidden\n%%\nafter');
    expect(html).toContain('<p>before</p>');
    expect(html).toContain('<p>after</p>');
    expect(html).not.toContain('hidden');
    expect(render('%% one line %%\n\nshown')).not.toContain('one line');
  });

  it('runs an unclosed block comment to the end, but leaves a lone %% in a paragraph', () => {
    expect(render('shown\n\n%% open\nrest')).not.toContain('rest');
    expect(render('100%% sure, 50%')).toContain('100%% sure, 50%');
    expect(render('text\n%% open\n\nnext')).toContain('next');
  });

  it('does not treat %% inside code as a comment', () => {
    expect(render('`%%x%%`')).toContain('<code>%%x%%</code>');
    expect(render('```\n%%\nkept\n```')).toContain('kept');
  });
});
