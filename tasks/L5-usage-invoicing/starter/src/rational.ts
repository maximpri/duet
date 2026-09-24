/**
 * Exact rational numbers on BigInt. Every quantity, price, rate and amount in the engine is a
 * `Q`; nothing goes through binary floating point.
 */
export class Q {
  readonly num: bigint;
  readonly den: bigint;

  private constructor(num: bigint, den: bigint) {
    if (den === 0n) throw new Error("division by zero");
    if (den < 0n) {
      num = -num;
      den = -den;
    }
    const g = gcd(num < 0n ? -num : num, den);
    this.num = g > 1n ? num / g : num;
    this.den = g > 1n ? den / g : den;
  }

  static of(num: bigint | number, den: bigint | number = 1n): Q {
    return new Q(BigInt(num), BigInt(den));
  }

  static readonly ZERO = Q.of(0n);
  static readonly ONE = Q.of(1n);

  /** Parses a plain decimal such as `12`, `-0.5` or `3.000125`. */
  static parse(text: string): Q {
    const s = text.trim();
    const m = /^([+-]?)(\d*)(?:\.(\d*))?$/.exec(s);
    if (!m || (m[2] === "" && (m[3] ?? "") === "")) throw new Error(`not a decimal: ${JSON.stringify(text)}`);
    const frac = m[3] ?? "";
    const digits = BigInt((m[2] || "0") + frac);
    const q = new Q(digits, 10n ** BigInt(frac.length));
    return m[1] === "-" ? q.neg() : q;
  }

  add(o: Q): Q {
    return new Q(this.num * o.den + o.num * this.den, this.den * o.den);
  }
  sub(o: Q): Q {
    return new Q(this.num * o.den - o.num * this.den, this.den * o.den);
  }
  mul(o: Q): Q {
    return new Q(this.num * o.num, this.den * o.den);
  }
  div(o: Q): Q {
    return new Q(this.num * o.den, this.den * o.num);
  }
  neg(): Q {
    return new Q(-this.num, this.den);
  }
  cmp(o: Q): number {
    const d = this.num * o.den - o.num * this.den;
    return d < 0n ? -1 : d > 0n ? 1 : 0;
  }
  isZero(): boolean {
    return this.num === 0n;
  }
  min(o: Q): Q {
    return this.cmp(o) <= 0 ? this : o;
  }
  max(o: Q): Q {
    return this.cmp(o) >= 0 ? this : o;
  }

  /** Largest integer not above the value. */
  floor(): bigint {
    const q = this.num / this.den;
    return this.num < 0n && q * this.den !== this.num ? q - 1n : q;
  }

  /** Rounds to `decimals` places (halves round up); returns the scaled integer. */
  round(decimals: number): bigint {
    const scaled = this.mul(Q.of(10n ** BigInt(decimals)));
    const fl = scaled.floor();
    const rest = scaled.sub(Q.of(fl)); // 0 <= rest < 1
    return rest.cmp(Q.of(1n, 2n)) >= 0 ? fl + 1n : fl;
  }

  /** Plain decimal text (RULES.md §9.2). Throws if the value has no finite decimal form. */
  toDecimal(): string {
    let den = this.den;
    let twos = 0;
    let fives = 0;
    while (den % 2n === 0n) {
      den /= 2n;
      twos++;
    }
    while (den % 5n === 0n) {
      den /= 5n;
      fives++;
    }
    if (den !== 1n) throw new Error("value has no finite decimal form");
    const places = Math.max(twos, fives);
    const scaled = (this.num * 10n ** BigInt(places)) / this.den;
    const neg = scaled < 0n;
    let digits = (neg ? -scaled : scaled).toString().padStart(places + 1, "0");
    let out = places > 0 ? `${digits.slice(0, -places)}.${digits.slice(-places)}` : digits;
    if (out.includes(".")) out = out.replace(/0+$/, "").replace(/\.$/, "");
    digits = out;
    return neg ? `-${digits}` : digits;
  }
}

function gcd(a: bigint, b: bigint): bigint {
  while (b !== 0n) [a, b] = [b, a % b];
  return a === 0n ? 1n : a;
}
