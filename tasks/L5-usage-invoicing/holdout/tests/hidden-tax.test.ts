import { test } from "node:test";
import assert from "node:assert/strict";
import { buildInvoices } from "../src/invoice.ts";
import { a, fixture, invoiceOf, only } from "./fixture.ts";

const run = (dir: string) => buildInvoices(dir, "2026-10").invoices;
const tax = (dir: string, account: string) => {
  const inv = invoiceOf(run(dir), account);
  return [inv.taxRule, inv.taxRatePct, inv.tax, inv.total];
};

test("tax: a German customer is domestic even with a VAT id", () => {
  const dir = fixture({ subscriptions: only("growth", "ACC-7") });
  assert.deepEqual(tax(dir, "ACC-7"), ["domestic", "19", 5681, 35581]);
});

test("tax: EU customers: reverse charge with a VAT id, destination rate without", () => {
  const dir = fixture({ subscriptions: only("growth", "ACC-2", "ACC-6") });
  assert.deepEqual(tax(dir, "ACC-2"), ["reverse_charge", "0", 0, 29900]);
  assert.deepEqual(tax(dir, "ACC-6"), ["oss", "23", 6877, 36777]);
});

test("tax: customers outside the EU are export, VAT id or not", () => {
  const dir = fixture({ subscriptions: only("growth", "ACC-8", "ACC-3") });
  assert.deepEqual(tax(dir, "ACC-8"), ["export", "0", 0, 23920]);
  assert.deepEqual(tax(dir, "ACC-3"), ["export", "0", 0, 37375]);
});

test("tax: rounded half to even once per invoice", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-9"),
    edgeA: [a("ACC-9", "build_minutes", "562.5", "2026-10-03 10:00:00")],
  });
  assert.deepEqual(tax(dir, "ACC-9"), ["domestic", "19", 940, 5890]);
});

test("tax: computed on the invoice sum, not line by line", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-9"),
    edgeA: [
      a("ACC-9", "api_requests", "100133", "2026-10-03 10:00:00"),
      a("ACC-9", "egress_gb", "0.2", "2026-10-03 10:00:00"),
      a("ACC-9", "build_minutes", "502.5", "2026-10-03 10:00:00"),
    ],
  });
  const inv = invoiceOf(run(dir), "ACC-9");
  assert.deepEqual(inv.lines.map((l) => l.amount), [4900, 2, 2, 2]);
  assert.deepEqual([inv.subtotal, inv.tax, inv.total], [4906, 932, 5838]);
});
