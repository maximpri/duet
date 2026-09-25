"use strict";
// Integers keep every digit when converted to strings.
const test = require("node:test");
const assert = require("node:assert");
const jsonata = require("../src/jsonata");

const data = { big_id: 5890840712243076, ref: 4817203344556677 };

test("$string of a 16-digit integer", async () => {
    assert.strictEqual(await jsonata("$string(ref)").evaluate(data), "4817203344556677");
    assert.strictEqual(await jsonata("$string(big_id)").evaluate(data), "5890840712243076");
});

test("string concatenation with a 16-digit integer", async () => {
    assert.strictEqual(await jsonata('"ORD-" & ref').evaluate(data), "ORD-4817203344556677");
});

test("largest safe integer", async () => {
    assert.strictEqual(await jsonata("$string(9007199254740991)").evaluate(data), "9007199254740991");
});

test("integers inside objects keep their digits, fractions are still tidied", async () => {
    assert.strictEqual(
        await jsonata('$string({"a": ref, "b": 0.1 + 0.2})').evaluate(data),
        '{"a":4817203344556677,"b":0.3}'
    );
});

test("$formatBase of a 16-digit integer", async () => {
    assert.strictEqual(await jsonata("$formatBase(big_id)").evaluate(data), "5890840712243076");
});
