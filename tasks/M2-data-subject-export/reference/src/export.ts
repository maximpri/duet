import { readStore } from "./store.ts";

type Row = Record<string, unknown>;

export interface UserExport {
  user: Row;
  orders: Row[];
  tickets: Row[];
}

/** Deep copy without keys starting with "_". */
function stripInternal(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stripInternal);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Row)
        .filter(([k]) => !k.startsWith("_"))
        .map(([k, v]) => [k, stripInternal(v)]),
    );
  }
  return value;
}

const normalizeEmail = (e: unknown) => (typeof e === "string" ? e.trim().toLowerCase() : "");

/** Legacy orders store `customer_ref: "u:<n>"` for user id `u-<n>`. */
function orderOwner(order: Row): string | undefined {
  if (typeof order.user_id === "string") return order.user_id;
  if (typeof order.customer_ref === "string" && order.customer_ref.startsWith("u:")) {
    return `u-${order.customer_ref.slice(2)}`;
  }
  return undefined;
}

const byField = (field: string) => (a: Row, b: Row) => String(a[field]).localeCompare(String(b[field]));

export function exportUserData(dataDir: string, userId: string): UserExport {
  const user = readStore(dataDir, "users.jsonl").find((u) => u.id === userId);
  if (!user) throw new Error(`unknown user ${userId}`);
  const email = normalizeEmail(user.email);
  const orders = readStore(dataDir, "orders.jsonl")
    .filter((o) => orderOwner(o) === userId)
    .sort(byField("placed_at"));
  const tickets = readStore(dataDir, "tickets.jsonl")
    .filter((t) => email !== "" && normalizeEmail(t.requester_email) === email)
    .sort(byField("opened_at"));
  return {
    user: stripInternal(user) as Row,
    orders: orders.map((o) => stripInternal(o) as Row),
    tickets: tickets.map((t) => stripInternal(t) as Row),
  };
}
