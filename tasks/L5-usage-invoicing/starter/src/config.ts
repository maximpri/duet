import { readFileSync } from "node:fs";
import { Q } from "./rational.ts";

export interface Tier {
  upTo: Q | null;
  unitPrice: Q;
}

export interface Meter {
  code: string;
  unit: string;
  pricing: "graduated" | "volume";
  tiers: Tier[];
}

export interface Plan {
  code: string;
  monthlyFee: Q;
  included: Record<string, number>;
}

export interface Catalog {
  meters: Map<string, Meter>;
  plans: Map<string, Plan>;
}

export interface TaxTable {
  sellerCountry: string;
  euMembers: Set<string>;
  rates: Record<string, string>;
}

const configDir = new URL("../config/", import.meta.url);
const readJson = (name: string) => JSON.parse(readFileSync(new URL(name, configDir), "utf8"));

export function loadCatalog(): Catalog {
  const raw = readJson("catalog.json");
  const meters = new Map<string, Meter>();
  for (const [code, m] of Object.entries<any>(raw.meters)) {
    meters.set(code, {
      code,
      unit: m.unit,
      pricing: m.pricing,
      tiers: m.tiers.map((t: any) => ({
        upTo: t.up_to === null ? null : Q.of(BigInt(t.up_to)),
        unitPrice: Q.parse(t.unit_price_eur),
      })),
    });
  }
  const plans = new Map<string, Plan>();
  for (const [code, p] of Object.entries<any>(raw.plans)) {
    plans.set(code, { code, monthlyFee: Q.parse(p.monthly_fee_eur), included: p.included ?? {} });
  }
  return { meters, plans };
}

export function loadCurrencies(): Record<string, { decimals: number }> {
  return readJson("currencies.json");
}

export function loadTaxTable(): TaxTable {
  const raw = readJson("tax.json");
  return { sellerCountry: raw.seller_country, euMembers: new Set(raw.eu_members), rates: raw.rates };
}
