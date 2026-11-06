import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { WorkspaceActions } from './WorkspaceActions';

it('connects the named trigger to a native dismissible popover and preserves disabled actions', () => {
  const html = renderToStaticMarkup(<WorkspaceActions label="Workspace tools">
    <section><button type="button" disabled>Rename</button><button type="button">Export</button></section>
  </WorkspaceActions>);
  const target = /popoverTarget="([^"]+)"/i.exec(html)?.[1];
  expect(target).toBeTruthy();
  expect(html).toContain(`id="${target}"`);
  expect(html).toContain('popover="auto"');
  expect(html).toContain('aria-label="Workspace tools"');
  expect(html).toContain('disabled="">Rename');
  expect(html).toContain('>Export</button>');
  expect(html).not.toContain('role="menu"');
});
