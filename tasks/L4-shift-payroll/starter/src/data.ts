import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import type { Employee } from "./types.ts";

export function loadEmployees(dataDir: string): Employee[] {
  const text = readFileSync(join(dataDir, "employees.csv"), "utf8");
  return parseRecords(text).map((r) => ({
    id: r.employee_id,
    badge: r.badge,
    name: r.name,
    site: r.site,
    contract: r.contract,
    rateCents: Math.round(parseFloat(r.hourly_rate_eur) * 100),
  }));
}
