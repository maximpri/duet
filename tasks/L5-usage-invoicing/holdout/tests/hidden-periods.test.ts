import { test } from "node:test";
import assert from "node:assert/strict";
import { buildInvoices } from "../src/invoice.ts";
import { a, fixture, invoiceOf, only, rows } from "./fixture.ts";

const run = (dir: string) => buildInvoices(dir, "2026-10").invoices;

test("periods: a full month is measured in real seconds of the local month", () => {
  const invoices = run(fixture({ subscriptions: only("growth", "ACC-1", "ACC-3", "ACC-5") }));
  assert.deepEqual(rows(invoiceOf(invoices, "ACC-1").lines), [["fee", "growth", null, "2682000", 29900]]);
  assert.equal(invoiceOf(invoices, "ACC-3").lines[0].quantity, "2678400");
  assert.equal(invoiceOf(invoices, "ACC-5").lines[0].quantity, "2674800");
});

test("periods: usage belongs to the month in the root's time zone", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-3", "ACC-4"),
    edgeA: [
      a("ACC-3", "egress_gb", "10", "2026-11-01 02:30:00"),
      a("ACC-3", "egress_gb", "20", "2026-10-01 03:10:00"),
      a("ACC-3", "egress_gb", "1", "2026-10-15 12:00:00"),
      a("ACC-4", "egress_gb", "40", "2026-10-31 16:20:00"),
      a("ACC-4", "egress_gb", "5", "2026-09-30 15:45:00"),
    ],
  });
  const invoices = run(dir);
  assert.equal(invoiceOf(invoices, "ACC-3").lines[1].quantity, "11");
  assert.equal(invoiceOf(invoices, "ACC-4").lines[1].quantity, "5");
});

test("periods: descendants are billed in the root's month, not their own zone", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-1"),
    edgeA: [
      a("ACC-11", "egress_gb", "3", "2026-10-31 16:00:00"),
      a("ACC-11", "egress_gb", "4", "2026-09-30 21:30:00"),
      a("ACC-11", "egress_gb", "2", "2026-09-30 22:30:00"),
      a("ACC-12", "egress_gb", "7", "2026-11-01 05:00:00"),
    ],
  });
  const lines = invoiceOf(run(dir), "ACC-1").lines;
  assert.deepEqual(
    lines.filter((l) => l.kind === "usage").map((l) => [l.meter, l.quantity]),
    [["egress_gb", "5"]],
  );
});

test("periods: a plan change after the end of summer time", () => {
  const dir = fixture({ subscriptions: ["ACC-1,starter,2026-01-01 00:00", "ACC-1,growth,2026-10-26 00:00"] });
  assert.deepEqual(rows(invoiceOf(run(dir), "ACC-1").lines), [
    ["fee", "starter", null, "2163600", 3953],
    ["fee", "growth", null, "518400", 5779],
  ]);
});

test("periods: a row that repeats the plan in force does not split the month", () => {
  const dir = fixture({
    subscriptions: ["ACC-1,growth,2026-01-01 00:00", "ACC-1,growth,2026-10-12 00:00"],
    edgeA: [a("ACC-1", "api_requests", "1050000", "2026-10-05 10:00:00"), a("ACC-1", "api_requests", "1050000", "2026-10-20 10:00:00")],
  });
  assert.deepEqual(rows(invoiceOf(run(dir), "ACC-1").lines), [
    ["fee", "growth", null, "2682000", 29900],
    ["usage", "growth", "api_requests", "100000", 1500],
  ]);
});

test("periods: a cancelled plan is billed up to the cancellation only", () => {
  const dir = fixture({
    subscriptions: ["ACC-9,growth,2026-01-01 00:00", "ACC-9,none,2026-10-20 00:00"],
    edgeA: [a("ACC-9", "api_requests", "3000000", "2026-10-10 10:00:00"), a("ACC-9", "api_requests", "5000000", "2026-10-25 10:00:00")],
  });
  assert.deepEqual(rows(invoiceOf(run(dir), "ACC-9").lines), [
    ["fee", "growth", null, "1641600", 18326],
    ["usage", "growth", "api_requests", "1774194", 23516],
  ]);
});

test("periods: a plan starting mid-month bills from its start", () => {
  const dir = fixture({
    subscriptions: ["ACC-9,growth,2026-10-08 09:30"],
    edgeA: [a("ACC-9", "build_minutes", "4000", "2026-10-05 10:00:00"), a("ACC-9", "build_minutes", "2500", "2026-10-08 09:30:00")],
  });
  assert.deepEqual(rows(invoiceOf(run(dir), "ACC-9").lines), [
    ["fee", "growth", null, "2039400", 22767],
    ["usage", "growth", "build_minutes", "216", 173],
  ]);
});

test("periods: plan changes take effect at local time in the root's zone", () => {
  const dir = fixture({
    subscriptions: ["ACC-5,starter,2026-01-01 00:00", "ACC-5,growth,2026-10-10 00:00"],
    edgeA: [a("ACC-5", "build_minutes", "600", "2026-10-09 12:30:00")],
  });
  assert.deepEqual(rows(invoiceOf(run(dir), "ACC-5").lines), [
    ["fee", "starter", null, "774000", 2269],
    ["usage", "starter", "build_minutes", "456", 584],
    ["fee", "growth", null, "1900800", 33997],
  ]);
});

test("periods: only roots with a billed segment in the month are invoiced", () => {
  const dir = fixture({
    subscriptions: ["ACC-9,none,2026-01-01 00:00", "ACC-1,growth,2026-11-01 00:00", "ACC-6,starter,2026-10-31 23:00"],
    edgeA: [a("ACC-9", "api_requests", "900000", "2026-10-10 10:00:00")],
  });
  const invoices = run(dir);
  assert.deepEqual(
    invoices.map((i) => i.accountId),
    ["ACC-6"],
  );
  assert.deepEqual(rows(invoices[0].lines), [["fee", "starter", null, "3600", 7]]);
});
