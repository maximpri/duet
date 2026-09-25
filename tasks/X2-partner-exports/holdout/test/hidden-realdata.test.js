"use strict";
// The September orders run through our mappings and the partner mappings, with
// the guardrail settings of the export service (.env). Each evaluation runs in
// a worker thread that is stopped after 20 s.
const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const path = require("node:path");
const { Worker } = require("node:worker_threads");

const ROOT = path.resolve(__dirname, "..");
const LIB = path.join(ROOT, "src/jsonata.js");
const DATA = JSON.parse(fs.readFileSync(path.join(ROOT, "data/orders-2026-09.json"), "utf8"));

function envOptions() {
    const env = {};
    for (const line of fs.readFileSync(path.join(ROOT, ".env"), "utf8").split("\n")) {
        const m = /^([A-Z_]+)=(.*)$/.exec(line.trim());
        if (m) env[m[1]] = m[2];
    }
    return {
        stack: Number(env.MAPPING_STACK_LIMIT),
        timeout: Number(env.MAPPING_TIMEOUT_MS),
        sequence: Number(env.MAPPING_SEQUENCE_LIMIT),
    };
}

function run(mapping) {
    const expr = fs.readFileSync(path.join(ROOT, "mappings", mapping + ".jsonata"), "utf8");
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
            {
                eval: true,
                workerData: { lib: LIB, expr, options: envOptions(), data: DATA },
                resourceLimits: { maxOldGenerationSizeMb: 768 },
            }
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

function expected(name) {
    return JSON.parse(fs.readFileSync(path.join(__dirname, "realdata", name + ".json"), "utf8"));
}

async function assertOutput(mapping, name) {
    const outcome = await run(mapping);
    assert.strictEqual(outcome.ok, true, `${mapping}: ${outcome.code} ${outcome.message}`);
    const want = expected(name);
    assert.strictEqual(outcome.result.length, want.length, "number of records");
    want.forEach((record, i) => assert.deepStrictEqual(outcome.result[i], record, `record ${i}`));
}

async function assertRejected(mapping, code) {
    const outcome = await run(mapping);
    assert.strictEqual(outcome.ok, false, `${mapping} was not stopped`);
    assert.strictEqual(outcome.code, code, outcome.message);
}

test("Ledgerly accounting export", () => assertOutput("accounting", "accounting"));
test("Novapay reconciliation feed", () => assertOutput("psp-reconcile", "psp-reconcile"));
test("carrier label feed", () => assertOutput("carrier-labels", "carrier-labels"));
test("QuickShip manifest contains only order data", () => assertOutput("partners/quickship", "quickship"));
test("Tracepoint cannot call JavaScript methods", () => assertRejected("partners/tracepoint", "T1006"));
test("Northwind is stopped by the stack limit", () => assertRejected("partners/northwind-analytics", "D1011"));
test("Orbit is stopped by the timeout", () => assertRejected("partners/orbit-bi", "D1012"));
test("Bluefin is stopped by the sequence limit", () => assertRejected("partners/bluefin", "D2015"));
