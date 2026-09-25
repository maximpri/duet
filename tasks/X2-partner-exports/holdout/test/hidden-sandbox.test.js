"use strict";
// A mapping sees only the data's own properties; JavaScript internals are unreachable.
const test = require("node:test");
const assert = require("node:assert");
const jsonata = require("../src/jsonata");

const data = { foo: { bar: "baz" } };

test("__proto__ is absent", async () => {
    assert.strictEqual(await jsonata("foo.__proto__").evaluate(data), undefined);
});

test("constructor is absent", async () => {
    assert.strictEqual(await jsonata("foo.constructor").evaluate(data), undefined);
});

test("methods of the JavaScript object cannot be invoked", async () => {
    await assert.rejects(jsonata("foo.toString()").evaluate(data), (e) => e.code === "T1006");
});

test("$lookup ignores inherited members", async () => {
    assert.strictEqual(await jsonata('$lookup(foo, "__proto__")').evaluate(data), undefined);
    assert.strictEqual(await jsonata('$lookup(foo, "hasOwnProperty")').evaluate(data), undefined);
});

test("the transform operator cannot reach the prototype", async () => {
    const result = await jsonata('{} ~> | __proto__ | {"is_admin": true} |').evaluate(data);
    assert.deepStrictEqual(JSON.parse(JSON.stringify(result)), {});
    assert.strictEqual({}.is_admin, undefined);
});

test("own keys with those names are still data", async () => {
    const own = JSON.parse('{"item": {"constructor": "c", "toString": "t", "valueOf": "v"}}');
    assert.strictEqual(await jsonata("item.constructor & item.toString & $lookup(item, 'valueOf')").evaluate(own), "ctv");
});
