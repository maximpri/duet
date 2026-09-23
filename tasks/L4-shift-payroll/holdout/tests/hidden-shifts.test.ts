import { test } from "node:test";
import assert from "node:assert/strict";
import { computePayroll } from "../src/payroll.ts";
import { b, byId, fixture, PERIOD, shiftB, ZERO } from "./fixture.ts";

const run = (dir: string) => computePayroll(dir, PERIOD[0], PERIOD[1]);

test("pays a plain shift at the base rate", () => {
  const dir = fixture({ terminalB: shiftB("B-11", "2026-10-20", "+02:00", ["08:00", "12:00"]) });
  const e1 = byId(run(dir).employees, "E-1");
  assert.deepEqual(e1.minutes, { ...ZERO, regular: 240 });
  assert.equal(e1.grossCents, 8000);
});

test("every employee is listed, sorted, with zeros when nothing is paid", () => {
  const dir = fixture({
    employees: ["E-0,B-10,Test Zero,zero@example.test,+49 30 000000,BER,FT,10.00"],
    terminalB: shiftB("B-22", "2026-10-20", "+02:00", ["08:00", "09:00"]),
  });
  const r = run(dir);
  assert.deepEqual(r.period, { start: PERIOD[0], end: PERIOD[1] });
  assert.deepEqual(r.employees.map((e) => e.employeeId), ["E-0", "E-1", "E-2", "E-3", "E-4", "E-5"]);
  assert.deepEqual(byId(r.employees, "E-0"), { employeeId: "E-0", minutes: ZERO, grossCents: 0 });
  assert.equal(byId(r.employees, "E-2").minutes.regular, 60);
});

test("rounds IN and OUT to the nearest quarter hour", () => {
  const dir = fixture({
    terminalB: [
      ...shiftB("B-11", "2026-10-20", "+02:00", ["07:07", "11:08"]),
      ...shiftB("B-11", "2026-10-21", "+02:00", ["07:08", "11:07"]),
    ],
  });
  assert.equal(byId(run(dir).employees, "E-1").minutes.regular, 255 + 225);
});

test("recorded breaks are unpaid", () => {
  const dir = fixture({ terminalB: shiftB("B-11", "2026-10-20", "+02:00", ["08:00", "13:00"], [["10:02", "10:19"]]) });
  assert.equal(byId(run(dir).employees, "E-1").minutes.regular, 300 - 17);
});

test("long shifts are topped up to a 30-minute break", () => {
  const dir = fixture({
    terminalB: [
      ...shiftB("B-11", "2026-10-20", "+02:00", ["07:00", "14:00"]),
      ...shiftB("B-11", "2026-10-21", "+02:00", ["07:00", "14:10"], [["10:00", "10:10"]]),
      ...shiftB("B-11", "2026-10-22", "+02:00", ["07:00", "13:00"]),
    ],
  });
  assert.equal(byId(run(dir).employees, "E-1").minutes.regular, 390 + 405 + 360);
});

test("the break top-up is taken from the end of the shift", () => {
  const dir = fixture({ terminalB: shiftB("B-11", "2026-10-20", "+02:00", ["15:00", "23:00"]) });
  assert.deepEqual(byId(run(dir).employees, "E-1").minutes, { ...ZERO, regular: 450, night: 30 });
});

test("an IN without an OUT is unpaid and reported as MISSING_OUT", () => {
  const dir = fixture({
    terminalB: [
      b("B-11", "2026-10-20T07:00+02:00", "clock_in"),
      ...shiftB("B-11", "2026-10-21", "+02:00", ["07:00", "11:00"]),
      b("B-11", "2026-10-22T07:00+02:00", "clock_in"),
    ],
  });
  const r = run(dir);
  assert.equal(byId(r.employees, "E-1").minutes.regular, 240);
  assert.deepEqual(r.exceptions, [
    { code: "MISSING_OUT", ref: "E-1", at: "2026-10-20T05:00:00Z" },
    { code: "MISSING_OUT", ref: "E-1", at: "2026-10-22T05:00:00Z" },
  ]);
});

test("orphan OUT and break punches are reported; an open break ends at OUT", () => {
  const dir = fixture({
    terminalB: [
      b("B-11", "2026-10-23T12:00+02:00", "clock_out"),
      b("B-11", "2026-10-26T08:00+01:00", "clock_in"),
      b("B-11", "2026-10-26T09:00+01:00", "break_end"),
      b("B-11", "2026-10-26T10:00+01:00", "break_start"),
      b("B-11", "2026-10-26T10:30+01:00", "break_start"),
      b("B-11", "2026-10-26T10:40+01:00", "break_end"),
      b("B-11", "2026-10-26T12:00+01:00", "clock_out"),
      b("B-11", "2026-10-27T08:00+01:00", "clock_in"),
      b("B-11", "2026-10-27T11:00+01:00", "break_start"),
      b("B-11", "2026-10-27T12:00+01:00", "clock_out"),
    ],
  });
  const r = run(dir);
  assert.equal(byId(r.employees, "E-1").minutes.regular, 200 + 180);
  assert.deepEqual(r.exceptions, [
    { code: "ORPHAN_OUT", ref: "E-1", at: "2026-10-23T10:00:00Z" },
    { code: "ORPHAN_BREAK", ref: "E-1", at: "2026-10-26T08:00:00Z" },
    { code: "ORPHAN_BREAK", ref: "E-1", at: "2026-10-26T09:30:00Z" },
  ]);
});

test("a shift belongs to the date of its IN and is paid in full", () => {
  const dir = fixture({
    terminalB: [
      b("B-11", "2026-10-17T07:00+02:00", "clock_in"),
      b("B-11", "2026-10-18T22:00+02:00", "clock_in"),
      b("B-11", "2026-10-19T06:00+02:00", "clock_out"),
      b("B-11", "2026-11-01T20:00+01:00", "clock_in"),
      b("B-11", "2026-11-02T04:00+01:00", "clock_out"),
    ],
  });
  const r = run(dir);
  assert.deepEqual(byId(r.employees, "E-1").minutes, { ...ZERO, regular: 450, night: 330 });
  assert.deepEqual(r.exceptions, []);
});
