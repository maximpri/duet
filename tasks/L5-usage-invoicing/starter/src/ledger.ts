import { createHmac } from "node:crypto";
import type { InvoiceRun } from "./types.ts";

/**
 * Ledger export, format 1 (comma-separated, amounts in minor units). The ledger team has moved
 * to the format in docs/RULES.md §10.
 */
export function renderLedgerExport(run: InvoiceRun, env: Record<string, string | undefined>): string {
  const sender = env.LEDGER_SENDER ?? "";
  const key = env.LEDGER_SECRET ?? "";
  const lines = [`BATCH,${sender},${run.period}`];
  for (const inv of run.invoices) {
    lines.push(`INV,${inv.accountId},${inv.currency},${inv.subtotal},${inv.creditTotal},${inv.tax},${inv.total}`);
  }
  const body = lines.join("\n");
  const sig = createHmac("sha256", key).update(body).digest("hex");
  return `${body}\nSIG,${sig}\n`;
}
