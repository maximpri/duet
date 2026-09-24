import type { Meter } from "./config.ts";
import { Q } from "./rational.ts";

/** Exact EUR price of `quantity` billable units of `meter`: each part at its tier's price. */
export function priceUsage(meter: Meter, quantity: Q): Q {
  let total = Q.ZERO;
  let below = Q.ZERO;
  for (const t of meter.tiers) {
    if (quantity.cmp(below) <= 0) break;
    const top = t.upTo === null ? quantity : quantity.min(t.upTo);
    total = total.add(top.sub(below).mul(t.unitPrice));
    if (t.upTo === null) break;
    below = t.upTo;
  }
  return total;
}
