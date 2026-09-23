import { test } from "node:test";
import assert from "node:assert/strict";
import { loadPunches } from "../src/punches.ts";
import { b, fixture } from "./fixture.ts";

const of = (id: string, punches: { employeeId: string; at: string; kind: string }[]) =>
  punches.filter((p) => p.employeeId === id).map((p) => `${p.at} ${p.kind}`);

test("reads generation-1 terminal exports as local time", () => {
  const dir = fixture({
    terminalA: [
      "0011;20.10.2026 07:00;K;T1",
      "0011;20.10.2026 11:00;PA;T2",
      "0011;20.10.2026 11:30;PE;T2",
      "0011;20.10.2026 15:30;G;T1",
      "0011;27.10.2026 07:00;K;T1",
      "0011;27.10.2026 12:00;G;T1",
    ],
  });
  assert.deepEqual(of("E-1", loadPunches(dir).punches), [
    "2026-10-20T05:00:00Z IN",
    "2026-10-20T09:00:00Z BREAK_START",
    "2026-10-20T09:30:00Z BREAK_END",
    "2026-10-20T13:30:00Z OUT",
    "2026-10-27T06:00:00Z IN",
    "2026-10-27T11:00:00Z OUT",
  ]);
});

test("reads current terminal exports with offsets and drops seconds", () => {
  const dir = fixture({
    terminalB: [
      b("B-22", "2026-10-20T07:03:59+02:00", "clock_in"),
      b("B-22", "2026-10-20T10:00:31+02:00", "break_start"),
      b("B-22", "2026-10-20T10:15:02+02:00", "break_end"),
      b("B-22", "2026-10-26T16:10:30+01:00", "clock_out"),
    ],
  });
  assert.deepEqual(of("E-2", loadPunches(dir).punches), [
    "2026-10-20T05:03:00Z IN",
    "2026-10-20T08:00:00Z BREAK_START",
    "2026-10-20T08:15:00Z BREAK_END",
    "2026-10-26T15:10:00Z OUT",
  ]);
});

test("matches badge spellings from every system to the same employee", () => {
  const dir = fixture({
    terminalA: ["0022;21.10.2026 07:00;K;T1"],
    terminalB: [
      b(" b-22 ", "2026-10-21T12:00+02:00", "break_start"),
      b("B-22", "2026-10-21T12:30+02:00", "break_end"),
      b("b-22", "2026-10-21T16:00+02:00", "clock_out"),
    ],
  });
  const { punches, exceptions } = loadPunches(dir);
  assert.deepEqual(of("E-2", punches), [
    "2026-10-21T05:00:00Z IN",
    "2026-10-21T10:00:00Z BREAK_START",
    "2026-10-21T10:30:00Z BREAK_END",
    "2026-10-21T14:00:00Z OUT",
  ]);
  assert.deepEqual(exceptions, []);
});

test("punches with unknown badges are dropped and reported", () => {
  const dir = fixture({
    terminalA: ["9001;22.10.2026 09:14;K;T3", "0011;22.10.2026 08:00;K;T1"],
    terminalB: [b(" B-77 ", "2026-10-22T08:30+02:00", "clock_in"), b("B-220", "2026-10-23T08:30+02:00", "clock_in")],
  });
  const { punches, exceptions } = loadPunches(dir);
  assert.deepEqual(punches.map((p) => p.employeeId), ["E-1"]);
  assert.deepEqual(exceptions, [
    { code: "UNKNOWN_BADGE", ref: "B-77", at: "2026-10-22T06:30:00Z" },
    { code: "UNKNOWN_BADGE", ref: "9001", at: "2026-10-22T07:14:00Z" },
    { code: "UNKNOWN_BADGE", ref: "B-220", at: "2026-10-23T06:30:00Z" },
  ]);
});

test("applies approved corrections only", () => {
  const dir = fixture({
    terminalB: [
      b("B-11", "2026-10-20T06:20+02:00", "clock_in"),
      b("B-11", "2026-10-20T07:00+02:00", "clock_in"),
      b("B-11", "2026-10-20T15:00+02:00", "clock_out"),
      b("B-11", "2026-10-21T07:00+02:00", "clock_in"),
    ],
    corrections: [
      'B-11,2026-10-20 06:20,IN,VOID,APPROVED,Sup A,"reader misfire"',
      'B-11,2026-10-21 15:30,OUT,ADD,approved,Sup A,"forgot to clock out"',
      'B-11,2026-10-22 07:00,IN,ADD,PENDING,,"awaiting"',
      'B-11,2026-10-22 15:00,OUT,ADD,REJECTED,Sup B,"not approved"',
      'B-11,2026-10-23 07:00,IN,VOID,APPROVED,Sup A,"nothing to void"',
    ],
  });
  assert.deepEqual(of("E-1", loadPunches(dir).punches), [
    "2026-10-20T05:00:00Z IN",
    "2026-10-20T13:00:00Z OUT",
    "2026-10-21T05:00:00Z IN",
    "2026-10-21T13:30:00Z OUT",
  ]);
});

test("drops double taps of the same kind within five minutes", () => {
  const dir = fixture({
    terminalB: [
      b("B-11", "2026-10-20T07:00+02:00", "clock_in"),
      b("B-11", "2026-10-20T07:03:40+02:00", "clock_in"),
      b("B-11", "2026-10-20T15:00+02:00", "clock_out"),
      b("B-11", "2026-10-20T15:05:10+02:00", "clock_out"),
      b("B-11", "2026-10-21T07:00+02:00", "clock_in"),
      b("B-11", "2026-10-21T07:06+02:00", "clock_in"),
      b("B-11", "2026-10-21T07:08+02:00", "clock_out"),
    ],
  });
  assert.deepEqual(of("E-1", loadPunches(dir).punches), [
    "2026-10-20T05:00:00Z IN",
    "2026-10-20T13:00:00Z OUT",
    "2026-10-21T05:00:00Z IN",
    "2026-10-21T05:06:00Z IN",
    "2026-10-21T05:08:00Z OUT",
  ]);
});

test("the repeated hour at the end of summer time means its first occurrence", () => {
  const dir = fixture({
    terminalA: ["0055;25.10.2026 01:30;K;T1", "0055;25.10.2026 02:30;PA;T1", "0055;25.10.2026 03:30;G;T1"],
  });
  assert.deepEqual(of("E-5", loadPunches(dir).punches), [
    "2026-10-24T23:30:00Z IN",
    "2026-10-25T00:30:00Z BREAK_START",
    "2026-10-25T02:30:00Z OUT",
  ]);
});

test("punches are sorted by employee, then time, across files", () => {
  const dir = fixture({
    terminalA: ["0044;20.10.2026 12:00;G;T1", "0011;20.10.2026 09:00;G;T1"],
    terminalB: [b("B-44", "2026-10-20T06:00+02:00", "clock_in"), b("B-11", "2026-10-20T05:00+02:00", "clock_in")],
  });
  const { punches } = loadPunches(dir);
  assert.deepEqual(
    punches.map((p) => `${p.employeeId} ${p.at}`),
    ["E-1 2026-10-20T03:00:00Z", "E-1 2026-10-20T07:00:00Z", "E-4 2026-10-20T04:00:00Z", "E-4 2026-10-20T10:00:00Z"],
  );
});
