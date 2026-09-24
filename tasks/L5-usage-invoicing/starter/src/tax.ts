import type { TaxTable } from "./config.ts";
import type { Account, TaxRule } from "./types.ts";

/** Tax rule and rate for a root account. */
export function taxFor(table: TaxTable, root: Account): { rule: TaxRule; ratePct: string } {
  const country = root.country;
  // Business customers with a VAT id account for the tax themselves.
  if (root.vatId !== "") return { rule: "reverse_charge", ratePct: "0" };
  if (country === table.sellerCountry) return { rule: "domestic", ratePct: table.rates[country] };
  if (table.euMembers.has(country)) return { rule: "oss", ratePct: table.rates[country] };
  return { rule: "export", ratePct: "0" };
}
