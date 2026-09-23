import { test } from "node:test";
import assert from "node:assert/strict";
import { computePayroll } from "../src/payroll.ts";
import { byId, fixture, PERIOD, shiftB, ZERO } from "./fixture.ts";

const run = (dir: string) => computePayroll(dir, PERIOD[0], PERIOD[1]);

test("night minutes earn the contract's night premium", () => {
  const dir = fixture({ terminalB: shiftB("B-55", "2026-10-20", "+02:00", ["20:00", "23:30"]) });
  const e5 = byId(run(dir).employees, "E-5");
  assert.deepEqual(e5.minutes, { ...ZERO, regular: 210, night: 90 });
  assert.equal(e5.grossCents, 10500 + 1800);
});

test("the night the clocks go back is an hour longer", () => {
  const dir = fixture({ terminalA: ["0055;24.10.2026 22:00;K;T1", "0055;25.10.2026 06:00;G;T1"] });
  const e5 = byId(run(dir).employees, "E-5");
  assert.deepEqual(e5.minutes, { ...ZERO, regular: 510, night: 510 });
  assert.equal(e5.grossCents, 25500 + 10200);
});

test("holidays of the site's region are paid double", () => {
  const dir = fixture({
    terminalB: [
      ...shiftB("B-22", "2026-10-31", "+01:00", ["08:00", "12:00"]),
      ...shiftB("B-11", "2026-10-31", "+01:00", ["08:00", "12:00"]),
    ],
  });
  const r = run(dir);
  assert.deepEqual(byId(r.employees, "E-2").minutes, { ...ZERO, holiday: 240 });
  assert.equal(byId(r.employees, "E-2").grossCents, 14400);
  assert.deepEqual(byId(r.employees, "E-1").minutes, { ...ZERO, regular: 240 });
});

test("holidays listed for every region apply to all sites", () => {
  const dir = fixture({
    holidays: ["2026-10-21,*,Company test holiday"],
    terminalB: shiftB("B-33", "2026-10-21", "+02:00", ["09:00", "13:00"]),
  });
  assert.deepEqual(byId(run(dir).employees, "E-3").minutes, { ...ZERO, holiday: 240 });
});

test("holiday minutes do not count toward weekly overtime", () => {
  const days = ["2026-10-26", "2026-10-27", "2026-10-28", "2026-10-29", "2026-10-31", "2026-11-01"];
  const dir = fixture({
    terminalB: days.flatMap((d) => shiftB("B-22", d, "+01:00", ["07:00", "15:30"], [["11:00", "11:30"]])),
  });
  assert.deepEqual(byId(run(dir).employees, "E-2").minutes, { ...ZERO, regular: 2400, holiday: 480 });
});

test("the night premium also applies to holiday minutes", () => {
  const dir = fixture({ terminalB: shiftB("B-55", "2026-10-31", "+01:00", ["22:00", "23:00"]) });
  const e5 = byId(run(dir).employees, "E-5");
  assert.deepEqual(e5.minutes, { ...ZERO, holiday: 60, night: 60 });
  assert.equal(e5.grossCents, 6000 + 1200);
});
