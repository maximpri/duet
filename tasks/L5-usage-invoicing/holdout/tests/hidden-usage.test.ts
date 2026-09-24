import { test } from "node:test";
import assert from "node:assert/strict";
import { loadUsage } from "../src/usage.ts";
import { a, fixture } from "./fixture.ts";

const brief = (dir: string) => loadUsage(dir).events.map((e) => [e.accountId, e.meter, e.quantity, e.at]);

test("usage: edge-b account spellings map to the canonical account ids", () => {
  const dir = fixture({
    edgeB: [
      { id: "b1", account: "acc-00001", metric: "api_requests", value: 10, unit: "request", at: "2026-10-05T10:00:00.000+02:00" },
      { id: "b2", account: "acc-00011", metric: "api_requests", value: 20, unit: "request", at: "2026-10-05T11:00:00.000+02:00" },
      { id: "b3", account: "acc-00013", metric: "build_minutes", value: 7, at: "2026-10-05T12:00:00.000+02:00" },
    ],
  });
  const { events, exceptions } = loadUsage(dir);
  assert.deepEqual(exceptions, []);
  assert.deepEqual(
    events.map((e) => [e.accountId, e.quantity]),
    [
      ["ACC-1", "10"],
      ["ACC-11", "20"],
      ["ACC-13", "7"],
    ],
  );
});

test("usage: quantities are converted exactly into the canonical unit", () => {
  const dir = fixture({
    edgeA: [a("ACC-1", "storage_gib_h", "12.125", "2026-10-02 00:00:00", "GiB-h"), a("ACC-1", "egress_gb", "3.5", "2026-10-02 00:00:01", "GB")],
    edgeB: [
      { id: "b1", account: "acc-00001", metric: "storage_gib_h", value: 2048, unit: "MiB-h", at: "2026-10-02T02:00:02.000+02:00" },
      { id: "b2", account: "acc-00001", metric: "storage_gib_h", value: 1536.5, unit: "MiB-h", at: "2026-10-02T02:00:03.000+02:00" },
      { id: "b3", account: "acc-00001", metric: "egress_gb", value: 1500000000, unit: "B", at: "2026-10-02T02:00:04.000+02:00" },
      { id: "b4", account: "acc-00001", metric: "egress_gb", value: 2500, unit: "MB", at: "2026-10-02T02:00:05.000+02:00" },
      { id: "b5", account: "acc-00001", metric: "egress_gb", value: 7, at: "2026-10-02T02:00:06.000+02:00" },
    ],
  });
  assert.deepEqual(
    loadUsage(dir).events.map((e) => [e.meter, e.quantity]),
    [
      ["storage_gib_h", "12.125"],
      ["egress_gb", "3.5"],
      ["storage_gib_h", "2"],
      ["storage_gib_h", "1.50048828125"],
      ["egress_gb", "1.5"],
      ["egress_gb", "2.5"],
      ["egress_gb", "7"],
    ],
  );
});

test("usage: an event re-delivered by the other collector counts once", () => {
  const dir = fixture({
    edgeA: [
      "9F3AC0DE11,ACC-1,api_requests,500,request,2026-10-14 01:30:00",
      "9F3AC0DE12,ACC-1,storage_gib_h,4,GiB-h,2026-10-14 01:31:00",
    ],
    edgeB: [
      { id: "9f3ac0de11", account: "acc-00001", metric: "api_requests", value: 500, unit: "request", at: "2026-10-14T03:30:00.000+02:00" },
      { id: "9f3ac0de12", account: "acc-00001", metric: "storage_gib_h", value: 4096, unit: "MiB-h", at: "2026-10-14T03:31:00.000+02:00" },
      { id: "9f3ac0de13", account: "acc-00001", metric: "api_requests", value: 1, unit: "request", at: "2026-10-14T03:32:00.000+02:00" },
    ],
  });
  assert.deepEqual(brief(dir), [
    ["ACC-1", "api_requests", "500", "2026-10-14T01:30:00.000Z"],
    ["ACC-1", "storage_gib_h", "4", "2026-10-14T01:31:00.000Z"],
    ["ACC-1", "api_requests", "1", "2026-10-14T01:32:00.000Z"],
  ]);
});

test("usage: unknown accounts and meters are reported once per event", () => {
  const dir = fixture({
    edgeA: [
      "C0FFEE01,ACC-0999,api_requests,5,request,2026-10-03 00:00:00",
      "C0FFEE02,ACC-0999,api_requests,5,request,2026-10-03 00:00:01",
      "C0FFEE03,ACC-1,gpu_seconds,5,,2026-10-03 00:00:02",
      "C0FFEE04,ACC-1,api_requests,5,request,2026-11-20 00:00:00",
    ],
    edgeB: [
      { id: "c0ffee01", account: "ACC-0999", metric: "api_requests", value: 5, unit: "request", at: "2026-10-03T02:00:00.000+02:00" },
      { id: "c0ffee05", account: "acc-09999", metric: "gpu_seconds", value: 5, at: "2026-10-03T02:00:03.000+02:00" },
      { id: "c0ffee06", account: "acc-00007", metric: "gpu_seconds", value: 5, at: "2026-10-03T02:00:04.000+02:00" },
      { id: "c0ffee07", account: "probe", metric: "api_requests", value: 5, at: "2026-08-01T00:00:00.000Z" },
    ],
  });
  const { events, exceptions } = loadUsage(dir);
  assert.equal(events.length, 1);
  assert.deepEqual(exceptions, [
    { code: "UNKNOWN_ACCOUNT", ref: "ACC-0999", count: 2 },
    { code: "UNKNOWN_ACCOUNT", ref: "acc-09999", count: 1 },
    { code: "UNKNOWN_ACCOUNT", ref: "probe", count: 1 },
    { code: "UNKNOWN_METER", ref: "gpu_seconds", count: 2 },
  ]);
});

test("usage: events from both collectors are merged in time order", () => {
  const dir = fixture({
    edgeA: [a("ACC-2", "api_requests", "1", "2026-10-25 01:30:00"), a("ACC-2", "api_requests", "2", "2026-10-25 00:59:59")],
    edgeB: [
      { id: "b1", account: "ACC-2", metric: "api_requests", value: 3, unit: "request", at: "2026-10-25T02:15:00.250+01:00" },
      { id: "b2", account: "ACC-2", metric: "api_requests", value: 4, unit: "request", at: "2026-10-25T02:15:00.250+02:00" },
    ],
  });
  assert.deepEqual(
    loadUsage(dir).events.map((e) => [e.quantity, e.at]),
    [
      ["4", "2026-10-25T00:15:00.250Z"],
      ["2", "2026-10-25T00:59:59.000Z"],
      ["3", "2026-10-25T01:15:00.250Z"],
      ["1", "2026-10-25T01:30:00.000Z"],
    ],
  );
});
