import { noteNameFor } from './template-view';

export type NewRowPathFailure = 'title-required' | 'invalid-folder';

export type NewRowPath =
  | { ok: true; path: string; name: string }
  | { ok: false; reason: NewRowPathFailure };

/**
 * F22-R03: turns the panel's folder scope and a title into the path of a real
 * Markdown note. Only the UI-level shape is checked here; the host still
 * validates reserved names, length and collisions before writing.
 */
export function newRowPath(folder: string, title: string): NewRowPath {
  if (title.trim() === '') {
    return { ok: false, reason: 'title-required' };
  }
  const name = noteNameFor(title);
  const raw = folder.trim().replace(/\\/g, '/');
  if (raw.startsWith('/') || /^[a-z]:/i.test(raw)) {
    return { ok: false, reason: 'invalid-folder' };
  }
  const trimmed = raw.replace(/\/+$/u, '');
  const segments = trimmed === '' ? [] : trimmed.split('/');
  const invalid = segments.some(
    (segment) =>
      segment === '' ||
      segment === '.' ||
      segment === '..' ||
      segment.toLowerCase() === '.opennote' ||
      /[:"<>|*?\u0000-\u001f\u007f]/u.test(segment) ||
      /[. ]$/u.test(segment),
  );
  if (invalid) {
    return { ok: false, reason: 'invalid-folder' };
  }
  const path = segments.length === 0 ? `${name}.md` : `${segments.join('/')}/${name}.md`;
  return { ok: true, path, name };
}
