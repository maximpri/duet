import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import { loadAccounts } from "./accounts.ts";
import { loadCatalog } from "./config.ts";
import { Q } from "./rational.ts";
import type { UsageEvent, UsageException, UsageLoad } from "./types.ts";

interface RawRecord {
  id: string;
  account: string;
  meter: string;
  quantity: string;
  unit: string;
  at: number;
}

/** edge-a: CSV with a header row; times without an offset are UTC (see the file header). */
function readEdgeA(text: string): RawRecord[] {
  return parseRecords(text).map((r) => ({
    id: r.event_id,
    account: r.account,
    meter: r.meter,
    quantity: r.quantity,
    unit: r.unit,
    at: Date.parse(r.ts.replace(" ", "T") + "Z"),
  }));
}

/** edge-b: one JSON object per line. */
function readEdgeB(text: string): RawRecord[] {
  const out: RawRecord[] = [];
  for (const line of text.split(/\r?\n/)) {
    if (line.trim() === "") continue;
    const r = JSON.parse(line);
    out.push({
      id: String(r.id),
      account: String(r.account),
      meter: String(r.metric),
      quantity: String(r.value),
      unit: r.unit === undefined ? "" : String(r.unit),
      at: Date.parse(r.at),
    });
  }
  return out;
}

export function loadUsage(dataDir: string): UsageLoad {
  const accounts = loadAccounts(dataDir);
  const catalog = loadCatalog();

  const dir = join(dataDir, "usage");
  const records: RawRecord[] = [];
  for (const name of readdirSync(dir).sort()) {
    const text = readFileSync(join(dir, name), "utf8");
    if (name.endsWith(".csv")) records.push(...readEdgeA(text));
    else if (name.endsWith(".jsonl")) records.push(...readEdgeB(text));
  }

  // Collectors retry on timeouts, so the same event can appear twice in an export.
  const seen = new Set<string>();
  const events: UsageEvent[] = [];
  const counts = new Map<string, UsageException>();
  const report = (code: UsageException["code"], ref: string) => {
    const key = `${code}\u0000${ref}`;
    const x = counts.get(key) ?? { code, ref, count: 0 };
    x.count++;
    counts.set(key, x);
  };
  for (const r of records) {
    const id = r.id.trim();
    if (seen.has(id)) continue;
    seen.add(id);
    const account = accounts.get(r.account.trim());
    if (!account) {
      report("UNKNOWN_ACCOUNT", r.account.trim());
      continue;
    }
    const meter = catalog.meters.get(r.meter.trim());
    if (!meter) {
      report("UNKNOWN_METER", r.meter.trim());
      continue;
    }
    events.push({
      id: id.toLowerCase(),
      accountId: account.id,
      meter: meter.code,
      quantity: Q.parse(r.quantity).toDecimal(),
      at: new Date(r.at).toISOString(),
    });
  }
  events.sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  const exceptions = [...counts.values()].sort((a, b) =>
    a.code !== b.code ? (a.code < b.code ? -1 : 1) : a.ref < b.ref ? -1 : a.ref > b.ref ? 1 : 0,
  );
  return { events, exceptions };
}
