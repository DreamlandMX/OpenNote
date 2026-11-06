import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { LocaleContext } from '../presentation-i18n';
import { FolderActionDialog, type FolderActionPhase } from './FolderActionDialog';

const translator = new LocaleContext('en').translator;
const render = (phase: FolderActionPhase) => renderToStaticMarkup(<FolderActionDialog
  title="Move" summary={['2 notes move.']} confirmLabel="Move" danger={false} phase={phase}
  translator={translator} onConfirm={() => undefined} onResume={() => undefined} onClose={() => undefined}
/>);

it('offers the action only when nothing refuses it', () => {
  const ready = render({ kind: 'review', refusal: null, otherFiles: [], otherCount: 0 });
  expect(ready).toContain('2 notes move.');
  expect(ready).toContain('>Move</button>');
  const refused = render({ kind: 'review', refusal: 'Holds files.', otherFiles: ['a/x.png'], otherCount: 1 });
  expect(refused).toContain('Holds files.');
  expect(refused).toContain('a/x.png');
  expect(refused).not.toContain('>Move</button>');
});

it('cannot be closed while running and resumes after a stop', () => {
  expect(render({ kind: 'running', done: 1, total: 3 })).toMatch(/disabled="">Close/);
  const stopped = render({ kind: 'stopped', done: 1, total: 3, error: 'Locked.' });
  expect(stopped).toContain(translator('folderOp.resume'));
  expect(stopped).toContain('Locked.');
});
