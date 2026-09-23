import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { parseRecords } from "./csv.ts";
import { loadEmployees } from "./data.ts";
import { parseBerlinLocal, parseOffsetTimestamp, toIsoMinute } from "./time.ts";
import type { PayrollException, Punch, PunchKind } from "./types.ts";

/** Current terminals (JSON lines). */
const EVENTS: Record<string, PunchKind> = {
  clock_in: "IN",
  clock_out: "OUT",
  break_start: "BREAK_START",
  break_end: "BREAK_END",
};

/** Generation-1 terminals (semicolon CSV): Kommen, Gehen, Pausenanfang, Pausenende. */
const CODES: Record<string, PunchKind> = { K: "IN", G: "OUT", PA: "BREAK_START", PE: "BREAK_END" };

const DOUBLE_TAP_MS = 5 * 60_000;

/** Badges are written `B-917`, `b-917 ` or `0917` depending on the system; the number identifies the card. */
export function badgeKey(badge: string): string | null {
  const digits = badge.replace(/\D/g, "");
  return digits === "" ? null : String(Number(digits));
}

interface RawPunch {
  badge: string;
  ms: number;
  kind: PunchKind;
}

function readTerminalFiles(dir: string): RawPunch[] {
  const out: RawPunch[] = [];
  if (!existsSync(dir)) return out;
  for (const name of readdirSync(dir).sort()) {
    const text = readFileSync(join(dir, name), "utf8");
    if (name.endsWith(".jsonl")) {
      for (const line of text.split(/\r?\n/)) {
        if (line.trim() === "") continue;
        const rec = JSON.parse(line);
        const kind = EVENTS[String(rec.event)];
        if (!kind) throw new Error(`${name}: unknown event ${rec.event}`);
        out.push({ badge: String(rec.badge).trim(), ms: parseOffsetTimestamp(String(rec.ts)), kind });
      }
    } else if (name.endsWith(".csv")) {
      for (const r of parseRecords(text, { delimiter: ";", comment: "#" })) {
        const kind = CODES[r.Buchung];
        if (!kind) throw new Error(`${name}: unknown booking code ${r.Buchung}`);
        out.push({ badge: r.Ausweis, ms: parseBerlinLocal(r.Zeitpunkt), kind });
      }
    }
  }
  return out;
}

/**
 * Every punch from `dataDir/punches`, with approved corrections applied and
 * double taps removed, sorted by employee id then time.
 */
export function loadPunches(dataDir: string): { punches: Punch[]; exceptions: PayrollException[] } {
  const byKey = new Map<string, string>();
  for (const e of loadEmployees(dataDir)) {
    const key = badgeKey(e.badge);
    if (key) byKey.set(key, e.id);
  }
  const exceptions: PayrollException[] = [];
  let list: { employeeId: string; ms: number; kind: PunchKind }[] = [];
  for (const raw of readTerminalFiles(join(dataDir, "punches"))) {
    const key = badgeKey(raw.badge);
    const employeeId = key ? byKey.get(key) : undefined;
    if (!employeeId) {
      exceptions.push({ code: "UNKNOWN_BADGE", ref: raw.badge, at: toIsoMinute(raw.ms) });
      continue;
    }
    list.push({ employeeId, ms: raw.ms, kind: raw.kind });
  }

  const correctionsPath = join(dataDir, "corrections.csv");
  if (existsSync(correctionsPath)) {
    for (const c of parseRecords(readFileSync(correctionsPath, "utf8"))) {
      if (c.status.toUpperCase() !== "APPROVED") continue;
      const key = badgeKey(c.badge);
      const employeeId = key ? byKey.get(key) : undefined;
      if (!employeeId) continue;
      const ms = parseBerlinLocal(c.local_time);
      const kind = c.kind as PunchKind;
      if (c.action === "ADD") {
        list.push({ employeeId, ms, kind });
      } else if (c.action === "VOID") {
        const i = list.findIndex((p) => p.employeeId === employeeId && p.ms === ms && p.kind === kind);
        if (i >= 0) list.splice(i, 1);
      }
    }
  }

  list.sort((a, b) => (a.employeeId < b.employeeId ? -1 : a.employeeId > b.employeeId ? 1 : a.ms - b.ms));
  const kept: typeof list = [];
  for (const p of list) {
    const prev = kept[kept.length - 1];
    if (prev && prev.employeeId === p.employeeId && prev.kind === p.kind && p.ms - prev.ms <= DOUBLE_TAP_MS) continue;
    kept.push(p);
  }
  list = kept;
  exceptions.sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));
  return {
    punches: list.map((p) => ({ employeeId: p.employeeId, at: toIsoMinute(p.ms), kind: p.kind })),
    exceptions,
  };
}
