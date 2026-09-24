// The real October run in the workspace: every expectation follows from docs/RULES.md applied to
// the files in data/ (no expectation depends on a name, contact or note).
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { buildInvoices } from "../src/invoice.ts";
import { renderLedgerExport } from "../src/ledger.ts";
import { invoiceOf, rows } from "./fixture.ts";

let cached: ReturnType<typeof buildInvoices> | undefined;
const result = () => (cached ??= buildInvoices(join(process.cwd(), "data"), "2026-10"));
const summary = (id: string) => {
  const i = invoiceOf(result().invoices, id);
  return [i.taxRule, i.taxRatePct, i.subtotal, i.creditTotal, i.tax, i.total];
};

test("real data: exceptions of the run", () => {
  assert.deepEqual(result().exceptions, [
    { code: "UNKNOWN_ACCOUNT", ref: "ACC-0999", count: 7 },
    { code: "UNKNOWN_ACCOUNT", ref: "acc-09999", count: 4 },
    { code: "UNKNOWN_ACCOUNT", ref: "probe-synthetic", count: 3 },
    { code: "UNKNOWN_METER", ref: "gpu_seconds", count: 15 },
  ]);
});

test("real data: invoiced accounts and currencies", () => {
  assert.deepEqual(
    result().invoices.map((i) => `${i.accountId}/${i.currency}`),
    [
      "ACC-1001/EUR", "ACC-1002/EUR", "ACC-1003/EUR", "ACC-1004/EUR", "ACC-1005/EUR", "ACC-1006/EUR", "ACC-1007/EUR",
      "ACC-1008/EUR", "ACC-1009/EUR", "ACC-1010/GBP", "ACC-1011/USD", "ACC-1012/AUD", "ACC-1013/JPY", "ACC-1014/EUR",
      "ACC-1015/EUR", "ACC-1016/EUR", "ACC-1017/USD", "ACC-1018/EUR", "ACC-1019/EUR", "ACC-1020/EUR", "ACC-1021/EUR",
      "ACC-1022/JPY", "ACC-1023/EUR", "ACC-1024/USD", "ACC-1025/EUR", "ACC-1026/EUR", "ACC-1028/AUD", "ACC-1029/GBP",
    ],
  );
});

test("real data: a group invoice with divisions on both collectors", () => {
  const i = invoiceOf(result().invoices, "ACC-1001");
  assert.deepEqual(rows(i.lines), [
    ["fee", "scale", null, "2682000", 149900],
    ["usage", "scale", "api_requests", "18072628", 178581],
    ["usage", "scale", "build_minutes", "4249", 3399],
    ["usage", "scale", "egress_gb", "8238.801089", 57672],
    ["usage", "scale", "storage_gib_h", "172858.635", 6050],
  ]);
  assert.deepEqual(summary("ACC-1001"), ["domestic", "19", 395602, 33000, 68894, 431496]);
});

test("real data: an upgrade after the end of summer time", () => {
  const i = invoiceOf(result().invoices, "ACC-1003");
  assert.deepEqual(rows(i.lines), [
    ["fee", "starter", null, "2163600", 3953],
    ["usage", "starter", "api_requests", "1692642", 22619],
    ["usage", "starter", "build_minutes", "2158", 1726],
    ["usage", "starter", "egress_gb", "771.195746", 6941],
    ["usage", "starter", "storage_gib_h", "51065.995", 1787],
    ["fee", "growth", null, "518400", 5779],
    ["usage", "growth", "api_requests", "205143", 3077],
    ["usage", "growth", "egress_gb", "57.497835", 517],
    ["usage", "growth", "storage_gib_h", "3005.368", 105],
  ]);
  assert.deepEqual(i.credits, [
    { creditId: "CR-2603", amount: 4000 },
    { creditId: "CR-2604", amount: 32772 },
  ]);
  assert.deepEqual(summary("ACC-1003"), ["domestic", "19", 46504, 36772, 1849, 11581]);
});

test("real data: customers billed in other currencies and time zones", () => {
  assert.deepEqual(rows(invoiceOf(result().invoices, "ACC-1013").lines), [
    ["fee", "growth", null, "2678400", 48303],
    ["usage", "growth", "api_requests", "375640", 9103],
    ["usage", "growth", "egress_gb", "989.492864", 14387],
    ["usage", "growth", "storage_gib_h", "36257.709", 2050],
  ]);
  assert.deepEqual(summary("ACC-1013"), ["export", "0", 73843, 12116, 0, 61727]);
  assert.deepEqual(summary("ACC-1011"), ["export", "0", 56136, 6557, 0, 49579]);
  assert.deepEqual(summary("ACC-1012"), ["export", "0", 77178, 4949, 0, 72229]);
});

test("real data: plan changes, a cancellation and a new customer", () => {
  assert.deepEqual(summary("ACC-1021"), ["oss", "21", 39990, 0, 8398, 48388]);
  assert.deepEqual(summary("ACC-1015"), ["reverse_charge", "0", 29441, 0, 0, 29441]);
  assert.deepEqual(summary("ACC-1018"), ["domestic", "19", 34514, 0, 6558, 41072]);
  const i = invoiceOf(result().invoices, "ACC-1024");
  assert.deepEqual(
    rows(i.lines).map((r) => [r[0], r[1], r[2], r[3]]),
    [
      ["fee", "scale", null, "2656800"],
      ["usage", "scale", "api_requests", "10219885"],
      ["usage", "scale", "egress_gb", "6809.722452"],
      ["usage", "scale", "storage_gib_h", "122242.617"],
      ["fee", "growth", null, "21600"],
      ["usage", "growth", "build_minutes", "390"],
      ["usage", "growth", "egress_gb", "19.968786"],
    ],
  );
  assert.deepEqual(summary("ACC-1024"), ["export", "0", 346591, 53552, 0, 293039]);
});

test("real data: totals per currency", () => {
  const totals: Record<string, number> = {};
  for (const i of result().invoices) totals[i.currency] = (totals[i.currency] ?? 0) + i.total;
  assert.deepEqual(totals, { EUR: 1669535, GBP: 233734, USD: 506444, AUD: 617542, JPY: 576342 });
});

test("real data: the ledger export of the run", () => {
  const text = renderLedgerExport(result(), { LEDGER_SENDER_ID: "TEST-SENDER", LEDGER_HMAC_KEY: "test-key" });
  const lines = text.split("\n");
  assert.equal(lines[0], "BATCH;TEST-SENDER;2026-10;28");
  assert.equal(lines[1], "INV;ACC-1001;EUR;3956.02;330.00;688.94;4314.96");
  assert.equal(lines.at(-2), "SIG;02cd99f4bc16c9cced4adee158f1c09d602e9ef1e499bff7593a145704d880de");
  assert.equal(createHash("sha256").update(text).digest("hex"), "6e53caec11abf764d8190f3604f97c05ce27d66821c0947b09dbc67c8069eac9");
});
