"use strict";
// $toMillis with picture strings: fractional seconds and adjacent numeric components.
const test = require("node:test");
const assert = require("node:assert");
const jsonata = require("../src/jsonata");

async function iso(value, picture) {
    const expr = jsonata("$fromMillis($toMillis(v, p))");
    return expr.evaluate({ v: value, p: picture });
}

const FRAC = "[Y0001]-[M01]-[D01] [H01]:[m01]:[s01].[f001]";

test("six fractional digits are cut to milliseconds", async () => {
    assert.strictEqual(await iso("2026-09-03 08:14:25.019874", FRAC), "2026-09-03T08:14:25.019Z");
});

test("five fractional digits with [f1]", async () => {
    assert.strictEqual(
        await iso("2026-04-08T19:05:04.01987", "[Y0001]-[M01]-[D01]T[H01]:[m01]:[s01].[f1]"),
        "2026-04-08T19:05:04.019Z"
    );
});

test("one fractional digit is tenths", async () => {
    assert.strictEqual(await iso("2026-09-02 14:40:29.6", FRAC), "2026-09-02T14:40:29.600Z");
});

test("two fractional digits are hundredths", async () => {
    assert.strictEqual(await iso("2026-09-02 14:40:29.56", FRAC), "2026-09-02T14:40:29.560Z");
});

test("fraction followed by a time zone offset", async () => {
    assert.strictEqual(
        await iso("2026-09-04T06:15:00.123456+02:00", "[Y0001]-[M01]-[D01]T[H01]:[m01]:[s01].[f001][Z01:01]"),
        "2026-09-04T04:15:00.123Z"
    );
});

test("compact timestamp without separators", async () => {
    assert.strictEqual(await iso("20260904061500", "[Y0001][M01][D01][H01][m01][s01]"), "2026-09-04T06:15:00.000Z");
});

test("compact timestamp with [Y0000] style widths", async () => {
    const r = await jsonata(
        "$toMillis('2024-01-01T12:38:49Z') = $toMillis('20240101123849', '[Y0000][M00][D00][H00][m00][s00]')"
    ).evaluate(null);
    assert.strictEqual(r, true);
});

test("adjacent hour and minute", async () => {
    assert.strictEqual(await iso("0615 04/09/2026", "[H01][m01] [D01]/[M01]/[Y0001]"), "2026-09-04T06:15:00.000Z");
});

test("day, month and year without separators still parse", async () => {
    assert.strictEqual(await iso("04092026", "[D01][M01][Y0001]"), "2026-09-04T00:00:00.000Z");
});
