import { test } from "node:test";
import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { renderBankExport } from "../src/bank.ts";
import { ZERO } from "./fixture.ts";

const result = {
  period: { start: "2026-10-19", end: "2026-11-01" },
  employees: [
    { employeeId: "E-2", minutes: ZERO, grossCents: 12345 },
    { employeeId: "E-1", minutes: ZERO, grossCents: 100005 },
    { employeeId: "E-3", minutes: ZERO, grossCents: 0 },
    { employeeId: "E-4", minutes: ZERO, grossCents: 5 },
  ],
  exceptions: [],
};
const env = { PAYROLL_SENDER_ID: "SND-TEST-1", PAYROLL_SIGNING_KEY: "test-key-not-secret" };

test("bank export lists every paid employee after the sender line", () => {
  const lines = renderBankExport(result, env).split("\n");
  assert.deepEqual(lines.slice(0, 4), ["SENDER;SND-TEST-1;2026-10-19;2026-11-01", "E-1;1000.05", "E-2;123.45", "E-4;0.05"]);
  assert.equal(lines.length, 6);
  assert.equal(lines[5], "");
});

test("bank export is signed with HMAC-SHA256 over the preceding lines", () => {
  const text = renderBankExport(result, env);
  const body = "SENDER;SND-TEST-1;2026-10-19;2026-11-01\nE-1;1000.05\nE-2;123.45\nE-4;0.05\n";
  const sig = createHmac("sha256", "test-key-not-secret").update(body).digest("hex");
  assert.equal(text, `${body}SIGNATURE;${sig}\n`);
});

test("bank export refuses to run without the signing key", () => {
  assert.throws(() => renderBankExport(result, { PAYROLL_SENDER_ID: "SND" }), /PAYROLL_SIGNING_KEY/);
  assert.throws(() => renderBankExport(result, { PAYROLL_SENDER_ID: "SND", PAYROLL_SIGNING_KEY: "" }), /PAYROLL_SIGNING_KEY/);
});

test("bank export refuses to run without the sender id", () => {
  assert.throws(() => renderBankExport(result, { PAYROLL_SIGNING_KEY: "k" }), /PAYROLL_SENDER_ID/);
});
