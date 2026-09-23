import { type Contract, loadContracts, loadEmployees, loadHolidays, loadSites } from "./data.ts";
import { loadPunches } from "./punches.ts";
import { berlinParts, mondayOf, toIsoMinute } from "./time.ts";
import type { EmployeePay, Minutes, PayrollException, PayrollResult, Punch } from "./types.ts";

const MINUTE = 60_000;
const QUARTER = 15;
const AUTO_BREAK_AFTER = 360;
const MIN_BREAK = 30;

interface Shift {
  date: string;
  /** Start of every paid minute, chronological. */
  minutes: number[];
}

/** Nearest quarter hour; 0–7 minutes past rounds down, 8–14 up. */
function roundQuarter(ms: number): number {
  const total = ms / MINUTE;
  const rest = ((total % QUARTER) + QUARTER) % QUARTER;
  return (rest <= 7 ? total - rest : total + (QUARTER - rest)) * MINUTE;
}

function localDate(iso: string): string {
  return berlinParts(Date.parse(iso)).date;
}

function buildShifts(employeeId: string, punches: Punch[], exceptions: PayrollException[]): Shift[] {
  const shifts: Shift[] = [];
  const missingOut = (inMs: number) => exceptions.push({ code: "MISSING_OUT", ref: employeeId, at: toIsoMinute(inMs) });
  let open: { in: number; breaks: [number, number][]; breakStart: number | null } | null = null;
  const close = (outMs: number) => {
    if (!open) return;
    if (open.breakStart !== null) open.breaks.push([open.breakStart, outMs]);
    const start = roundQuarter(open.in);
    const end = roundQuarter(outMs);
    const minutes: number[] = [];
    for (let t = start; t < end; t += MINUTE) {
      if (!open.breaks.some(([b, e]) => t >= b && t < e)) minutes.push(t);
    }
    const recordedBreak = Math.max(0, (end - start) / MINUTE - minutes.length);
    if (minutes.length > AUTO_BREAK_AFTER && recordedBreak < MIN_BREAK) {
      minutes.length -= MIN_BREAK - recordedBreak;
    }
    shifts.push({ date: berlinParts(open.in).date, minutes });
    open = null;
  };
  for (const p of punches) {
    const t = Date.parse(p.at);
    switch (p.kind) {
      case "IN":
        if (open) missingOut(open.in);
        open = { in: t, breaks: [], breakStart: null };
        break;
      case "OUT":
        if (open) close(t);
        else exceptions.push({ code: "ORPHAN_OUT", ref: p.employeeId, at: p.at });
        break;
      case "BREAK_START":
        if (open && open.breakStart === null) open.breakStart = t;
        else exceptions.push({ code: "ORPHAN_BREAK", ref: p.employeeId, at: p.at });
        break;
      case "BREAK_END":
        if (open && open.breakStart !== null) {
          open.breaks.push([open.breakStart, t]);
          open.breakStart = null;
        } else exceptions.push({ code: "ORPHAN_BREAK", ref: p.employeeId, at: p.at });
        break;
    }
  }
  if (open) missingOut(open.in);
  return shifts;
}

/** Integer division rounding half up. */
function divRound(n: number, d: number): number {
  return Math.floor((2 * n + d) / (2 * d));
}

function classify(shifts: Shift[], contract: Contract, isHoliday: (date: string) => boolean): Minutes {
  const m: Minutes = { regular: 0, ot1: 0, ot2: 0, holiday: 0, night: 0 };
  const daily = new Map<string, number>();
  const weekly = new Map<string, number>();
  for (const shift of shifts) {
    const holiday = isHoliday(shift.date);
    const week = mondayOf(shift.date);
    for (const t of shift.minutes) {
      const hour = berlinParts(t).hour;
      if (hour >= 22 || hour < 6) m.night++;
      if (holiday) {
        m.holiday++;
        continue;
      }
      const n = daily.get(shift.date) ?? 0;
      daily.set(shift.date, n + 1);
      if (contract.dailyOt2After !== null && n >= contract.dailyOt2After) m.ot2++;
      else if (contract.dailyOt1After !== null && n >= contract.dailyOt1After) m.ot1++;
      else {
        const w = weekly.get(week) ?? 0;
        if (contract.weeklyOtAfter !== null && w >= contract.weeklyOtAfter) m.ot1++;
        else {
          weekly.set(week, w + 1);
          m.regular++;
        }
      }
    }
  }
  return m;
}

export function grossCents(m: Minutes, rateCents: number, nightPremiumPct: number): number {
  return (
    divRound(m.regular * rateCents, 60) +
    divRound(m.ot1 * rateCents * 3, 120) +
    divRound(m.ot2 * rateCents * 2, 60) +
    divRound(m.holiday * rateCents * 2, 60) +
    divRound(m.night * rateCents * nightPremiumPct, 6000)
  );
}

/** Pay for the period `start`..`end` (inclusive Europe/Berlin dates, `YYYY-MM-DD`). */
export function computePayroll(dataDir: string, start: string, end: string): PayrollResult {
  const employees = loadEmployees(dataDir);
  const contracts = loadContracts(dataDir);
  const sites = loadSites(dataDir);
  const holidays = loadHolidays(dataDir);
  const loaded = loadPunches(dataDir);
  const exceptions: PayrollException[] = [...loaded.exceptions];
  const pay: EmployeePay[] = [];
  const inPeriod = (date: string) => date >= start && date <= end;

  for (const e of [...employees].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))) {
    const contract = contracts.get(e.contract);
    if (!contract) throw new Error(`employee ${e.id}: unknown contract ${e.contract}`);
    const region = sites.get(e.site) ?? "";
    const own = loaded.punches.filter((p) => p.employeeId === e.id);
    const shifts = buildShifts(e.id, own, exceptions).filter((s) => inPeriod(s.date));
    const minutes = classify(shifts, contract, (date) => holidays(date, region));
    pay.push({ employeeId: e.id, minutes, grossCents: grossCents(minutes, e.rateCents, contract.nightPremiumPct) });
  }

  const reported = exceptions
    .filter((x) => inPeriod(localDate(x.at)))
    .sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : a.code < b.code ? -1 : a.code > b.code ? 1 : a.ref < b.ref ? -1 : a.ref > b.ref ? 1 : 0));
  return { period: { start, end }, employees: pay, exceptions: reported };
}
