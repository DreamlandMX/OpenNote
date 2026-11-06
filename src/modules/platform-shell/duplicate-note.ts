/**
 * Picks a free copy path next to the source note.
 *
 * The comparison is case-insensitive because the vault lives on a Windows file
 * system (F03-R01); `Note copy.md` and `note COPY.md` are the same target there.
 */
export function duplicatePath(
  relativePath: string,
  existingPaths: Iterable<string>,
  suffix: string,
): string {
  const lower = new Set<string>();
  for (const path of existingPaths) {
    lower.add(path.toLowerCase());
  }
  const slash = Math.max(relativePath.lastIndexOf('/'), relativePath.lastIndexOf('\\'));
  const dot = relativePath.lastIndexOf('.');
  const hasExtension = dot > slash + 1;
  const base = hasExtension ? relativePath.slice(0, dot) : relativePath;
  const extension = hasExtension ? relativePath.slice(dot) : '';
  const label = suffix.trim() === '' ? 'copy' : suffix.trim();
  for (let index = 1; index <= lower.size + 1; index += 1) {
    const numbered = index === 1 ? '' : ` ${index}`;
    const candidate = `${base} ${label}${numbered}${extension}`;
    if (!lower.has(candidate.toLowerCase())) {
      return candidate;
    }
  }
  // Unreachable: the loop offers more candidates than there are taken paths.
  return `${base} ${label} ${lower.size + 2}${extension}`;
}
