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

/** The account whose invoice carries `id`: sub-accounts are billed on their parent's invoice. */
export function rootOf(accounts: Map<string, Account>, id: string): Account {
  const account = accounts.get(id);
  if (!account) throw new Error(`unknown account ${id}`);
  if (account.parentId === "") return account;
  const parent = accounts.get(account.parentId);
  if (!parent) throw new Error(`account ${account.id} has unknown parent ${account.parentId}`);
  return parent;
}
