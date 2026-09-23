import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { exportUserData } from "../src/export.ts";

const USERS = [
  { id: "u-2001", name: "Rin Tal", email: "rin.tal@example.test", created_at: "2020-01-01T00:00:00Z", _score: 1, prefs: { theme: "dark", _cohort: 3 } },
  { id: "u-2002", name: "Oda Mae", email: "Oda.Mae@Example.test", created_at: "2020-02-02T00:00:00Z" },
  { id: "u-2003", name: "Other", email: "other@example.test", created_at: "2020-03-03T00:00:00Z" },
];
const ORDERS = [
  { order_id: "a", user_id: "u-2001", placed_at: "2024-05-01T00:00:00Z", ship: { city: "Oslo", _cost: 9 } },
  { order_id: "b", customer_ref: "u:2001", placed_at: "2019-01-01T00:00:00Z", _flag: true },
  { order_id: "c", user_id: "u-2003", placed_at: "2021-01-01T00:00:00Z" },
  { order_id: "d", customer_ref: "u:2003", placed_at: "2018-01-01T00:00:00Z" },
  { order_id: "e", user_id: "u-2001", placed_at: "2022-06-15T12:00:00Z" },
  { order_id: "f", customer_ref: "u:20011", placed_at: "2022-01-01T00:00:00Z" },
];
const TICKETS = [
  { ticket: "t1", requester_email: "rin.tal@example.test", opened_at: "2024-01-02T00:00:00Z", _agent: "x" },
  { ticket: "t2", requester_email: " RIN.TAL@EXAMPLE.TEST ", opened_at: "2023-01-02T00:00:00Z" },
  { ticket: "t3", requester_email: "oda.mae@example.test", opened_at: "2022-01-02T00:00:00Z", _internal: { a: 1 } },
  { ticket: "t4", requester_email: "other@example.test", opened_at: "2021-01-02T00:00:00Z" },
];

function store(): string {
  const dir = mkdtempSync(join(tmpdir(), "dsar-hidden-"));
  const w = (f: string, rows: object[]) => writeFileSync(join(dir, f), rows.map((r) => JSON.stringify(r)).join("\n") + "\n");
  w("users.jsonl", USERS);
  w("orders.jsonl", ORDERS);
  w("tickets.jsonl", TICKETS);
  return dir;
}

const ids = (rows: Record<string, unknown>[], key: string) => rows.map((r) => r[key]);
const hasUnderscoreKey = (v: unknown): boolean =>
  Array.isArray(v) ? v.some(hasUnderscoreKey)
  : v !== null && typeof v === "object" ? Object.entries(v).some(([k, x]) => k.startsWith("_") || hasUnderscoreKey(x))
  : false;

test("user record without internal fields", () => {
  const out = exportUserData(store(), "u-2001");
  assert.equal(out.user.id, "u-2001");
  assert.equal(out.user.email, "rin.tal@example.test");
  assert.equal("_score" in out.user, false);
});

test("orders linked by user_id", () => {
  assert.ok(ids(exportUserData(store(), "u-2001").orders, "order_id").includes("a"));
});

test("legacy orders linked by customer_ref", () => {
  const got = ids(exportUserData(store(), "u-2001").orders, "order_id");
  assert.ok(got.includes("b"));
  assert.ok(!got.includes("f"), "u:20011 is a different user");
});

test("tickets linked by requester email", () => {
  assert.ok(ids(exportUserData(store(), "u-2001").tickets, "ticket").includes("t1"));
});

test("email matching ignores case and surrounding spaces", () => {
  assert.ok(ids(exportUserData(store(), "u-2001").tickets, "ticket").includes("t2"));
  assert.deepEqual(ids(exportUserData(store(), "u-2002").tickets, "ticket"), ["t3"]);
});

test("never includes another user's records", () => {
  const out = exportUserData(store(), "u-2001");
  assert.deepEqual(ids(out.orders, "order_id").sort(), ["a", "b", "e"]);
  assert.deepEqual(ids(out.tickets, "ticket").sort(), ["t1", "t2"]);
});

test("internal fields removed at every depth", () => {
  const out = exportUserData(store(), "u-2001");
  assert.equal(hasUnderscoreKey(out), false);
  assert.equal((out.user.prefs as Record<string, unknown>).theme, "dark");
  const a = out.orders.find((o) => o.order_id === "a")!;
  assert.deepEqual(a.ship, { city: "Oslo" });
});

test("orders sorted oldest first", () => {
  assert.deepEqual(ids(exportUserData(store(), "u-2001").orders, "order_id"), ["b", "e", "a"]);
});

test("tickets sorted oldest first", () => {
  assert.deepEqual(ids(exportUserData(store(), "u-2001").tickets, "ticket"), ["t2", "t1"]);
});

test("unknown user throws and inputs are unchanged", () => {
  const dir = store();
  const before = readFileSync(join(dir, "orders.jsonl"), "utf8");
  assert.throws(() => exportUserData(dir, "u-9999"), /^Error: unknown user u-9999$/);
  exportUserData(dir, "u-2001");
  assert.equal(readFileSync(join(dir, "orders.jsonl"), "utf8"), before);
});
