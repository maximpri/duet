"use strict";
// JSONata 2.1 operators: `??` (coalescing) and `?:` (default).
const test = require("node:test");
const assert = require("node:assert");
const jsonata = require("../src/jsonata");

const data = {
    foo: { bar: 42, blah: [{ baz: { fud: "hello" } }, { baz: { fud: "world" } }] },
    bar: 98,
    Account: { Name: "Firefly" },
};

function plain(v) {
    return v === undefined ? undefined : JSON.parse(JSON.stringify(v));
}

const cases = [
    ["bar ?? 42", 98],
    ["foo.blah[9].baz.fud ?? 42", 42],
    ["null ?? 42", null],
    ["0 ?? 42", 0],
    ["foo.blah[0] ?? 42", { baz: { fud: "hello" } }],
    ["foo.blah[5] ?? 42", 42],
    ["false ?: 42", 42],
    ["[0] ?: 42", 42],
    ["function(){true} ?: 42", 42],
    ["foo.blah[5] ?: 42", 42],
    ["[1,2,3][-1] ?: 42", 3],
    ["-5 ?: 99", -5],
];

for (const [expr, expected] of cases) {
    test(expr, async () => {
        const result = await jsonata(expr).evaluate(data);
        assert.deepStrictEqual(plain(result), expected);
    });
}
