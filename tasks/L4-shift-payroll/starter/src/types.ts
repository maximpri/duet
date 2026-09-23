export type PunchKind = "IN" | "OUT" | "BREAK_START" | "BREAK_END";

export interface Punch {
  employeeId: string;
  /** UTC instant, truncated to the minute: `YYYY-MM-DDTHH:MM:00Z`. */
  at: string;
  kind: PunchKind;
}

export type ExceptionCode = "UNKNOWN_BADGE" | "MISSING_OUT" | "ORPHAN_OUT" | "ORPHAN_BREAK";

export interface PayrollException {
  code: ExceptionCode;
  /** The employee id, or for UNKNOWN_BADGE the badge text as recorded (trimmed). */
  ref: string;
  /** UTC instant of the punch concerned, same format as `Punch.at`. */
  at: string;
}

export interface Minutes {
  regular: number;
  ot1: number;
  ot2: number;
  holiday: number;
  night: number;
}

export interface EmployeePay {
  employeeId: string;
  minutes: Minutes;
  grossCents: number;
}

export interface PayrollResult {
  period: { start: string; end: string };
  employees: EmployeePay[];
  exceptions: PayrollException[];
}

export interface Employee {
  id: string;
  badge: string;
  name: string;
  site: string;
  contract: string;
  rateCents: number;
}
