// Time-zone helpers built on Intl (no external time library).

const formatters = new Map<string, Intl.DateTimeFormat>();

function formatter(tz: string): Intl.DateTimeFormat {
  let f = formatters.get(tz);
  if (!f) {
    f = new Intl.DateTimeFormat("en-US", {
      timeZone: tz,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
    formatters.set(tz, f);
  }
  return f;
}

/** Offset of `tz` from UTC at the instant `ms`, in milliseconds (local = UTC + offset). */
export function offsetAt(tz: string, ms: number): number {
  const parts: Record<string, number> = {};
  for (const p of formatter(tz).formatToParts(new Date(ms))) {
    if (p.type !== "literal") parts[p.type] = Number(p.value);
  }
  const local = Date.UTC(parts.year, parts.month - 1, parts.day, parts.hour, parts.minute, parts.second);
  return local - Math.floor(ms / 1000) * 1000;
}

/**
 * The instant (epoch ms) of a local wall-clock time in `tz`. `local` is `YYYY-MM-DD` or
 * `YYYY-MM-DD HH:MM`. Wall-clock times inside a time-zone transition are not supported.
 */
export function localToInstant(tz: string, local: string): number {
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:[ T](\d{2}):(\d{2}))?$/.exec(local.trim());
  if (!m) throw new Error(`bad local time ${JSON.stringify(local)}`);
  const wall = Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4] ?? 0), Number(m[5] ?? 0));
  let t = wall - offsetAt(tz, wall);
  const again = offsetAt(tz, t);
  if (wall - again !== t) t = wall - again;
  return t;
}

/** Local calendar date `YYYY-MM-DD` of the instant `ms` in `tz`. */
export function localDate(tz: string, ms: number): string {
  return new Date(ms + offsetAt(tz, ms)).toISOString().slice(0, 10);
}

/** First and last calendar day of a `YYYY-MM` period. */
export function periodDays(period: string): { first: string; last: string } {
  const [y, m] = period.split("-").map(Number);
  const last = new Date(Date.UTC(y, m, 0)).getUTCDate();
  return { first: `${period}-01`, last: `${period}-${String(last).padStart(2, "0")}` };
}

/** The month after a `YYYY-MM` period. */
export function nextPeriod(period: string): string {
  const [y, m] = period.split("-").map(Number);
  return m === 12 ? `${y + 1}-01` : `${y}-${String(m + 1).padStart(2, "0")}`;
}
