import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import { Q } from "./rational.ts";
import type { AppliedCredit } from "./types.ts";

export interface Credit {
  id: string;
  accountId: string;
  grantedOn: string;
  /** Last valid day; empty when the credit never expires. */
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

/** Credits usable on an invoice, oldest grant first. */
export function eligibleCredits(credits: Credit[], tree: Set<string>, _first: string, _last: string): Credit[] {
  return credits
    .filter((c) => tree.has(c.accountId) && c.remainingEur.cmp(Q.ZERO) > 0)
    .sort((a, b) => {
      if (a.grantedOn !== b.grantedOn) return a.grantedOn < b.grantedOn ? -1 : 1;
      return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
    });
}

/** Applies credits (values in minor units) against `due`. */
export function applyCredits(ordered: { id: string; value: bigint }[], due: bigint): AppliedCredit[] {
  const out: AppliedCredit[] = [];
  let left = due;
  for (const c of ordered) {
    const amount = c.value < left ? c.value : left;
    if (amount > 0n) {
      out.push({ creditId: c.id, amount: Number(amount) });
      left -= amount;
    }
  }
  return out;
}
