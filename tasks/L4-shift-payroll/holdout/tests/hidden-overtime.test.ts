import { test } from "node:test";
import assert from "node:assert/strict";
import { computePayroll } from "../src/payroll.ts";
import { byId, fixture, PERIOD, shiftB, ZERO } from "./fixture.ts";

const run = (dir: string) => computePayroll(dir, PERIOD[0], PERIOD[1]);

/** Weekdays of the first and second week of the period. */
const WEEK1 = ["2026-10-19", "2026-10-20", "2026-10-21", "2026-10-22", "2026-10-23"];
const WEEK2 = ["2026-10-26", "2026-10-27", "2026-10-28", "2026-10-29", "2026-10-30"];

test("full-time daily overtime has two tiers", () => {
  const dir = fixture({ terminalB: shiftB("B-11", "2026-10-20", "+02:00", ["06:00", "20:00"], [["12:00", "12:30"]]) });
  const e1 = byId(run(dir).employees, "E-1");
  assert.deepEqual(e1.minutes, { ...ZERO, regular: 480, ot1: 240, ot2: 90 });
  assert.equal(e1.grossCents, 16000 + 12000 + 6000);
});

test("part-time contracts have no daily overtime", () => {
  const dir = fixture({ terminalB: shiftB("B-33", "2026-10-20", "+02:00", ["07:00", "17:30"], [["12:00", "12:30"]]) });
  assert.deepEqual(byId(run(dir).employees, "E-3").minutes, { ...ZERO, regular: 600 });
});

test("weekly overtime starts at the contract's weekly threshold", () => {
  const dir = fixture({ terminalB: WEEK2.flatMap((d) => shiftB("B-33", d, "+01:00", ["09:00", "14:00"])) });
  assert.deepEqual(byId(run(dir).employees, "E-3").minutes, { ...ZERO, regular: 1200, ot1: 300 });
});

test("the weekly count restarts on Monday", () => {
  const days: [string, string][] = [
    ["2026-10-22", "+02:00"],
    ["2026-10-23", "+02:00"],
    ["2026-10-24", "+02:00"],
    ["2026-10-25", "+01:00"],
    ["2026-10-26", "+01:00"],
  ];
  const dir = fixture({ terminalB: days.flatMap(([d, o]) => shiftB("B-33", d, o, ["09:00", "14:00"])) });
  assert.deepEqual(byId(run(dir).employees, "E-3").minutes, { ...ZERO, regular: 1500 });
});

test("only regular minutes count toward the weekly threshold", () => {
  const dir = fixture({
    terminalB: [
      ...WEEK1.flatMap((d) => shiftB("B-11", d, "+02:00", ["07:00", "17:30"], [["12:00", "12:30"]])),
      ...shiftB("B-11", "2026-10-24", "+02:00", ["08:00", "12:00"]),
    ],
  });
  assert.deepEqual(byId(run(dir).employees, "E-1").minutes, { ...ZERO, regular: 2400, ot1: 5 * 120 + 240 });
});

test("legacy contracts use their own thresholds from contracts.csv", () => {
  const dir = fixture({ terminalB: shiftB("B-44", "2026-10-20", "+02:00", ["07:00", "18:00"], [["11:00", "11:30"]]) });
  const e4 = byId(run(dir).employees, "E-4");
  assert.deepEqual(e4.minutes, { ...ZERO, regular: 420, ot1: 180, ot2: 30 });
  assert.equal(e4.grossCents, 16800 + 10800 + 2400);
});

test("shifts on the same date share the daily count", () => {
  const dir = fixture({
    terminalB: [
      ...shiftB("B-11", "2026-10-20", "+02:00", ["06:00", "11:00"]),
      ...shiftB("B-11", "2026-10-20", "+02:00", ["13:00", "19:00"]),
    ],
  });
  assert.deepEqual(byId(run(dir).employees, "E-1").minutes, { ...ZERO, regular: 480, ot1: 180 });
});
