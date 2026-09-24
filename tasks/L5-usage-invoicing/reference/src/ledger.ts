import { createHmac } from "node:crypto";
import { loadCurrencies } from "./config.ts";
import type { InvoiceRun } from "./types.ts";

const SENDER = "LEDGER_SENDER_ID";
const KEY = "LEDGER_HMAC_KEY";

function major(amount: number, decimals: number): string {
  const neg = amount < 0;
  const digits = String(Math.abs(amount)).padStart(decimals + 1, "0");
  const text = decimals === 0 ? digits : `${digits.slice(0, -decimals)}.${digits.slice(-decimals)}`;
  return neg ? `-${text}` : text;
}

/** The ledger export of a run (RULES.md §10). */
export function renderLedgerExport(run: InvoiceRun, env: Record<string, string | undefined>): string {
  const sender = env[SENDER];
  if (!sender) throw new Error(`${SENDER} is missing`);
  const key = env[KEY];
  if (!key) throw new Error(`${KEY} is missing`);
  const currencies = loadCurrencies();
  let body = `BATCH;${sender};${run.period};${run.invoices.length}\n`;
  for (const inv of run.invoices) {
    const d = currencies[inv.currency].decimals;
    const f = (n: number) => major(n, d);
    body += `INV;${inv.accountId};${inv.currency};${f(inv.subtotal)};${f(inv.creditTotal)};${f(inv.tax)};${f(inv.total)}\n`;
  }
  const sig = createHmac("sha256", key).update(body).digest("hex");
  return `${body}SIG;${sig}\n`;
}
