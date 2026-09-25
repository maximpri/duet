/**
 * Runs the JSONata test suite (test/test-suite) with the Node.js test runner:
 * one test per case, `node --test test/`.
 *
 * Replaces the upstream mocha/chai runner (run-test-suite.js) so the suite
 * runs offline without dependencies; the case format is unchanged
 * (see test/test-suite/TESTSUITE.md).
 */
"use strict";

const test = require("node:test");
const fs = require("fs");
const path = require("path");
const jsonata = require("../src/jsonata");

const suiteDir = path.join(__dirname, "test-suite");

function readJSON(file) {
    try {
        return JSON.parse(fs.readFileSync(file).toString());
    } catch (e) {
        throw new Error("Error reading " + file + ": " + e.message);
    }
}

const datasets = {};
for (const name of fs.readdirSync(path.join(suiteDir, "datasets"))) {
    datasets[name.replace(".json", "")] = readJSON(path.join(suiteDir, "datasets", name));
}

/** Deep equality of JSON-like values; prototypes and non-enumerable properties are ignored. */
function deepEqual(a, b) {
    if (a === b) return true;
    if (typeof a !== typeof b || a === null || b === null || typeof a !== "object") {
        return Number.isNaN(a) && Number.isNaN(b);
    }
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    if (Array.isArray(a)) {
        return a.length === b.length && a.every((x, i) => deepEqual(x, b[i]));
    }
    const ka = Object.keys(a).filter((k) => a[k] !== undefined);
    const kb = Object.keys(b).filter((k) => b[k] !== undefined);
    return ka.length === kb.length && ka.every((k) => Object.prototype.hasOwnProperty.call(b, k) && deepEqual(a[k], b[k]));
}

function show(v) {
    try {
        return JSON.stringify(v);
    } catch (e) {
        return String(v);
    }
}

function resolveDataset(testcase) {
    if ("data" in testcase) return testcase.data;
    if (testcase.dataset === null) return undefined;
    if (Object.prototype.hasOwnProperty.call(datasets, testcase.dataset)) return datasets[testcase.dataset];
    throw new Error("Unable to find dataset " + testcase.dataset);
}

/** Stops a runaway expression, as the upstream runner did (error code U1001). */
function timeboxExpression(expr, timeout, maxDepth) {
    let depth = 0;
    const time = Date.now();
    const check = function () {
        if (maxDepth > 0 && depth > maxDepth) {
            throw { message: "Stack overflow error", stack: new Error().stack, code: "U1001" };
        }
        if (Date.now() - time > timeout) {
            throw { message: "Expression evaluation timeout", stack: new Error().stack, code: "U1001" };
        }
    };
    expr.assign(Symbol.for("jsonata.__evaluate_entry"), function (expr, input, env) {
        if (env.isParallelCall) return;
        depth++;
        check();
    });
    expr.assign(Symbol.for("jsonata.__evaluate_exit"), function (expr, input, env) {
        if (env.isParallelCall) return;
        depth--;
        check();
    });
}

async function runCase(group, testcase) {
    if (testcase["expr-file"]) {
        testcase.expr = fs.readFileSync(path.join(suiteDir, "groups", group, testcase["expr-file"])).toString();
    }
    let expr;
    try {
        expr = jsonata(testcase.expr);
        if ("timelimit" in testcase && "depth" in testcase) {
            timeboxExpression(expr, testcase.timelimit, testcase.depth);
        }
    } catch (e) {
        if (testcase.code) {
            if (e.code !== testcase.code) throw new Error(`expected code ${testcase.code}, got ${e.code}: ${e.message}`);
            if (Object.prototype.hasOwnProperty.call(testcase, "token") && e.token !== testcase.token) {
                throw new Error(`expected token ${testcase.token}, got ${e.token}`);
            }
            return;
        }
        throw new Error("Got an unexpected exception: " + e.message);
    }
    const dataset = resolveDataset(testcase);
    if ("undefinedResult" in testcase) {
        const result = await expr.evaluate(dataset, testcase.bindings);
        if (result !== undefined) throw new Error("expected undefined, got " + show(result));
    } else if ("result" in testcase) {
        const result = await expr.evaluate(dataset, testcase.bindings);
        if (!deepEqual(result, testcase.result)) {
            throw new Error("expected " + show(testcase.result) + ", got " + show(result));
        }
    } else if ("error" in testcase || "code" in testcase) {
        const want = "error" in testcase ? testcase.error : { code: testcase.code };
        let error;
        try {
            await expr.evaluate(dataset, testcase.bindings);
        } catch (e) {
            error = e;
        }
        if (!error) throw new Error("expected an error " + show(want));
        for (const key of Object.keys(want)) {
            if (!deepEqual(error[key], want[key])) {
                throw new Error(`expected error ${key}=${show(want[key])}, got ${show(error[key])} (${error.message})`);
            }
        }
    } else {
        throw new Error("Nothing to test in this test case");
    }
}

const groupsDir = path.join(suiteDir, "groups");
for (const group of fs.readdirSync(groupsDir).filter((n) => !n.endsWith(".json")).sort()) {
    const files = fs.readdirSync(path.join(groupsDir, group)).filter((n) => n.endsWith(".json")).sort();
    for (const name of files) {
        const spec = readJSON(path.join(groupsDir, group, name));
        const cases = Array.isArray(spec) ? spec : [spec];
        cases.forEach((testcase, i) => {
            const label = `${group}/${name}${cases.length > 1 ? "#" + i : ""}`;
            test(label, () => runCase(group, testcase));
        });
    }
}
