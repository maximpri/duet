import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { loadEmployees } from "./data.ts";
import { parseOffsetTimestamp, toIsoMinute } from "./time.ts";
import type { PayrollException, Punch, PunchKind } from "./types.ts";

const EVENTS: Record<string, PunchKind> = {
  clock_in: "IN",
  clock_out: "OUT",
  break_start: "BREAK_START",
  break_end: "BREAK_END",
};

/**
 * Reads every punch file in `dataDir/punches`.
 * Only the JSON-lines export of the current terminals is supported.
 */
export function loadPunches(dataDir: string): { punches: Punch[]; exceptions: PayrollException[] } {
  const byBadge = new Map(loadEmployees(dataDir).map((e) => [e.badge, e.id]));
  const dir = join(dataDir, "punches");
  const punches: Punch[] = [];
  const exceptions: PayrollException[] = [];
  for (const name of readdirSync(dir).sort()) {
    if (!name.endsWith(".jsonl")) continue;
    for (const line of readFileSync(join(dir, name), "utf8").split("\n")) {
      if (line.trim() === "") continue;
      const rec = JSON.parse(line);
      const badge = String(rec.badge).trim().toUpperCase();
      const at = toIsoMinute(parseOffsetTimestamp(rec.ts));
      const employeeId = byBadge.get(badge);
      if (!employeeId) {
        exceptions.push({ code: "UNKNOWN_BADGE", ref: badge, at });
        continue;
      }
      punches.push({ employeeId, at, kind: EVENTS[rec.event] });
    }
  }
  punches.sort((a, b) => a.employeeId.localeCompare(b.employeeId) || a.at.localeCompare(b.at));
  return { punches, exceptions };
}
