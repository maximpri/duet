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

/** Day of month of the last Sunday of `month` (0-based) in `year`. */
function lastSunday(year: number, month: number): number {
  const last = new Date(Date.UTC(year, month + 1, 0));
  return last.getUTCDate() - last.getUTCDay();
}

/** Europe/Berlin offset from UTC, in minutes, at an instant (EU rule: summer time 01:00 UTC last Sunday of March to 01:00 UTC last Sunday of October). */
export function berlinOffsetMinutes(ms: number): number {
  const year = new Date(ms).getUTCFullYear();
  const start = Date.UTC(year, 2, lastSunday(year, 2), 1);
  const end = Date.UTC(year, 9, lastSunday(year, 9), 1);
  return ms >= start && ms < end ? 120 : 60;
}

/**
 * Epoch ms of a Europe/Berlin wall-clock time. An ambiguous time (the repeated
 * hour when summer time ends) resolves to its first occurrence, in summer time.
 */
export function berlinToUtc(year: number, month: number, day: number, hour: number, minute: number): number {
  const wall = Date.UTC(year, month - 1, day, hour, minute);
  const summer = wall - 120 * MINUTE;
  if (berlinOffsetMinutes(summer) === 120) return summer;
  return wall - 60 * MINUTE;
}

/** Local wall-clock parts of an instant in Europe/Berlin. */
export function berlinParts(ms: number): { date: string; hour: number; minute: number } {
  const local = new Date(ms + berlinOffsetMinutes(ms) * MINUTE);
  return { date: local.toISOString().slice(0, 10), hour: local.getUTCHours(), minute: local.getUTCMinutes() };
}

/** Parses `DD.MM.YYYY HH:MM` or `YYYY-MM-DD HH:MM` as Europe/Berlin wall-clock time. */
export function parseBerlinLocal(text: string): number {
  const t = text.trim();
  let m = /^(\d{2})\.(\d{2})\.(\d{4})\s+(\d{1,2}):(\d{2})$/.exec(t);
  if (m) return berlinToUtc(+m[3], +m[2], +m[1], +m[4], +m[5]);
  m = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{1,2}):(\d{2})$/.exec(t);
  if (m) return berlinToUtc(+m[1], +m[2], +m[3], +m[4], +m[5]);
  throw new Error(`bad local time ${JSON.stringify(text)}`);
}

/** Monday (`YYYY-MM-DD`) of the week containing the calendar date `date`. */
export function mondayOf(date: string): string {
  const d = new Date(`${date}T00:00:00Z`);
  const back = (d.getUTCDay() + 6) % 7;
  return new Date(d.getTime() - back * 86_400_000).toISOString().slice(0, 10);
}
