import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import type { Account } from "./types.ts";

export function loadAccounts(dataDir: string): Map<string, Account> {
  const out = new Map<string, Account>();
  for (const r of parseRecords(readFileSync(join(dataDir, "accounts.csv"), "utf8"))) {
    out.set(r.account_id, {
      id: r.account_id,
      parentId: r.parent_id,
      country: r.country,
      vatId: r.vat_id,
      currency: r.currency,
      timezone: r.timezone,
    });
  }
  return out;
}

/** The root account of `id` (RULES.md §1.1). */
export function rootOf(accounts: Map<string, Account>, id: string): Account {
  let account = accounts.get(id);
  if (!account) throw new Error(`unknown account ${id}`);
  const seen = new Set<string>();
  while (account.parentId !== "") {
    if (seen.has(account.id)) throw new Error(`account cycle at ${account.id}`);
    seen.add(account.id);
    const parent = accounts.get(account.parentId);
    if (!parent) throw new Error(`account ${account.id} has unknown parent ${account.parentId}`);
    account = parent;
  }
  return account;
}
