import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { exportUserData } from "../src/export.ts";

test("unknown user throws", () => {
  const dir = mkdtempSync(join(tmpdir(), "dsar-"));
  for (const f of ["users.jsonl", "orders.jsonl", "tickets.jsonl"]) writeFileSync(join(dir, f), "");
  assert.throws(() => exportUserData(dir, "u-404"), /unknown user u-404/);
});
