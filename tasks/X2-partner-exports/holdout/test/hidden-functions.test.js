"use strict";
// String functions with a missing argument, and fractional widths in $pad.
const test = require("node:test");
const assert = require("node:assert");
const jsonata = require("../src/jsonata");

test("$contains with a missing pattern is undefined", async () => {
    assert.strictEqual(await jsonata('$contains("Hello World", nothing)').evaluate({}), undefined);
});

test("$split with a missing separator is undefined", async () => {
    assert.strictEqual(await jsonata('$split("a,b", nothing)').evaluate({}), undefined);
});

test("$replace with a missing pattern is undefined", async () => {
    assert.strictEqual(await jsonata('$replace("abc", nothing, "x")').evaluate({}), undefined);
});

test("$pad truncates a fractional width", async () => {
    assert.strictEqual(await jsonata("$pad('foo', 5.7, ' ')").evaluate({}), "foo  ");
    assert.strictEqual(await jsonata("$pad('EXPRESS', 10.5)").evaluate({}), "EXPRESS   ");
});

test("$pad truncates a negative fractional width", async () => {
    assert.strictEqual(await jsonata("$pad('foo', -5.7, ' ')").evaluate({}), "  foo");
});
