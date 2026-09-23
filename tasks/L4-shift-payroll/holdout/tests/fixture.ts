import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const PERIOD = ["2026-10-19", "2026-11-01"] as const;

const EMPLOYEES = [
  "employee_id,badge,name,email,phone,site,contract,hourly_rate_eur",
  "E-1,B-11,Test One,one@example.test,+49 30 000001,BER,FT,20.00",
  "E-2,B-22,Test Two,two@example.test,+49 30 000002,POT,FT,18.00",
  "E-3,B-33,Test Three,three@example.test,+49 30 000003,MUC,PT,16.00",
  "E-4,B-44,Test Four,four@example.test,+49 30 000004,BER,LEGACY-7,24.00",
  "E-5,B-55,Test Five,five@example.test,+49 30 000005,POT,NIGHT,30.00",
];

const CONTRACTS = [
  "code,daily_ot1_after_min,daily_ot2_after_min,weekly_ot_after_min,night_premium_pct,description",
  "FT,480,720,2400,25,Full time",
  "PT,,,1200,25,Part time",
  "LEGACY-7,420,600,2100,30,Legacy",
  "NIGHT,600,720,2400,40,Night crew",
];

const HOLIDAYS = [
  "date,region,name",
  "2026-10-03,*,National day",
  "2026-10-31,BB,Reformation Day",
  "2026-11-01,BY,All Saints",
];

export interface FixtureInput {
  /** Extra employee rows (appended). */
  employees?: string[];
  /** Generation-1 rows `badge;DD.MM.YYYY HH:MM;code;terminal` (header added). */
  terminalA?: string[];
  /** Current terminal records. */
  terminalB?: { badge: string; ts: string; event: string }[];
  corrections?: string[];
  /** Extra holiday rows (appended). */
  holidays?: string[];
}

export function fixture(input: FixtureInput = {}): string {
  const dir = mkdtempSync(join(tmpdir(), "payroll-hidden-"));
  mkdirSync(join(dir, "punches"));
  const w = (name: string, lines: string[]) => writeFileSync(join(dir, name), lines.join("\n") + "\n");
  w("employees.csv", [...EMPLOYEES, ...(input.employees ?? [])]);
  w("contracts.csv", CONTRACTS);
  w("sites.csv", ["site,region,city", "BER,BE,Berlin", "POT,BB,Potsdam", "MUC,BY,Munich"]);
  w("holidays.csv", [...HOLIDAYS, ...(input.holidays ?? [])]);
  w("corrections.csv", ["badge,local_time,kind,action,status,approved_by,comment", ...(input.corrections ?? [])]);
  w("punches/terminal-a-test.csv", [
    "# Test export. Buchung: K = Kommen, G = Gehen, PA = Pausenanfang, PE = Pausenende",
    "Ausweis;Zeitpunkt;Buchung;Terminal",
    ...(input.terminalA ?? []),
  ]);
  writeFileSync(
    join(dir, "punches/terminal-b-test.jsonl"),
    (input.terminalB ?? []).map((r) => JSON.stringify({ ...r, device: "TB-T-1" }) + "\n").join(""),
  );
  return dir;
}

/** Current-terminal record. `ts` like `2026-10-20T08:00+02:00` (seconds optional). */
export function b(badge: string, ts: string, event: "clock_in" | "clock_out" | "break_start" | "break_end") {
  const full = /T\d{2}:\d{2}[+-]/.test(ts) ? ts.replace(/(T\d{2}:\d{2})/, "$1:00") : ts;
  return { badge, ts: full, event };
}

/** One current-terminal shift on `date` with local times `HH:MM` and an explicit offset. */
export function shiftB(badge: string, date: string, offset: string, times: [string, string], breaks: [string, string][] = []) {
  const at = (hm: string) => b(badge, `${date}T${hm}${offset}`, "clock_in").ts;
  const out = [{ badge, ts: at(times[0]), event: "clock_in" }];
  for (const [s, e] of breaks) {
    out.push({ badge, ts: at(s), event: "break_start" }, { badge, ts: at(e), event: "break_end" });
  }
  out.push({ badge, ts: at(times[1]), event: "clock_out" });
  return out;
}

export function byId<T extends { employeeId: string }>(rows: T[], id: string): T {
  const row = rows.find((r) => r.employeeId === id);
  if (!row) throw new Error(`no row for ${id}`);
  return row;
}

export const ZERO = { regular: 0, ot1: 0, ot2: 0, holiday: 0, night: 0 };
