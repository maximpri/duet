import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import { loadAccounts } from "./accounts.ts";
import { loadCatalog } from "./config.ts";
import { Q } from "./rational.ts";
import type { UsageEvent, UsageException, UsageLoad } from "./types.ts";

/** Factor from an accepted unit to the canonical unit (RULES.md §2.2), per canonical unit. */
const UNITS: Record<string, Record<string, Q>> = {
  request: { request: Q.ONE },
  "GiB-h": { "GiB-h": Q.ONE, "MiB-h": Q.of(1n, 1024n) },
  GB: { GB: Q.ONE, MB: Q.of(1n, 1000n), B: Q.of(1n, 10n ** 9n) },
  min: { min: Q.ONE },
};

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

/** Account spellings differ between systems: case and zero-padding of the number. */
function accountKey(raw: string): string {
  return raw.trim().toUpperCase().replace(/^([A-Z]+)-0*(\d)/, "$1-$2");
}

export function loadUsage(dataDir: string): UsageLoad {
  const accounts = loadAccounts(dataDir);
  const catalog = loadCatalog();
  const byKey = new Map<string, string>();
  for (const id of accounts.keys()) byKey.set(accountKey(id), id);

  const dir = join(dataDir, "usage");
  const records: RawRecord[] = [];
  for (const name of readdirSync(dir).sort()) {
    const text = readFileSync(join(dir, name), "utf8");
    if (name.endsWith(".csv")) records.push(...readEdgeA(text));
    else if (name.endsWith(".jsonl")) records.push(...readEdgeB(text));
  }

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
    const id = r.id.trim().toLowerCase();
    if (seen.has(id)) continue;
    seen.add(id);
    const accountId = byKey.get(accountKey(r.account));
    if (!accountId) {
      report("UNKNOWN_ACCOUNT", r.account.trim());
      continue;
    }
    const meter = catalog.meters.get(r.meter.trim());
    if (!meter) {
      report("UNKNOWN_METER", r.meter.trim());
      continue;
    }
    const unit = r.unit.trim() === "" ? meter.unit : r.unit.trim();
    const factor = UNITS[meter.unit]?.[unit];
    if (!factor) throw new Error(`event ${r.id}: unit ${unit} is not accepted for ${meter.code}`);
    events.push({
      id,
      accountId,
      meter: meter.code,
      quantity: Q.parse(r.quantity).mul(factor).toDecimal(),
      at: new Date(r.at).toISOString(),
    });
  }
  events.sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  const exceptions = [...counts.values()].sort((a, b) =>
    a.code !== b.code ? (a.code < b.code ? -1 : 1) : a.ref < b.ref ? -1 : a.ref > b.ref ? 1 : 0,
  );
  return { events, exceptions };
}
