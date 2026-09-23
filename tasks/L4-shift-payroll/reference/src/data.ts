import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import type { Employee } from "./types.ts";

/** Euro amount (`18.50`, `21,75`, `19`) to integer cents. */
export function parseEuroCents(text: string): number {
  const [whole, frac = ""] = text.trim().replace(",", ".").split(".");
  return Number(whole) * 100 + Number((frac + "00").slice(0, 2));
}

export function loadEmployees(dataDir: string): Employee[] {
  const text = readFileSync(join(dataDir, "employees.csv"), "utf8");
  return parseRecords(text).map((r) => ({
    id: r.employee_id,
    badge: r.badge,
    name: r.name,
    site: r.site,
    contract: r.contract,
    rateCents: parseEuroCents(r.hourly_rate_eur),
  }));
}

export interface Contract {
  code: string;
  dailyOt1After: number | null;
  dailyOt2After: number | null;
  weeklyOtAfter: number | null;
  nightPremiumPct: number;
}

const optional = (s: string): number | null => (s.trim() === "" ? null : Number(s));

export function loadContracts(dataDir: string): Map<string, Contract> {
  const text = readFileSync(join(dataDir, "contracts.csv"), "utf8");
  return new Map(
    parseRecords(text).map((r) => [
      r.code,
      {
        code: r.code,
        dailyOt1After: optional(r.daily_ot1_after_min),
        dailyOt2After: optional(r.daily_ot2_after_min),
        weeklyOtAfter: optional(r.weekly_ot_after_min),
        nightPremiumPct: Number(r.night_premium_pct || 0),
      },
    ]),
  );
}

/** site code -> region code */
export function loadSites(dataDir: string): Map<string, string> {
  const text = readFileSync(join(dataDir, "sites.csv"), "utf8");
  return new Map(parseRecords(text).map((r) => [r.site, r.region]));
}

/** True when `date` is a public holiday in `region` (`*` rows apply everywhere). */
export function loadHolidays(dataDir: string): (date: string, region: string) => boolean {
  const text = readFileSync(join(dataDir, "holidays.csv"), "utf8");
  const set = new Set(parseRecords(text).map((r) => `${r.date}|${r.region}`));
  return (date, region) => set.has(`${date}|${region}`) || set.has(`${date}|*`);
}
