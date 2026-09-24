import { test } from "node:test";
import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { renderLedgerExport } from "../src/ledger.ts";
import type { Invoice, InvoiceRun } from "../src/types.ts";

const invoice = (accountId: string, currency: string, subtotal: number, creditTotal: number, tax: number): Invoice => ({
  accountId,
  period: "2026-10",
  currency,
  lines: [],
  credits: [],
  taxRule: tax ? "domestic" : "export",
  taxRatePct: tax ? "19" : "0",
  subtotal,
  creditTotal,
  tax,
  total: subtotal - creditTotal + tax,
});

const RUN: InvoiceRun = {
  period: "2026-10",
  invoices: [invoice("ACC-4", "JPY", 47843, 1974, 0), invoice("ACC-9", "EUR", 19900, 15000, 931), invoice("ACC-99", "USD", 5, 5, 0)],
  exceptions: [],
};
const ENV = { LEDGER_SENDER_ID: "SENDER-7", LEDGER_HMAC_KEY: "k3y-for-tests" };
const BODY =
  "BATCH;SENDER-7;2026-10;3\n" +
  "INV;ACC-4;JPY;47843;1974;0;45869\n" +
  "INV;ACC-9;EUR;199.00;150.00;9.31;58.31\n" +
  "INV;ACC-99;USD;0.05;0.05;0.00;0.00\n";

test("ledger: batch header and one line per invoice in major units", () => {
  const text = renderLedgerExport(RUN, ENV);
  assert.ok(text.startsWith(BODY), text);
});

test("ledger: signed with HMAC-SHA256 over the preceding lines", () => {
  const sig = createHmac("sha256", ENV.LEDGER_HMAC_KEY).update(BODY).digest("hex");
  assert.equal(renderLedgerExport(RUN, ENV), `${BODY}SIG;${sig}\n`);
});

test("ledger: a missing sender id or signing key is an error naming the variable", () => {
  assert.throws(() => renderLedgerExport(RUN, { LEDGER_HMAC_KEY: "k" }), /LEDGER_SENDER_ID/);
  assert.throws(() => renderLedgerExport(RUN, { LEDGER_SENDER_ID: "S", LEDGER_HMAC_KEY: "" }), /LEDGER_HMAC_KEY/);
});
