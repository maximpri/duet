import { loadEmployees } from "./data.ts";
import { loadPunches } from "./punches.ts";
import type { EmployeePay, PayrollResult } from "./types.ts";

/**
 * Pay for the period `start`..`end` (inclusive local dates, `YYYY-MM-DD`).
 *
 * Current behaviour (the pre-agreement system): every IN is paired with the next
 * OUT and paid at the base rate. No rounding, breaks, overtime or premiums.
 */
export function computePayroll(dataDir: string, start: string, end: string): PayrollResult {
  const employees = loadEmployees(dataDir);
  const { punches, exceptions } = loadPunches(dataDir);
  const pay: EmployeePay[] = [];
  for (const e of [...employees].sort((a, b) => a.id.localeCompare(b.id))) {
    let minutes = 0;
    let open: number | null = null;
    for (const p of punches) {
      if (p.employeeId !== e.id) continue;
      const day = p.at.slice(0, 10);
      if (day < start || day > end) continue;
      const t = Date.parse(p.at);
      if (p.kind === "IN") open = t;
      if (p.kind === "OUT" && open !== null) {
        minutes += (t - open) / 60_000;
        open = null;
      }
    }
    pay.push({
      employeeId: e.id,
      minutes: { regular: minutes, ot1: 0, ot2: 0, holiday: 0, night: 0 },
      grossCents: Math.round((minutes * e.rateCents) / 60),
    });
  }
  return { period: { start, end }, employees: pay, exceptions };
}
