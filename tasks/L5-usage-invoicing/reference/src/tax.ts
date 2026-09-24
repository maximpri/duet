import type { TaxTable } from "./config.ts";
import type { Account, TaxRule } from "./types.ts";

/** Tax rule and rate for a root account (RULES.md §8.1). */
export function taxFor(table: TaxTable, root: Account): { rule: TaxRule; ratePct: string } {
  const country = root.country;
  if (country === table.sellerCountry) return { rule: "domestic", ratePct: table.rates[country] };
  if (table.euMembers.has(country)) {
    if (root.vatId !== "") return { rule: "reverse_charge", ratePct: "0" };
    return { rule: "oss", ratePct: table.rates[country] };
  }
  return { rule: "export", ratePct: "0" };
}
