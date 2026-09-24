import { loadAccounts, rootOf } from "./accounts.ts";
import { loadCatalog, loadTaxTable } from "./config.ts";
import { applyCredits, eligibleCredits, loadCredits } from "./credits.ts";
import { loadFx, rateFor } from "./fx.ts";
import { loadSubscriptions, segmentsOf } from "./plans.ts";
import { Q } from "./rational.ts";
import { priceUsage } from "./rating.ts";
import { taxFor } from "./tax.ts";
import { nextPeriod, periodDays } from "./time.ts";
import type { Invoice, InvoiceLine, InvoiceRun, UsageEvent } from "./types.ts";
import { loadUsage } from "./usage.ts";

/** Invoice amounts are kept in cents. */
const DECIMALS = 2;

/** Invoices of every root account for the month `period` (`YYYY-MM`). */
export function buildInvoices(dataDir: string, period: string): InvoiceRun {
  const accounts = loadAccounts(dataDir);
  const catalog = loadCatalog();
  const taxTable = loadTaxTable();
  const subscriptions = loadSubscriptions(dataDir);
  const fx = loadFx(dataDir);
  const credits = loadCredits(dataDir);
  const usage = loadUsage(dataDir);
  const { first, last } = periodDays(period);
  const start = Date.parse(`${first}T00:00:00Z`);
  const end = Date.parse(`${nextPeriod(period)}-01T00:00:00Z`);
  const periodSeconds = BigInt(Number(last.slice(8)) * 86400);

  const trees = new Map<string, Set<string>>();
  for (const a of accounts.values()) {
    const root = rootOf(accounts, a.id).id;
    const tree = trees.get(root) ?? new Set<string>();
    tree.add(a.id);
    trees.set(root, tree);
  }
  const eventsByRoot = new Map<string, UsageEvent[]>();
  for (const e of usage.events) {
    const root = rootOf(accounts, e.accountId).id;
    const list = eventsByRoot.get(root) ?? [];
    list.push(e);
    eventsByRoot.set(root, list);
  }

  const invoices: Invoice[] = [];
  const roots = [...trees.keys()].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  for (const rootId of roots) {
    const root = accounts.get(rootId)!;
    const rows = subscriptions.get(rootId);
    if (!rows) continue;
    const billed = segmentsOf(rows, root.timezone, start, end).filter((s) => s.plan !== "none");
    if (billed.length === 0) continue;

    const rate = rateFor(fx, root.currency, last);
    const convert = (eur: Q) => Number(eur.mul(rate).round(DECIMALS));

    const lines: InvoiceLine[] = [];
    const events = eventsByRoot.get(rootId) ?? [];
    for (const seg of billed) {
      const plan = catalog.plans.get(seg.plan);
      if (!plan) throw new Error(`account ${rootId}: unknown plan ${seg.plan}`);
      const seconds = BigInt((seg.end - seg.start) / 1000);
      const share = Q.of(seconds, periodSeconds);
      lines.push({ kind: "fee", plan: plan.code, meter: null, quantity: seconds.toString(), amount: convert(plan.monthlyFee.mul(share)) });

      const raw = new Map<string, Q>();
      for (const e of events) {
        const t = Date.parse(e.at);
        if (t < seg.start || t >= seg.end) continue;
        raw.set(e.meter, (raw.get(e.meter) ?? Q.ZERO).add(Q.parse(e.quantity)));
      }
      for (const code of [...raw.keys()].sort()) {
        const meter = catalog.meters.get(code)!;
        const billable = raw.get(code)!.sub(Q.of(BigInt(plan.included[code] ?? 0)));
        if (billable.cmp(Q.ZERO) <= 0) continue;
        lines.push({ kind: "usage", plan: plan.code, meter: code, quantity: billable.toDecimal(), amount: convert(priceUsage(meter, billable)) });
      }
    }

    const subtotal = lines.reduce((s, l) => s + BigInt(l.amount), 0n);
    const ordered = eligibleCredits(credits, trees.get(rootId)!, first, last).map((c) => ({
      id: c.id,
      value: c.remainingEur.mul(rate).round(DECIMALS),
    }));
    const applied = applyCredits(ordered, subtotal);
    const creditTotal = applied.reduce((s, c) => s + BigInt(c.amount), 0n);
    const { rule, ratePct } = taxFor(taxTable, root);
    const pct = Q.parse(ratePct).div(Q.of(100n));
    let tax = 0n;
    for (const l of lines) tax += Q.of(BigInt(l.amount)).mul(pct).round(0);
    tax -= Q.of(creditTotal).mul(pct).round(0);
    const base = subtotal - creditTotal;
    invoices.push({
      accountId: rootId,
      period,
      currency: root.currency,
      lines,
      credits: applied,
      taxRule: rule,
      taxRatePct: ratePct,
      subtotal: Number(subtotal),
      creditTotal: Number(creditTotal),
      tax: Number(tax),
      total: Number(base + tax),
    });
  }
  return { period, invoices, exceptions: usage.exceptions };
}
