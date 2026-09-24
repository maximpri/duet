// Public types of the invoicing engine. See docs/RULES.md for the rules behind every field.

export interface Account {
  id: string;
  /** Empty for a root account. */
  parentId: string;
  country: string;
  vatId: string;
  currency: string;
  timezone: string;
}

export interface UsageEvent {
  /** Event id, lower case. */
  id: string;
  /** Canonical account id (as in accounts.csv) of the account that produced the event. */
  accountId: string;
  meter: string;
  /** Exact quantity in the meter's canonical unit, written as in RULES.md §9.2. */
  quantity: string;
  /** UTC instant, `Date#toISOString()` format. */
  at: string;
}

export type ExceptionCode = "UNKNOWN_ACCOUNT" | "UNKNOWN_METER";

export interface UsageException {
  code: ExceptionCode;
  ref: string;
  count: number;
}

export interface UsageLoad {
  /** Sorted by `at`, then `id`. */
  events: UsageEvent[];
  /** Sorted by code, then ref. */
  exceptions: UsageException[];
}

export interface InvoiceLine {
  kind: "fee" | "usage";
  plan: string;
  /** Meter code for usage lines, null for fee lines. */
  meter: string | null;
  /** Fee lines: segment length in seconds. Usage lines: billable quantity. */
  quantity: string;
  /** Integer amount in minor units of the invoice currency. */
  amount: number;
}

export interface AppliedCredit {
  creditId: string;
  /** Minor units of the invoice currency. */
  amount: number;
}

export type TaxRule = "domestic" | "reverse_charge" | "oss" | "export";

export interface Invoice {
  accountId: string;
  /** `YYYY-MM`. */
  period: string;
  currency: string;
  lines: InvoiceLine[];
  credits: AppliedCredit[];
  taxRule: TaxRule;
  /** Percentage as written in config/tax.json, "0" when no tax applies. */
  taxRatePct: string;
  /** All amounts below in minor units of the invoice currency. */
  subtotal: number;
  creditTotal: number;
  tax: number;
  total: number;
}

export interface InvoiceRun {
  period: string;
  invoices: Invoice[];
  exceptions: UsageException[];
}
