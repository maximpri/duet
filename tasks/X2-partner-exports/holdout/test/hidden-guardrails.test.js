"use strict";
// Resource guardrails: jsonata(expr, { stack, timeout, sequence }).
// Each evaluation runs in a worker thread that is stopped after 20 s.
const test = require("node:test");
const assert = require("node:assert");
const path = require("node:path");
const { Worker } = require("node:worker_threads");

const LIB = path.resolve(__dirname, "../src/jsonata.js");

function evaluate(expr, options, data) {
    return new Promise((resolve) => {
        const worker = new Worker(
            `const { parentPort, workerData } = require("node:worker_threads");
            const jsonata = require(workerData.lib);
            (async () => {
                try {
                    const r = await jsonata(workerData.expr, workerData.options).evaluate(workerData.data);
                    parentPort.postMessage({ ok: true, result: r === undefined ? null : JSON.parse(JSON.stringify(r)) });
                } catch (e) {
                    parentPort.postMessage({ ok: false, code: e && e.code, token: e && e.token, message: e && e.message });
                }
            })();`,
            { eval: true, workerData: { lib: LIB, expr, options, data }, resourceLimits: { maxOldGenerationSizeMb: 768 } }
        );
        const timer = setTimeout(() => {
            worker.terminate();
            resolve({ ok: false, code: "KILLED after 20 s" });
        }, 20000);
        worker.on("message", (m) => {
            clearTimeout(timer);
            worker.terminate();
            resolve(m);
        });
        worker.on("error", (e) => {
            clearTimeout(timer);
            resolve({ ok: false, code: "WORKER ERROR", message: String(e) });
        });
    });
}

function assertError(outcome, code, token) {
    assert.strictEqual(outcome.ok, false, "expected an error, got " + JSON.stringify(outcome.result));
    assert.strictEqual(outcome.code, code, outcome.message);
    if (token !== undefined) assert.strictEqual(outcome.token, token);
}

const ackermann = (m, n) => `(
    $ack := function($m, $n) {
        $m = 0 ? $n + 1 :
        $n = 0 ? $ack($m - 1, 1) :
        $ack($m - 1, $ack($m, $n - 1))
    };
    $ack(${m}, ${n})
)`;

test("stack: non-tail recursion stops with D1011", async () => {
    assertError(await evaluate("($inf := function($n){$n+$inf($n-1)};  $inf(5))", { timeout: 1000, stack: 300 }), "D1011", "inf");
});

test("timeout: tail recursion stops with D1012", async () => {
    assertError(await evaluate("( $inf := function(){$inf()}; $inf())", { timeout: 1000, stack: 500 }), "D1012", "inf");
});

test("timeout alone stops an infinite loop", async () => {
    assertError(await evaluate("( $inf := function(){$inf()}; $inf())", { timeout: 1000 }), "D1012", "inf");
});

test("limits leave a bounded computation alone", async () => {
    const outcome = await evaluate(ackermann(3, 4), { timeout: 1000, stack: 500 });
    assert.deepStrictEqual(outcome, { ok: true, result: 125 });
});

test("stack: deep recursion stops with D1011", async () => {
    assertError(await evaluate(ackermann(4, 4), { stack: 500 }), "D1011", "ack");
});

test("sequence: a range longer than the limit stops with D2015", async () => {
    assertError(await evaluate("[0..1001]", { sequence: 1000 }), "D2015");
});

test("sequence: intermediate sequences count", async () => {
    assertError(await evaluate("[0..100].([0..100]) ~> $count()", { sequence: 1000 }), "D2015");
});

test("sequence: a sequence of exactly the limit is allowed", async () => {
    const outcome = await evaluate("$count([1..1000])", { sequence: 1000 });
    assert.deepStrictEqual(outcome, { ok: true, result: 1000 });
});

test("sequence: functions that build sequences count", async () => {
    assertError(await evaluate("$append([1..600], [1..600])", { sequence: 1000 }), "D2015");
});

test("without options there are no limits", async () => {
    const outcome = await evaluate("($f := function($n){ $n = 0 ? 0 : $n + $f($n - 1) }; $f(400))", undefined);
    assert.deepStrictEqual(outcome, { ok: true, result: 80200 });
});
