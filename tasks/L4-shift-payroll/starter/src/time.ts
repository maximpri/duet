const MINUTE = 60_000;

/** Parses an ISO-8601 timestamp with an explicit offset; returns epoch ms truncated to the minute. */
export function parseOffsetTimestamp(text: string): number {
  const ms = Date.parse(text.trim());
  if (Number.isNaN(ms)) throw new Error(`bad timestamp ${JSON.stringify(text)}`);
  return Math.floor(ms / MINUTE) * MINUTE;
}

/** `YYYY-MM-DDTHH:MM:00Z` for an epoch-ms instant. */
export function toIsoMinute(ms: number): string {
  return new Date(ms).toISOString().slice(0, 16) + ":00Z";
}

/** `YYYY-MM-DD` of an epoch-ms instant, in UTC. */
export function utcDate(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}
