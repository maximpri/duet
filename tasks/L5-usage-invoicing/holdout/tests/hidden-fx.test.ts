import { test } from "node:test";
import assert from "node:assert/strict";
import { buildInvoices } from "../src/invoice.ts";
import { a, fixture, invoiceOf, only } from "./fixture.ts";

const run = (dir: string) => buildInvoices(dir, "2026-10").invoices;

test("fx: the latest rate on or before the last day of the period", () => {
  const dir = fixture({ subscriptions: only("growth", "ACC-3", "ACC-5") });
  assert.equal(invoiceOf(run(dir), "ACC-3").total, 37375);
  assert.equal(invoiceOf(run(dir), "ACC-5").total, 47840);
});

test("fx: a rate published on the last day itself is used", () => {
  const dir = fixture({ subscriptions: only("growth", "ACC-3"), fx: ["2026-10-31,USD,1.30"] });
  assert.equal(invoiceOf(run(dir), "ACC-3").total, 38870);
});

test("fx: yen invoices have no minor unit", () => {
  const dir = fixture({
    subscriptions: only("growth", "ACC-4"),
    edgeA: [a("ACC-4", "api_requests", "2000123", "2026-10-03 10:00:00")],
  });
  const inv = invoiceOf(run(dir), "ACC-4");
  assert.deepEqual(inv.lines.map((l) => l.amount), [47840, 3]);
  assert.deepEqual([inv.subtotal, inv.tax, inv.total], [47843, 0, 47843]);
});

test("fx: a missing rate is an error, not a later rate", () => {
  const dir = fixture({ subscriptions: only("growth", "ACC-8"), fxOnly: ["2026-11-02,GBP,0.9"] });
  assert.throws(() => run(dir));
});
