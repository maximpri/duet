// Usage: node src/cli.ts <dataDir> <YYYY-MM> [--ledger <file>]
import { writeFileSync } from "node:fs";
import { loadCurrencies } from "./config.ts";
import { loadEnv } from "./env.ts";
import { buildInvoices } from "./invoice.ts";
import { renderLedgerExport } from "./ledger.ts";

const [dataDir, period, flag, ledgerFile] = process.argv.slice(2);
if (!dataDir || !/^\d{4}-\d{2}$/.test(period ?? "")) {
  console.error("usage: node src/cli.ts <dataDir> <YYYY-MM> [--ledger <file>]");
  process.exit(2);
}
const run = buildInvoices(dataDir, period);
const currencies = loadCurrencies();
const money = (n: number, cur: string) => {
  const d = currencies[cur]?.decimals ?? 2;
  return (n / 10 ** d).toFixed(d);
};
for (const inv of run.invoices) {
  console.log(`invoice ${inv.accountId} ${inv.period} ${inv.currency} tax=${inv.taxRule}/${inv.taxRatePct}%`);
  for (const l of inv.lines) {
    console.log(`  ${l.kind.padEnd(5)} ${l.plan.padEnd(8)} ${(l.meter ?? "-").padEnd(14)} ${l.quantity.padStart(22)} ${money(l.amount, inv.currency).padStart(12)}`);
  }
  for (const c of inv.credits) console.log(`  credit ${c.creditId} -${money(c.amount, inv.currency)}`);
  console.log(
    `  subtotal ${money(inv.subtotal, inv.currency)} credits ${money(inv.creditTotal, inv.currency)} tax ${money(inv.tax, inv.currency)} total ${money(inv.total, inv.currency)}`,
  );
}
for (const x of run.exceptions) console.log(`exception ${x.code} ${x.ref} x${x.count}`);
if (flag === "--ledger" && ledgerFile) {
  writeFileSync(ledgerFile, renderLedgerExport(run, loadEnv()));
  console.log(`ledger export written to ${ledgerFile}`);
}
