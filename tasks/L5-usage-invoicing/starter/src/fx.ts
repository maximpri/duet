import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import { Q } from "./rational.ts";

export interface FxRow {
  date: string;
  currency: string;
  rate: Q;
}

export function loadFx(dataDir: string): FxRow[] {
  return parseRecords(readFileSync(join(dataDir, "fx.csv"), "utf8")).map((r) => ({
    date: r.date,
    currency: r.currency,
    rate: Q.parse(r.rate),
  }));
}

/** 1 EUR in `currency` for the invoice date `day`. */
export function rateFor(rows: FxRow[], currency: string, day: string): Q {
  if (currency === "EUR") return Q.ONE;
  const own = rows.filter((r) => r.currency === currency);
  const exact = own.find((r) => r.date === day);
  if (exact) return exact.rate;
  // No fixing published on that day: use the most recent fixing on file.
  const latest = own.sort((a, b) => (a.date < b.date ? 1 : a.date > b.date ? -1 : 0))[0];
  if (!latest) throw new Error(`no ${currency} rate for ${day}`);
  return latest.rate;
}
