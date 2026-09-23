import { test } from "node:test";
import assert from "node:assert/strict";
import { computePayroll } from "../src/payroll.ts";
import { byId, fixture, PERIOD, shiftB } from "./fixture.ts";

const run = (dir: string) => computePayroll(dir, PERIOD[0], PERIOD[1]);

test("rates written with a decimal comma", () => {
  const dir = fixture({
    employees: [
      'E-6,B-66,Test Six,six@example.test,+49 30 000006,BER,FT,"19,5"',
      'E-7,B-77,Test Seven,seven@example.test,+49 30 000007,BER,FT,"21,75"',
    ],
    terminalB: [
      ...shiftB("B-66", "2026-10-20", "+02:00", ["08:00", "12:00"]),
      ...shiftB("B-77", "2026-10-20", "+02:00", ["08:00", "09:00"]),
    ],
  });
  const r = run(dir);
  assert.equal(byId(r.employees, "E-6").grossCents, 7800);
  assert.equal(byId(r.employees, "E-7").grossCents, 2175);
});

test("each category is rounded half up on its own", () => {
  const dir = fixture({
    employees: ["E-8,B-88,Test Eight,eight@example.test,+49 30 000008,BER,FT,10.01"],
    terminalB: [
      ...shiftB("B-88", "2026-10-20", "+02:00", ["07:00", "16:00"], [["11:00", "11:40"]]),
      ...shiftB("B-88", "2026-10-21", "+02:00", ["08:00", "08:30"]),
    ],
  });
  const e8 = byId(run(dir).employees, "E-8");
  assert.equal(e8.minutes.regular, 510);
  assert.equal(e8.minutes.ot1, 20);
  assert.equal(e8.grossCents, 8509 + 501);
});

test("amounts are rounded per category over the period, not per shift", () => {
  const dir = fixture({
    employees: ["E-8,B-88,Test Eight,eight@example.test,+49 30 000008,BER,FT,10.01"],
    terminalB: [
      ...shiftB("B-88", "2026-10-20", "+02:00", ["08:00", "08:30"]),
      ...shiftB("B-88", "2026-10-21", "+02:00", ["08:00", "08:30"]),
    ],
  });
  assert.equal(byId(run(dir).employees, "E-8").grossCents, 1001);
});
