/** Coarse age of `at`, for a row that has room for `5m` and not a timestamp. */
export function relativeAge(at: string, now: Date): string {
  const minutes = Math.max(0, Math.floor((now.getTime() - new Date(at).getTime()) / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}
