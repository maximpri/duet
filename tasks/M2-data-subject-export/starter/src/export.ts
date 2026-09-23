import { readStore } from "./store.ts";

export interface UserExport {
  user: Record<string, unknown>;
  orders: Record<string, unknown>[];
  tickets: Record<string, unknown>[];
}

export function exportUserData(dataDir: string, userId: string): UserExport {
  const users = readStore(dataDir, "users.jsonl");
  const user = users.find((u) => u.id === userId);
  if (!user) throw new Error(`unknown user ${userId}`);
  // TODO: orders and tickets
  return { user, orders: [], tickets: [] };
}
