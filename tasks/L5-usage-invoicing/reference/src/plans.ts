import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import { localToInstant } from "./time.ts";

export interface SubscriptionRow {
  plan: string;
  /** Local `YYYY-MM-DD HH:MM` in the root's time zone. */
  effectiveFrom: string;
}

export interface Segment {
  /** Plan code; `none` when no plan is in force. */
  plan: string;
  /** Epoch ms, inclusive. */
  start: number;
  /** Epoch ms, exclusive. */
  end: number;
}

export function loadSubscriptions(dataDir: string): Map<string, SubscriptionRow[]> {
  const out = new Map<string, SubscriptionRow[]>();
  for (const r of parseRecords(readFileSync(join(dataDir, "subscriptions.csv"), "utf8"))) {
    const rows = out.get(r.account_id) ?? [];
    rows.push({ plan: r.plan, effectiveFrom: r.effective_from });
    out.set(r.account_id, rows);
  }
  return out;
}

/** Splits `[start, end)` where the plan in force changes (RULES.md §3). */
export function segmentsOf(rows: SubscriptionRow[], tz: string, start: number, end: number): Segment[] {
  const changes = rows
    .map((r) => ({ plan: r.plan, at: localToInstant(tz, r.effectiveFrom) }))
    .sort((a, b) => a.at - b.at);
  let current = "none";
  for (const c of changes) if (c.at <= start) current = c.plan;
  const out: Segment[] = [];
  let from = start;
  for (const c of changes) {
    if (c.at <= start || c.at >= end) continue;
    if (c.plan === current) continue;
    out.push({ plan: current, start: from, end: c.at });
    current = c.plan;
    from = c.at;
  }
  out.push({ plan: current, start: from, end });
  return out;
}
