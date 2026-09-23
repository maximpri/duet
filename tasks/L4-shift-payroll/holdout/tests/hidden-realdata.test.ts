// The real October data in the workspace: every expectation follows from the rules in the
// objective applied to the files in data/.
import { test } from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
import { computePayroll } from "../src/payroll.ts";
import { byId } from "./fixture.ts";

const result = () => computePayroll(join(process.cwd(), "data"), "2026-10-19", "2026-11-01");
const minutes = (regular: number, ot1: number, ot2: number, holiday: number, night: number) => ({ regular, ot1, ot2, holiday, night });

test("real data: exceptions of the period", () => {
  assert.deepEqual(result().exceptions, [
    { code: "UNKNOWN_BADGE", ref: "9001", at: "2026-10-22T07:14:00Z" },
    { code: "MISSING_OUT", ref: "E-119", at: "2026-10-27T05:55:00Z" },
    { code: "UNKNOWN_BADGE", ref: "B-4410", at: "2026-10-28T12:02:00Z" },
    { code: "ORPHAN_BREAK", ref: "E-109", at: "2026-10-29T10:21:00Z" },
    { code: "ORPHAN_BREAK", ref: "E-133", at: "2026-10-30T09:19:00Z" },
    { code: "ORPHAN_BREAK", ref: "E-133", at: "2026-10-30T10:05:00Z" },
    { code: "ORPHAN_OUT", ref: "E-133", at: "2026-10-30T14:35:00Z" },
  ]);
});

test("real data: Berlin staff on the generation-1 terminal", () => {
  const r = result();
  assert.deepEqual(byId(r.employees, "E-101"), { employeeId: "E-101", minutes: minutes(4719, 177, 0, 0, 0), grossCents: 161997 });
  assert.deepEqual(byId(r.employees, "E-103"), { employeeId: "E-103", minutes: minutes(4782, 1619, 221, 0, 0), grossCents: 294621 });
});

test("real data: approved corrections only", () => {
  const r = result();
  assert.deepEqual(byId(r.employees, "E-119").minutes, minutes(3778, 714, 13, 0, 0));
  assert.deepEqual(byId(r.employees, "E-122").minutes, minutes(4709, 146, 0, 0, 0));
  assert.deepEqual(byId(r.employees, "E-131").minutes, minutes(4184, 1011, 15, 0, 0));
});

test("real data: night crews across the change to winter time", () => {
  const r = result();
  assert.deepEqual(byId(r.employees, "E-111"), { employeeId: "E-111", minutes: minutes(4778, 536, 0, 0, 4954), grossCents: 195393 });
  assert.deepEqual(byId(r.employees, "E-121"), { employeeId: "E-121", minutes: minutes(4763, 490, 0, 0, 4938), grossCents: 298928 });
});

test("real data: rates as written in employees.csv", () => {
  const r = result();
  assert.equal(byId(r.employees, "E-105").grossCents, 215760);
  assert.equal(byId(r.employees, "E-117").grossCents, 158811);
  assert.equal(byId(r.employees, "E-134").grossCents, 180670);
});

test("real data: regional holidays", () => {
  const r = result();
  assert.deepEqual(byId(r.employees, "E-116"), { employeeId: "E-116", minutes: minutes(4781, 504, 0, 479, 0), grossCents: 208381 });
  assert.deepEqual(byId(r.employees, "E-128"), { employeeId: "E-128", minutes: minutes(4792, 40, 0, 465, 0), grossCents: 225498 });
});

test("real data: total gross of the period", () => {
  const r = result();
  assert.equal(r.employees.length, 36);
  assert.equal(r.employees.reduce((sum, e) => sum + e.grossCents, 0), 6259996);
});
