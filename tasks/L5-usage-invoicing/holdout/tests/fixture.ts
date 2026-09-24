// Builds small data directories for the hidden tests. Every expectation in the hidden tests
// follows from docs/RULES.md applied to these files (or, in hidden-realdata, to data/).
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Invoice, InvoiceLine } from "../src/types.ts";

const HEADER = "account_id,parent_id,company,contact_name,contact_email,contact_phone,country,vat_id,currency,timezone,contract_value_eur";

/** account_id,parent_id,country,vat_id,currency,timezone */
const ACCOUNTS: [string, string, string, string, string, string][] = [
  ["ACC-1", "", "DE", "", "EUR", "Europe/Berlin"],
  ["ACC-2", "", "FR", "FR 11 222 333", "EUR", "Europe/Paris"],
  ["ACC-3", "", "US", "", "USD", "America/New_York"],
  ["ACC-4", "", "JP", "", "JPY", "Asia/Tokyo"],
  ["ACC-5", "", "AU", "", "AUD", "Australia/Sydney"],
  ["ACC-6", "", "IE", "", "EUR", "Europe/Dublin"],
  ["ACC-7", "", "DE", "DE 44 555 666", "EUR", "Europe/Berlin"],
  ["ACC-8", "", "GB", "GB 77 888 999", "GBP", "Europe/London"],
  ["ACC-9", "", "DE", "", "EUR", "Etc/UTC"],
  ["ACC-11", "ACC-1", "US", "US 1", "USD", "Asia/Tokyo"],
  ["ACC-12", "ACC-11", "JP", "", "JPY", "America/Los_Angeles"],
  ["ACC-13", "ACC-12", "FR", "", "EUR", "Europe/Paris"],
];

const FX = [
  "date,currency,rate",
  "2026-10-29,USD,1.10",
  "2026-10-29,JPY,150",
  "2026-10-30,USD,1.25",
  "2026-10-30,JPY,160",
  "2026-10-30,AUD,1.6",
  "2026-10-30,GBP,0.8",
  "2026-11-02,USD,1.40",
  "2026-11-02,JPY,170",
  "2026-11-02,AUD,1.7",
  "2026-11-02,GBP,0.9",
];

export interface BRecord {
  id: string;
  account: string;
  metric: string;
  value: number | string;
  unit?: string;
  at: string;
}

export interface FixtureInput {
  /** Subscription rows `account,plan,effective_from`; default: every root on `growth` since 2026-01-01. */
  subscriptions?: string[];
  /** edge-a rows `event_id,account,meter,quantity,unit,ts` (ts in UTC, `YYYY-MM-DD HH:MM:SS`). */
  edgeA?: string[];
  edgeB?: BRecord[];
  /** Credit rows `credit_id,account_id,kind,granted_on,expires_on,remaining_eur,note`. */
  credits?: string[];
  /** Extra fx rows `date,currency,rate`. */
  fx?: string[];
  /** Replaces the default fx rows. */
  fxOnly?: string[];
}

export function fixture(input: FixtureInput = {}): string {
  const dir = mkdtempSync(join(tmpdir(), "invoicing-hidden-"));
  mkdirSync(join(dir, "usage"));
  const w = (name: string, lines: string[]) => writeFileSync(join(dir, name), lines.join("\n") + "\n");
  w("accounts.csv", [
    HEADER,
    ...ACCOUNTS.map(([id, parent, country, vat, cur, tz]) => `${id},${parent},Co ${id},Person ${id},${id.toLowerCase()}@example.test,+49 30 1,${country},${vat},${cur},${tz},`),
  ]);
  const roots = ACCOUNTS.filter((a) => a[1] === "").map((a) => a[0]);
  w("subscriptions.csv", ["account_id,plan,effective_from", ...(input.subscriptions ?? roots.map((r) => `${r},growth,2026-01-01 00:00`))]);
  w("fx.csv", input.fxOnly ? ["date,currency,rate", ...input.fxOnly] : [...FX, ...(input.fx ?? [])]);
  w("credits.csv", ["credit_id,account_id,kind,granted_on,expires_on,remaining_eur,note", ...(input.credits ?? [])]);
  w("usage/edge-a-test.csv", ["# edge-a test export; ts is UTC", "event_id,account,meter,quantity,unit,ts", ...(input.edgeA ?? [])]);
  writeFileSync(join(dir, "usage/edge-b-test.jsonl"), (input.edgeB ?? []).map((r) => JSON.stringify(r) + "\n").join(""));
  return dir;
}

let n = 0;
/** One edge-a row with a fresh id. */
export function a(account: string, meter: string, quantity: string, ts: string, unit = ""): string {
  n++;
  return `A${String(n).padStart(8, "0")},${account},${meter},${quantity},${unit},${ts}`;
}

export function invoiceOf(invoices: Invoice[], accountId: string): Invoice {
  const inv = invoices.find((i) => i.accountId === accountId);
  if (!inv) throw new Error(`no invoice for ${accountId}`);
  return inv;
}

/** Lines as `[kind, plan, meter, quantity, amount]` tuples. */
export function rows(lines: InvoiceLine[]): [string, string, string | null, string, number][] {
  return lines.map((l) => [l.kind, l.plan, l.meter, l.quantity, l.amount]);
}

/** Only the given roots subscribed (to `plan` since 2026-01-01). */
export function only(plan: string, ...accounts: string[]): string[] {
  return accounts.map((acc) => `${acc},${plan},2026-01-01 00:00`);
}
