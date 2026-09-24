import { test } from "node:test";
import assert from "node:assert/strict";
import { buildInvoices } from "../src/invoice.ts";
import { a, fixture, invoiceOf, only } from "./fixture.ts";

// ACC-9 on starter with 1,000,000 billable api requests: fee 49.00 EUR, usage 150.00 EUR.
const base = (credits: string[], extra: Parameters<typeof fixture>[0] = {}) =>
  fixture({
    subscriptions: only("starter", "ACC-9"),
    edgeA: [a("ACC-9", "api_requests", "1100000", "2026-10-12 10:00:00")],
    credits,
    ...extra,
  });
const credits = (dir: string, account = "ACC-9") => invoiceOf(buildInvoices(dir, "2026-10").invoices, account).credits.map((c) => [c.creditId, c.amount]);

test("credits: the earliest expiry is used first", () => {
  const dir = base(["CR-A,ACC-9,prepaid,2026-01-10,,100.00,prepaid", "CR-B,ACC-9,promo,2026-09-20,2026-10-15,80.00,promo"]);
  assert.deepEqual(credits(dir), [
    ["CR-B", 8000],
    ["CR-A", 7000],
  ]);
});

test("credits: ties on expiry go to the earliest grant, then the credit id", () => {
  const dir = base([
    "CR-1,ACC-9,promo,2026-06-01,2026-12-31,60.00,x",
    "CR-2,ACC-9,promo,2026-05-01,2026-12-31,60.00,x",
    "CR-4,ACC-9,promo,2026-07-01,2026-11-30,60.00,x",
    "CR-3,ACC-9,promo,2026-07-01,2026-11-30,60.00,x",
  ]);
  assert.deepEqual(credits(dir), [
    ["CR-3", 6000],
    ["CR-4", 6000],
    ["CR-2", 3000],
  ]);
});

test("credits: only credits valid in the period are eligible", () => {
  const dir = base([
    "CR-X,ACC-9,promo,2026-06-01,2026-09-30,100.00,expired",
    "CR-Y,ACC-9,promo,2026-11-01,,100.00,future",
    "CR-Z,ACC-9,promo,2026-06-01,2026-10-01,10.00,last day is the first day",
    "CR-W,ACC-9,prepaid,2026-10-31,,20.00,granted on the last day",
    "CR-V,ACC-9,promo,2026-06-01,,0.00,used up",
  ]);
  assert.deepEqual(credits(dir), [
    ["CR-Z", 1000],
    ["CR-W", 2000],
  ]);
});

test("credits: apply to usage only and reduce the tax base", () => {
  const dir = base(["CR-L,ACC-9,prepaid,2026-01-01,,500.00,large"]);
  const inv = invoiceOf(buildInvoices(dir, "2026-10").invoices, "ACC-9");
  assert.deepEqual(
    [inv.subtotal, inv.creditTotal, inv.tax, inv.total],
    [19900, 15000, 931, 5831],
  );
  assert.deepEqual(inv.credits, [{ creditId: "CR-L", amount: 15000 }]);
});

test("credits: credits of descendants at any depth apply to the root invoice", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-1"),
    edgeA: [a("ACC-12", "api_requests", "1100000", "2026-10-12 10:00:00")],
    credits: ["CR-D,ACC-13,promo,2026-03-01,,40.00,x", "CR-C,ACC-11,promo,2026-02-01,,10.00,x"],
  });
  assert.deepEqual(credits(dir, "ACC-1"), [
    ["CR-C", 1000],
    ["CR-D", 4000],
  ]);
});

test("credits: values are converted with the invoice rate and rounded half to even", () => {
  const dir = fixture({
    subscriptions: only("starter", "ACC-4", "ACC-3"),
    edgeA: [a("ACC-4", "api_requests", "1100000", "2026-10-12 10:00:00"), a("ACC-3", "api_requests", "1100000", "2026-10-12 10:00:00")],
    credits: ["CR-J,ACC-4,promo,2026-01-01,,12.34,x", "CR-U,ACC-3,promo,2026-01-01,,0.02,x"],
  });
  assert.deepEqual(credits(dir, "ACC-4"), [["CR-J", 1974]]);
  assert.deepEqual(credits(dir, "ACC-3"), [["CR-U", 2]]);
});
