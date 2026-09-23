import type { PayrollResult } from "./types.ts";

/** The signed bank transfer file for a payroll result. */
export function renderBankExport(result: PayrollResult, env: Record<string, string | undefined>): string {
  void result;
  void env;
  throw new Error("bank export not implemented");
}
