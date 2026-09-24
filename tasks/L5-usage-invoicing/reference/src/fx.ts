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

/** 1 EUR in `currency`, from the latest row on or before `day` (RULES.md §6.2). */
export function rateFor(rows: FxRow[], currency: string, day: string): Q {
  if (currency === "EUR") return Q.ONE;
  let best: FxRow | null = null;
  for (const r of rows) {
    if (r.currency !== currency || r.date > day) continue;
    if (!best || r.date > best.date) best = r;
  }
  if (!best) throw new Error(`no ${currency} rate on or before ${day}`);
  return best.rate;
}
