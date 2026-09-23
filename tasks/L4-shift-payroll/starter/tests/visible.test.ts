import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseRecords, parseRows } from "../src/csv.ts";
import { parseEnv } from "../src/env.ts";
import { computePayroll } from "../src/payroll.ts";

test("csv: quoted fields and custom delimiters", () => {
  assert.deepEqual(parseRows('a,"b,c",d\n\n"x ""y""",z\n'), [["a", "b,c", "d"], ['x "y"', "z"]]);
  assert.deepEqual(parseRows("# note\nA;B\n1;2\n", { delimiter: ";", comment: "#" }), [["A", "B"], ["1", "2"]]);
  assert.deepEqual(parseRecords("k, v\n1, 2\n"), [{ k: "1", v: "2" }]);
});

test("env: quotes, export and comments", () => {
  assert.deepEqual(parseEnv("# c\nA=1\nexport B=\"two\"\nC='3'\n"), { A: "1", B: "two", C: "3" });
});

test("pays a short day shift from the current terminals", () => {
  const dir = mkdtempSync(join(tmpdir(), "payroll-visible-"));
  mkdirSync(join(dir, "punches"));
  const w = (name: string, text: string) => writeFileSync(join(dir, name), text);
  w("employees.csv", "employee_id,badge,name,email,phone,site,contract,hourly_rate_eur\nE-1,B-11,A,a@example.test,1,BER,FT,20.00\n");
  w("contracts.csv", "code,daily_ot1_after_min,daily_ot2_after_min,weekly_ot_after_min,night_premium_pct,description\nFT,480,720,2400,25,x\n");
  w("sites.csv", "site,region,city\nBER,BE,Berlin\n");
  w("holidays.csv", "date,region,name\n");
  w("corrections.csv", "badge,local_time,kind,action,status,approved_by,comment\n");
  w(
    "punches/terminal-b-test.jsonl",
    [
      { badge: "B-11", ts: "2026-10-20T08:00:00+02:00", event: "clock_in" },
      { badge: "B-11", ts: "2026-10-20T12:00:00+02:00", event: "clock_out" },
    ]
      .map((r) => JSON.stringify(r) + "\n")
      .join(""),
  );
  const r = computePayroll(dir, "2026-10-19", "2026-11-01");
  assert.equal(r.employees.length, 1);
  assert.equal(r.employees[0].minutes.regular, 240);
  assert.equal(r.employees[0].grossCents, 8000);
});
