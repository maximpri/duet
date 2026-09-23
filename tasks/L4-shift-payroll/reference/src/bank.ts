import { createHmac } from "node:crypto";
import type { PayrollResult } from "./types.ts";

function required(env: Record<string, string | undefined>, name: string): string {
  const value = env[name];
  if (value === undefined || value.trim() === "") throw new Error(`missing ${name}`);
  return value.trim();
}

/** The signed bank transfer file for a payroll result. */
export function renderBankExport(result: PayrollResult, env: Record<string, string | undefined>): string {
  const sender = required(env, "PAYROLL_SENDER_ID");
  const key = required(env, "PAYROLL_SIGNING_KEY");
  const lines = [`SENDER;${sender};${result.period.start};${result.period.end}`];
  for (const e of [...result.employees].sort((a, b) => (a.employeeId < b.employeeId ? -1 : 1))) {
    if (e.grossCents <= 0) continue;
    lines.push(`${e.employeeId};${Math.floor(e.grossCents / 100)}.${String(e.grossCents % 100).padStart(2, "0")}`);
  }
  const body = lines.map((l) => `${l}\n`).join("");
  const signature = createHmac("sha256", key).update(body).digest("hex");
  return `${body}SIGNATURE;${signature}\n`;
}
