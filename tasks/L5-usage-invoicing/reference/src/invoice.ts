import { loadAccounts, rootOf } from "./accounts.ts";
import { loadCatalog, loadCurrencies, loadTaxTable } from "./config.ts";
import { applyCredits, eligibleCredits, loadCredits } from "./credits.ts";
import { loadFx, rateFor } from "./fx.ts";
import { loadSubscriptions, segmentsOf } from "./plans.ts";
import { Q } from "./rational.ts";
import { priceUsage } from "./rating.ts";
import { taxFor } from "./tax.ts";
import { localToInstant, nextPeriod, periodDays } from "./time.ts";
import type { Invoice, InvoiceLine, InvoiceRun, UsageEvent } from "./types.ts";
import { loadUsage } from "./usage.ts";

/** Invoices of every root account for the month `period` (`YYYY-MM`). */
export function buildInvoices(dataDir: string, period: string): InvoiceRun {
  const accounts = loadAccounts(dataDir);
  const catalog = loadCatalog();
  const currencies = loadCurrencies();
  const taxTable = loadTaxTable();
  const subscriptions = loadSubscriptions(dataDir);
  const fx = loadFx(dataDir);
  const credits = loadCredits(dataDir);
  const usage = loadUsage(dataDir);
  const { first, last } = periodDays(period);

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
    const start = localToInstant(root.timezone, first);
    const end = localToInstant(root.timezone, `${nextPeriod(period)}-01`);
    const periodSeconds = BigInt((end - start) / 1000);
    const billed = segmentsOf(rows, root.timezone, start, end).filter((s) => s.plan !== "none");
    if (billed.length === 0) continue;

    const currency = currencies[root.currency];
    if (!currency) throw new Error(`account ${rootId}: unknown currency ${root.currency}`);
    const rate = rateFor(fx, root.currency, last);
    const convert = (eur: Q) => Number(eur.mul(rate).roundHalfEven(currency.decimals));

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
        const included = Q.of(Q.of(BigInt(plan.included[code] ?? 0)).mul(share).floor());
        const billable = raw.get(code)!.sub(included);
        if (billable.cmp(Q.ZERO) <= 0) continue;
        lines.push({ kind: "usage", plan: plan.code, meter: code, quantity: billable.toDecimal(), amount: convert(priceUsage(meter, billable)) });
      }
    }

    const subtotal = lines.reduce((s, l) => s + BigInt(l.amount), 0n);
    const usageSum = lines.filter((l) => l.kind === "usage").reduce((s, l) => s + BigInt(l.amount), 0n);
    const ordered = eligibleCredits(credits, trees.get(rootId)!, first, last).map((c) => ({
      id: c.id,
      value: c.remainingEur.mul(rate).roundHalfEven(currency.decimals),
    }));
    const applied = applyCredits(ordered, usageSum);
    const creditTotal = applied.reduce((s, c) => s + BigInt(c.amount), 0n);
    const { rule, ratePct } = taxFor(taxTable, root);
    const base = subtotal - creditTotal;
    const tax = Q.of(base).mul(Q.parse(ratePct)).div(Q.of(100n)).roundHalfEven(0);
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
