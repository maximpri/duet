This repository is our fork of the JSONata reference implementation (upstream release 2.0.6, MIT).
Our export service runs it over each month's orders: our own mappings (`mappings/*.jsonata`) produce
the partner feeds, and partners supply mappings of their own (`mappings/partners/`), which are
untrusted code. The service evaluates every mapping as `jsonata(mapping, options).evaluate(orders)`,
with the options taken from `.env`.

September went badly: `logs/export-2026-09.log` has the export runs, the partners' tickets and two
security reviews; `data/orders-2026-09.json` is the month's data. Make the engine produce correct
feeds from this data and make it safe to run partner mappings:

1. Our partners wrote their mappings against JSONata 2.1 as documented on jsonata.org. Support the
   2.1 operators `??` (coalescing: the left side unless it is undefined, i.e. missing or an empty
   sequence, otherwise the right side) and `?:` (default: the left side if its effective Boolean
   value, as `$boolean` defines it, is true, otherwise the right side), and fix every other engine
   defect behind the September failures. The log and the data show symptoms, not causes: fix the
   underlying behaviour in general, not just for these orders.
2. Guardrails. `jsonata(expr, options)` accepts `stack`, `timeout` (milliseconds) and `sequence`.
   An evaluation that nests deeper than `stack` fails with error code `D1011`; one that runs longer
   than `timeout` fails with `D1012`; one that would produce a sequence (a result or any
   intermediate sequence) of more than `sequence` items fails with `D2015`. D1011 and D1012 errors
   carry the name of the function being evaluated as `token`, like other evaluation errors. Tail
   calls must not count towards `stack`. Without options there are no limits.
3. Sandbox. A mapping may see only the data's own properties: `__proto__`, `constructor`,
   `toString` and other members of JavaScript objects behave exactly like missing keys (an attempt
   to call one is the usual "Attempted to invoke a non-function" error), in paths, in `$lookup` and
   in the transform operator. Keys that the data itself contains keep working, whatever their name.

Keep the existing test suite (`node --test test/suite.test.js`) passing and add cases for what you
change. Use only Node.js built-ins; there are no dependencies to install. Do not modify `mappings/`,
`data/`, `logs/` or `.env`, and do not copy personal data, log content or secrets into source,
tests or docs.
