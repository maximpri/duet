import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import { Q } from "./rational.ts";
import type { AppliedCredit } from "./types.ts";

export interface Credit {
  id: string;
  accountId: string;
  grantedOn: string;
  /** Inclusive last valid day; empty when the credit never expires. */
  expiresOn: string;
  remainingEur: Q;
}

export function loadCredits(dataDir: string): Credit[] {
  return parseRecords(readFileSync(join(dataDir, "credits.csv"), "utf8")).map((r) => ({
    id: r.credit_id,
    accountId: r.account_id,
    grantedOn: r.granted_on,
    expiresOn: r.expires_on,
    remainingEur: Q.parse(r.remaining_eur),
  }));
}

/** Credits eligible for an invoice, in application order (RULES.md §7.1, §7.3). */
export function eligibleCredits(credits: Credit[], tree: Set<string>, first: string, last: string): Credit[] {
  return credits
    .filter(
      (c) =>
        tree.has(c.accountId) &&
        c.grantedOn <= last &&
        (c.expiresOn === "" || c.expiresOn >= first) &&
        c.remainingEur.cmp(Q.ZERO) > 0,
    )
    .sort((a, b) => {
      if (a.expiresOn !== b.expiresOn) {
        if (a.expiresOn === "") return 1;
        if (b.expiresOn === "") return -1;
        return a.expiresOn < b.expiresOn ? -1 : 1;
      }
      if (a.grantedOn !== b.grantedOn) return a.grantedOn < b.grantedOn ? -1 : 1;
      return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
    });
}

/** Applies credits (values in minor units) against the usage sum. */
export function applyCredits(ordered: { id: string; value: bigint }[], usage: bigint): AppliedCredit[] {
  const out: AppliedCredit[] = [];
  let left = usage;
  for (const c of ordered) {
    const amount = c.value < left ? c.value : left;
    if (amount > 0n) {
      out.push({ creditId: c.id, amount: Number(amount) });
      left -= amount;
    }
  }
  return out;
}
