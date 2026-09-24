import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseRecords, parseRows } from "../src/csv.ts";
import { parseEnv } from "../src/env.ts";
import { buildInvoices } from "../src/invoice.ts";
import { Q } from "../src/rational.ts";

test("csv: quoted fields, comments and trimming", () => {
  assert.deepEqual(parseRows('# note\na,"b,c", d \n\n"x ""y""",z\n'), [["a", "b,c", "d"], ['x "y"', "z"]]);
  assert.deepEqual(parseRecords("k, v\n1, 2\n"), [{ k: "1", v: "2" }]);
});

test("env: quotes, export and comments", () => {
  assert.deepEqual(parseEnv("# c\nA=1\nexport B=\"two\"\nC='3'\n"), { A: "1", B: "two", C: "3" });
});

test("rational: exact arithmetic and plain decimals", () => {
  assert.equal(Q.parse("0.1").add(Q.parse("0.2")).toDecimal(), "0.3");
  assert.equal(Q.parse("1234.5000").mul(Q.of(1n, 1024n)).toDecimal(), "1.20556640625");
  assert.equal(Q.of(7n, 2n).floor(), 3n);
});

test("invoices a single full-month customer", () => {
  const dir = mkdtempSync(join(tmpdir(), "invoicing-visible-"));
  mkdirSync(join(dir, "usage"));
  const w = (name: string, text: string) => writeFileSync(join(dir, name), text);
  w(
    "accounts.csv",
    "account_id,parent_id,company,contact_name,contact_email,contact_phone,country,vat_id,currency,timezone,contract_value_eur\n" +
      "ACC-1,,Test Co,Test Person,t@example.test,+49 30 1,DE,,EUR,Etc/UTC,\n",
  );
  w("subscriptions.csv", "account_id,plan,effective_from\nACC-1,starter,2026-01-01 00:00\n");
  w("fx.csv", "date,currency,rate\n");
  w("credits.csv", "credit_id,account_id,kind,granted_on,expires_on,remaining_eur,note\n");
  w(
    "usage/edge-a-test.csv",
    "event_id,account,meter,quantity,unit,ts\n" +
      "E1,ACC-1,api_requests,150000,request,2026-10-05 10:00:00\n" +
      "E2,ACC-1,api_requests,50000,request,2026-10-20 10:00:00\n",
  );
  const run = buildInvoices(dir, "2026-10");
  assert.equal(run.invoices.length, 1);
  const inv = run.invoices[0];
  assert.equal(inv.accountId, "ACC-1");
  assert.deepEqual(
    inv.lines.map((l) => [l.kind, l.meter, l.quantity, l.amount]),
    [
      ["fee", null, "2678400", 4900],
      ["usage", "api_requests", "100000", 1500],
    ],
  );
  assert.equal(inv.taxRule, "domestic");
  assert.equal(inv.tax, 1216);
  assert.equal(inv.total, 7616);
});
