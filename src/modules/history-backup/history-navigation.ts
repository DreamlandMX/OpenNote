export function revisionFocusTarget(key: string, current: number, count: number): number | null {
  if (count <= 0) return null;
  const start = current < 0 || current >= count ? 0 : current;
  switch (key) {
    case 'ArrowDown':
      return Math.min(start + 1, count - 1);
    case 'ArrowUp':
      return Math.max(start - 1, 0);
    case 'Home':
      return 0;
    case 'End':
      return count - 1;
    default:
      return null;
  }
}
