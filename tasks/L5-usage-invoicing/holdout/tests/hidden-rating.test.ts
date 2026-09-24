import { test } from "node:test";
import assert from "node:assert/strict";
import { buildInvoices } from "../src/invoice.ts";
import { a, fixture, invoiceOf, only, rows } from "./fixture.ts";

const run = (dir: string) => buildInvoices(dir, "2026-10").invoices;
const usage = (dir: string, account: string) => rows(invoiceOf(run(dir), account).lines).filter((r) => r[0] === "usage");

test("rating: graduated tiers across all three api tiers", () => {
  const dir = fixture({
    subscriptions: only("growth", "ACC-9"),
    edgeA: [a("ACC-9", "api_requests", "7500000", "2026-10-03 10:00:00"), a("ACC-9", "api_requests", "5000000", "2026-10-13 10:00:00")],
  });
  assert.deepEqual(usage(dir, "ACC-9"), [["usage", "growth", "api_requests", "10500000", 118000]]);
});

test("rating: volume pricing prices the whole quantity at one tier", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-9"),
    edgeA: [a("ACC-9", "egress_gb", "1200", "2026-10-03 10:00:00"), a("ACC-9", "egress_gb", "300", "2026-10-04 10:00:00")],
  });
  assert.deepEqual(usage(dir, "ACC-9"), [["usage", "starter", "egress_gb", "1500", 10500]]);
});

test("rating: tier bounds are inclusive", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-9", "ACC-1", "ACC-6"),
    edgeA: [
      a("ACC-9", "egress_gb", "1000", "2026-10-03 10:00:00"),
      a("ACC-1", "egress_gb", "1000.000001", "2026-10-03 10:00:00"),
      a("ACC-6", "egress_gb", "10000", "2026-10-03 10:00:00"),
    ],
  });
  const invoices = run(dir);
  assert.equal(invoiceOf(invoices, "ACC-9").lines[1].amount, 9000);
  assert.equal(invoiceOf(invoices, "ACC-1").lines[1].amount, 7000);
  assert.equal(invoiceOf(invoices, "ACC-6").lines[1].amount, 70000);
});

test("rating: included quantities are deducted before volume pricing", () => {
  const dir = fixture({
    subscriptions: only("growth", "ACC-9"),
    edgeA: [a("ACC-9", "egress_gb", "1050", "2026-10-03 10:00:00")],
  });
  assert.deepEqual(usage(dir, "ACC-9"), [["usage", "growth", "egress_gb", "950", 8550]]);
});

test("rating: tiers and included quantities start again in each segment", () => {
  const dir = fixture({
    subscriptions: ["ACC-9,starter,2026-01-01 00:00", "ACC-9,growth,2026-10-16 00:00"],
    edgeA: [a("ACC-9", "api_requests", "1100000", "2026-10-10 10:00:00"), a("ACC-9", "api_requests", "3000000", "2026-10-20 10:00:00")],
  });
  assert.deepEqual(usage(dir, "ACC-9"), [
    ["usage", "starter", "api_requests", "1051613", 15568],
    ["usage", "growth", "api_requests", "1967742", 25645],
  ]);
});

test("rating: quantities are summed exactly", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-9"),
    edgeA: [
      a("ACC-9", "storage_gib_h", "100000.1", "2026-10-03 10:00:00", "GiB-h"),
      a("ACC-9", "storage_gib_h", "200000.2", "2026-10-04 10:00:00", "GiB-h"),
      a("ACC-9", "egress_gb", "0.1", "2026-10-05 10:00:00", "GB"),
      a("ACC-9", "egress_gb", "0.2", "2026-10-06 10:00:00", "GB"),
    ],
    edgeB: [{ id: "b1", account: "acc-00009", metric: "storage_gib_h", value: 1024.5, unit: "MiB-h", at: "2026-10-07T10:00:00.000Z" }],
  });
  assert.deepEqual(usage(dir, "ACC-9"), [
    ["usage", "starter", "egress_gb", "0.3", 3],
    ["usage", "starter", "storage_gib_h", "300001.30048828125", 10500],
  ]);
});

test("rating: line amounts are rounded once, half to even, after conversion", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-9", "ACC-3", "ACC-6"),
    edgeA: [
      a("ACC-9", "api_requests", "100300", "2026-10-03 10:00:00"),
      a("ACC-3", "build_minutes", "502.5", "2026-10-03 10:00:00"),
      a("ACC-3", "api_requests", "100300", "2026-10-03 10:00:00"),
      a("ACC-6", "api_requests", "100100", "2026-10-03 10:00:00"),
    ],
  });
  const invoices = run(dir);
  assert.deepEqual(rows(invoiceOf(invoices, "ACC-9").lines)[1], ["usage", "starter", "api_requests", "300", 4]);
  assert.deepEqual(
    rows(invoiceOf(invoices, "ACC-3").lines).slice(1),
    [
      ["usage", "starter", "api_requests", "300", 6],
      ["usage", "starter", "build_minutes", "2.5", 2],
    ],
  );
  assert.deepEqual(rows(invoiceOf(invoices, "ACC-6").lines)[1], ["usage", "starter", "api_requests", "100", 2]);
});
