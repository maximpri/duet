// Usage: node src/cli.ts <dataDir> <start YYYY-MM-DD> <end YYYY-MM-DD> [--bank <file>]
import { writeFileSync } from "node:fs";
import { renderBankExport } from "./bank.ts";
import { loadEnv } from "./env.ts";
import { computePayroll } from "./payroll.ts";

const [dataDir, start, end, flag, bankFile] = process.argv.slice(2);
if (!dataDir || !start || !end) {
  console.error("usage: node src/cli.ts <dataDir> <start> <end> [--bank <file>]");
  process.exit(2);
}
const result = computePayroll(dataDir, start, end);
const fmt = (n: number) => String(n).padStart(6);
console.log("employee   regular    ot1    ot2 holiday  night      gross");
for (const e of result.employees) {
  const m = e.minutes;
  console.log(
    `${e.employeeId.padEnd(8)} ${fmt(m.regular)} ${fmt(m.ot1)} ${fmt(m.ot2)} ${fmt(m.holiday)} ${fmt(m.night)} ${(e.grossCents / 100).toFixed(2).padStart(10)}`,
  );
}
for (const x of result.exceptions) console.log(`exception ${x.code} ${x.ref} ${x.at}`);
if (flag === "--bank" && bankFile) {
  writeFileSync(bankFile, renderBankExport(result, loadEnv()));
  console.log(`bank file written to ${bankFile}`);
}
