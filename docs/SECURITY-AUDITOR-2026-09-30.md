# Security review: auditor and repository scanner, 2026-09-30

Revision 3 implements both M5.3 entry points: opt-in change review at `finish` and explicit
`duet scan` of existing code. It adds bounded contextual evidence, optional offline scanners and
fresh-context frontier opinions. **This is experimental candidate finding, not a security
certificate.** Finish review, blocking and frontier opinions remain off by default. Model
judgments never suppress findings or control enforcement.

## Revision 3 behavior

See [Usage](USAGE.md#security-review-and-repository-scans) for commands and owner configuration.
`duet scan --rules-only` uses no models or language servers. `--background` starts a detached
worker; `--status <id>` returns its state, starting commit/dirty state and report location.
`--fail-on-high` exits 2 only for narrowly rule-confirmed high findings. Both entry points use
the same rules, private snapshots, output filters and coverage limits. Scans work outside git,
where the commit is null. Reports are per invocation and label the captured revision; they are
not a continuously updated index. A hard-killed worker can leave a stale running status.

The rule set now also nominates dynamic code evaluation, unsafe deserialization, input-derived
paths, HTML and redirects, weak hashes/crypto, predictable randomness, XML parsing, credential
literals, new static outbound hosts and removed recognized authorization calls/decorators.
Host inventory is repository-wide, so moving an already-used host into another file is not a
new-host finding. Auth removal includes deletion of a file. These are original, narrow syntax
heuristics and remain advisory: names alone cannot prove receiver types, reachability or a
security boundary. Only the documented unshadowed Python `requests` TLS setting is confirmed.
Rust raw-string/numeric private literals and decorated Python function context are included.
Comprehension bindings and match captures also make a Python receiver ambiguous; these calls
remain advisory rather than becoming rule-confirmed TLS blockers.

Imports and up to four same-file helper declarations share a 12,000-byte context allowance.
Existing sandboxed language servers can add definitions/references from up to two call sites
and four files; snippets come from captured source, never arbitrary server-supplied text.
Sensitive/protected documents are not sent to language servers. Missing references do not imply
a safe path, and this remains bounded contextual evidence rather than interprocedural proof.
Cancellation is checked during reference requests and between opinions/scanners.
Location snippets select the requested function itself, including its decorators. When a
function exceeds the snippet allowance, the window retains the requested line.

Optional owner-installed scanners accept SARIF, Bandit, gosec, cargo-audit, npm-audit and
pip-audit JSON. No third-party rules are bundled. At most four tools run in separate read-only
snapshot sandboxes with no network, no original-workspace/credential-store reads and no inherited
secret environment. Each tool gets a 1–300-second timeout and a 256-KiB output cap. Offline
resources must already be available. Invalid/truncated output, out-of-snapshot locations and
candidate overflow make that tool unavailable. Raw bounded JSON is stored privately under
`security-scanners/`; only validated locations and advisory severity enter candidates. Raw tool
prose never becomes an instruction, model prompt or report. Before/after multisets suppress
existing findings for finish review; repository scanning uses an empty baseline.
New cross-file findings are retained even when the reported sink file is unchanged, and its
digest is included in the report. Scanner exits other than 0 or 1 are unavailable; exit 1
requires validated findings. Explicit analysis errors, failed SARIF invocations, and skipped
dependency analysis also make coverage incomplete. Valid JSON from a failed tool remains in
the private evidence directory.

The local judgment contract now requires verdict, reason, severity, explanation and fix.
Inconsistent verdict/reason pairs become uncertain. A host-classified private literal cannot
be dismissed as public: `not_sensitive` becomes uncertain. This checks structured fields,
not arbitrary natural-language consistency. Local review defaults to protected code and privacy
flows (`review.local_open = false`), following the measured lack of benefit on open code.
Pass-through and top-clearance finish auditing have no separate local reader and report rules
plus configured scanner inputs; a manual scan can use the configured trusted local reader.

Optional frontier opinions have a fresh gated provider, no working conversation and no tools.
The host selects high or locally uncertain open-code candidates, then the boundary independently
checks paths, every supplied context/diff, known private values and privacy/credential patterns.
Whole consulted sources are checked before context clipping. Protected sources, privacy flows,
credential and external-scanner candidates are ineligible. Top clearance disables this role.
Defaults are four calls per attempt, a $0.50 reviewer budget and the remaining run budget.
In sessions, the remaining allowance respects both the current turn and session ceilings;
earlier turns' spending does not consume the next turn's own allowance.
Each call reserves bounded input plus 4,096 output tokens before sending, has a 120-second
timeout, and charges actual usage or conservative failed-call estimates. Unknown model prices
disable this role. Usage survives transcript replay without polluting the working conversation.

GLM uses JSON-object response mode plus an explicit schema in the system prompt; GLM-5.3 uses
low reasoning effort. This follows the provider's documented JSON mode and mandatory reasoning
behavior, not a rule change made to improve benchmark labels.
See [Z.ai's API reference](https://docs.z.ai/api-reference/llm/chat-completion).
Other providers retain JSON-schema mode. Invalid or length-truncated replies are unavailable;
they cannot hide a rule candidate. Both local and frontier prose pass the existing value,
copied-span and protected-source filters before feedback reaches the working agent.

Shared limits remain 20,000 directory entries, 16 MiB per snapshot, 256 KiB per UTF-8 file,
64 stored findings, bounded context/expansion and a 32,000-byte feedback allowance. Ignored files
are considered; symlinks, dependency/build directories and reserved state are excluded.
Unsupported files, caps, read/parse errors and unavailable configured services are reported.
Changed captured content or a changed commit invalidates completion. Excluded content remains
outside that guarantee. Snapshots, digests, reports, raw scanner evidence and status are private;
audit events contain fixed rule IDs, counts and outcomes without source or reviewer prose.
The report limit does not disable enforcement of detected rule-confirmed high findings.
Such findings replace advisory entries when the report is full; coverage remains incomplete
when anything is omitted. The separate per-file syntax and snapshot bounds still apply.

## Revision 3 measurements

These frozen measurements precede the six code-review fixes recorded in the hand-off.
The follow-up uses regression tests; the model benchmarks have not been rerun after those fixes.

Artifacts are in `/Volumes/EXT_DISK/duet_v2/experiments/security-review-v3/`. Every corpus is read
statically by Rust examples; no reviewed source is executed or bundled. The public checkout
remains OWASP BenchmarkPython `f1291485808b66e20ddb6b01b10dc71b3df8c8ba`.

`independent-manifest.json` froze 48 cases before selected source inspection: four cases per
expected label in each of code injection, deserialization, weak hash, path traversal, XSS and
XXE, selected by a fixed salted SHA-256 ordering. Per-source hashes are checked before each run.
Those cases did not inform rule changes. Prompts omit labels, filenames and benchmark function
names. The comparison reviews every nominated candidate, including medium severity, whereas
production frontier selection is narrower. Models cannot recover rule misses.

The initial matrix had 11 TP / 5 FP / 13 FN / 19 TN in the rules and local lanes (68.75%
precision, 45.83% recall). All 16 local opinions retained their candidates. All 16 frontier
requests failed JSON validation, so their retained counts are **not a valid accuracy comparison**.
They consumed 575.1 seconds and $0.01467821. The 1,600-token allowance was mostly consumed by
reasoning; a low-effort probe still failed until the provider's JSON mode and explicit schema
were used. The failed matrix and both probes are preserved. An accidentally started low-effort
matrix was interrupted before its first row and is not a scored run. The final matrix uses
the corrected generic protocol with unchanged rules and frozen cases.

The 24 authored development privacy cases now score rules 12 TP / 4 FP / 0 FN / 8 TN, and
local-retained candidates 12 / 0 / 0 / 12. All 16 opinions were available: 121.2 seconds of
local time, 8,524 input and 1,948 output tokens. The host fact/reason check prevents the earlier
private-literal dismissal from becoming a safe verdict. Privacy cases never go to the frontier,
so the four conceptual lanes are rules, local, rules, local; no frontier benefit is claimed.

Four authored Rust cases were also appended independently to X1's actual CLI starter source
(base SHA-256 `e6f5cba0489d148f89245729380efcc3534193ec86d3f6e7e7a1e8332d347d68`) and scored as
before/after additions. Rules scored 2 / 1 / 0 / 1; local retention scored 2 / 0 / 0 / 2 in
19.6 seconds over three opinions. This is a small seeded dogfood smoke check, not a new holdout.
Both privacy runs found zero exact checked canaries in filtered reports and forced echoes.
That does not measure all possible fragments or paraphrases; the revision-2 limitation below
still applies. Production findings remain visible regardless of either model's verdict.

The three ordinary X1 diffs again produced zero candidates in 7/10/11 changed supported files
within the cap, with zero incomplete parses. Debug runs took 31.2/45.0/39.8 seconds under other
build/measurement activity, so these are not isolated performance comparisons. This small,
mostly Rust sample cannot establish a general false-positive rate.

The final independent comparison completed in 276.2 seconds:

| Lane (48 cases) | TP | FP | FN | TN | Precision | Recall |
|---|---:|---:|---:|---:|---:|---:|
| Rules | 11 | 5 | 13 | 19 | 68.75% | 45.83% |
| Rules + local retention | 11 | 5 | 13 | 19 | 68.75% | 45.83% |
| Rules + frontier retention | 11 | 3 | 13 | 21 | 78.57% | 45.83% |
| Both reviewers must dismiss | 11 | 5 | 13 | 19 | 68.75% | 45.83% |

All 16 local judgments were available and retained their candidates: 148.5 seconds, 12,984
input and 3,124 output tokens. Frontier review rejected two false positives, retained the other
candidates and returned two schema-invalid opinions, which conservatively retain candidates.
It used 126.1 seconds, 12,033 uncached input + 5,760 cached input tokens, 3,749 output tokens
(986 reasoning), and $0.00385225 at the checked-in list prices. Invalid replies are included in
cost. The table's model lanes are diagnostic retention policies only; the product keeps all
16 findings. Requiring both models to dismiss a finding gives no benefit because the local
model retained everything. This single small sample supports neither a broad precision estimate
nor any improvement in recall.

Final delivery verification passed: `tools/gate.sh` (format, workspace clippy/tests, dependency
policy, license headers, privacy/egress construction and provenance), the optimized `duet` and
`duet-eval` builds, and a release CLI scan detecting disabled TLS verification with exit code 2.
New regressions cover scanner confinement/raw evidence, foreground/background metadata, pinned
LSP context, fresh gated requests, privacy exclusions, budgeting across resume, transcript usage,
judgment reconciliation, attachment delivery/resume and Tab/Shift-Tab panel navigation.
Logs: `/Volumes/EXT_DISK/duet_v2/review-v3-delivery-{gate,release}.log`. The earlier failed gates
are retained. Final artifacts include source archive/digests, the measured matrix executable,
result digests and release hashes. Both shell binaries were installed at `target/release/`;
backups are under `EXT_DISK/duet_v2/releases/review-v3-before-install/`. All three opt-in settings
(`review.enabled`, `review.block_high`, `review.frontier`) still read false. Changes are uncommitted.
M5.3 is complete as the bounded experimental implementation described here; broader detection
quality and publication acceptance remain separate work.

Revision 3's syntax-only untrusted-source recognition also changes the older 36-case development
set: rules score 16 TP / 3 FP / 2 FN / 15 TN (84.21% precision, 88.89% recall), versus revision
2's 18/5/0/13. Two true and two false candidates used request wrappers. The old text search
matched `request` inside a wrapper's name; the syntax rule no longer treats that name alone as
a source. Bare request objects passed through custom wrappers remain a coverage gap. No rule
was tuned using the independent case sources or labels. These changed development counts must
not be presented as preserving revision 2's recall.
All 19 local opinions in that development rerun retained their candidates, including the three
false positives: 174.3 seconds, 15,514 input and 3,437 output tokens, with no malformed retries.

Reproduction (with the external build/tmp environment used below):

```sh
cargo run -p duet-boundary --example review_matrix -- freeze CORPUS MANIFEST
cargo run -p duet-boundary --example review_matrix -- run CORPUS MANIFEST OUTPUT OWNER_CONFIG
cargo run -p duet-boundary --example review_privacy -- OUTPUT OWNER_CONFIG
cargo run -p duet-boundary --example review_privacy -- \
  OUTPUT OWNER_CONFIG tasks/X1-sql-gateway/starter/examples/cli.rs
cargo run -p duet-boundary --example review_diff -- BEFORE AFTER [AFTER...]
```

The configured local alias is `omlx-coding`, not a pinned weight digest; the frontier is
`glm-5.3-flash`. Calls are one measurement per case, not repeated statistical trials. Local-only
public measurements below remain development diagnostics. Coverage is strongest on this small
privacy fixture; independent public recall is low. These results justify keeping defaults off
and limiting the local role, not trusting model security judgments.

## Revisions 1–2 implementation (historical)

The following records describe the earlier finish-only implementation. Revision 3 above
supersedes its missing-feature list and local eligibility; historical measurements are retained.

### Running it

```sh
duet config set review.enabled true
duet config set review.block_high true  # optional: enforce rule-confirmed high findings
```

Both settings are tighten-only in project configuration. The owner controls
`review.max_candidates` (16 local opinions per finish attempt by default; range 1–64).
The local layer uses the existing local reader in privacy mode, with its endpoint trust,
cancellation, deadline and usage accounting. Without that reader (including pass-through and
currently top clearance), it reports rules only and explicitly records unavailable opinions.
There is no fallback to a frontier provider.

The normal completion checks run first. A blocking review uses the existing failed-check repair
loop and finish-attempt budget. A successful review is appended to the operator's completion
summary. A session is reviewed on `finish`, not on progress replies or questions. Child agents do
not run separate auditors; their writes are included when the parent finishes. The tool description
mentions review only when enabled, preserving the default prompt/tool set.

### What was implemented

`duet-review` is an I/O-free tree-sitter library, shared by the auditor and the offline measurement
examples. These are original rules; no external scanner rules or benchmark source ship with it.

| Rule | Initial coverage | Enforcement |
|---|---|---|
| `tls-verification-disabled` | Python HTTP calls with literal `verify=False`; Rust methods named `danger_accept_invalid_certs` / `danger_accept_invalid_hostnames` with literal `true` | High. Only a Python call through a top-level `import requests` (including an alias), with no detected rebinding and a valid parse, is rule-confirmed. This confirms the unsafe setting, not remote exploitability. Rust receiver types are unresolved, so those findings are advisory. |
| `shell-input` | Python `os.system` / `os.popen`, and `subprocess` calls with `shell=True` or an explicit shell name in their argument evidence; recognizable input, request, environment, CLI or read expressions | High, advisory. Bounded expansion follows earlier same-function assignments, augmented assignments, member/index writes, loop bindings and list `append` / `extend` arguments, including branches. It does not prove reachability, sanitization, aliasing, container-key identity or list position. |
| `sql-dynamic-text` | Interpolation, concatenation and `.format` evidence in the first argument of `execute`, `executemany` or `query` calls | Medium, advisory. Static f-strings do not count as interpolation. Separate bound parameters are not treated as SQL text. |
| `sensitive-sink` | Sensitive identifiers, member/index/getter selectors, and string literals containing locally recognized private values in logging/outbound call arguments, with the same bounded expression expansion | Medium, advisory. Uses common credential/PII names, indexed schema names, known vault/structured values and configured detectors. Literal messages and object/keyword labels alone do not count as flows. Known values cross the internal review API only as byte ranges; protected-code tokens are excluded. Literal redaction markers are excluded from detector-generated candidates. This does not broaden the vocabulary allowed through privacy filters. |

Python, Rust, TypeScript/TSX and JavaScript/JSX syntax is accepted, with the language-specific rule
coverage above. There is no LSP/type resolution, interprocedural analysis, general alias analysis,
control-flow proof, deletion/auth-removal rule, new-host inventory, weak-crypto/credential rule,
external-scanner adapter, frontier second opinion or `duet scan` command yet.

The run-start baseline is independent of git and of the write journal. It includes dirty starting
files, new files and command writes, even if `.gitignore` excludes them. Resume reuses the baseline;
a missing baseline after work has begun, a malformed baseline, or a rules/workspace mismatch is an
error, never a silent reset. Before/after candidate multisets suppress unchanged findings and count
new duplicates. Traced argument changes count as new findings; formatting changes inside the
expression can also count again. Changes outside those expressions can be missed.

Bounds: 20,000 directory entries, 16 MiB read per snapshot, 256 KiB per file, 12,000 bytes per local
context/expression expansion, twelve expression-expansion levels, 64 stored findings per attempt,
120 seconds per local opinion (including its malformed-reply retry), and a 32,000-byte feedback
budget. The workspace walk excludes `.git`, `.duet`, `target`, `node_modules`, `.venv`, `venv`,
`vendor`, and `__pycache__`; dependency/build exclusions are counted. It ignores repository ignore
files and never reads through symlinks. Caps, read/parse failures, deletions, unsupported changed
files, and missing local opinions are reported as incomplete coverage. These limits can miss
findings and do not themselves create a security blocker. A second snapshot detects concurrent
changes to the captured scope before completion; changes in excluded/unreadable files are outside
that comparison.

`review-baseline.json` contains raw source and stays in the private run directory (0600). The
sanitized `security-review.json` includes file digests, rules, severities, opinions and timing;
it is also private and purged with the run. Review events in the audit chain contain only counts,
fixed rule IDs and outcomes; aborted reviews have fixed host reasons. Local prose is cleaned by
the same value, copied-span, re-encoding and protected-source filters as local exploration. Any
feedback sent to the frontier passes the outbound gate. The reviewer has no tools, receives only
the candidate context (no working conversation), and cannot create or veto a blocking finding.

## Revision 1 measurement (superseded by revision 2 below)

Artifacts: `/Volumes/EXT_DISK/duet_v2/experiments/security-review/`.
The [OWASP BenchmarkPython](https://github.com/OWASP-Benchmark/BenchmarkPython) checkout is pinned
to `f1291485808b66e20ddb6b01b10dc71b3df8c8ba`. The harness statically reads its
`expectedresults-0.1.csv` and source; it does not execute the benchmark or copy it into Duet.
Only its SQL-injection and command-injection categories are scored: 36 cases, 18 positive and
18 negative. These cases informed rule development, so this is a **development measurement, not
an independent holdout**. It does not validate TLS enforcement or privacy-flow recall.

| Final rules | TP | FP | FN | TN | Precision | Recall |
|---|---:|---:|---:|---:|---:|---:|
| SQL injection | 5 | 0 | 0 | 11 | 100% | 100% |
| Command injection | 5 | 0 | 8 | 7 | 100% | 38.5% |
| Combined selected categories | 10 | 0 | 8 | 18 | 100% | 55.6% |

All ten final candidates were reviewed by the configured local model (`omlx-coding`, the owner's
existing trusted endpoint). Labels, filenames and benchmark case names were withheld from the
prompt; function names were replaced with `handler`. Each opinion said `likely`. Rules plus local
judgments therefore have **the same confusion matrix** on this set: no demonstrated accuracy gain.
The local layer cannot recover a case the rules did not nominate. Ten opinions took 65.0 seconds
of local call time (65.3 seconds for the full debug-harness run), 5,633 total input tokens including
2,690 cached, and 1,955 output tokens, with no malformed retries. The model alias does not pin the
server's underlying weights.

The earlier diagnostic measurement is retained as `owasp-rules.json` and `owasp-local.json`:
the initial rules flagged static f-strings (SQL 5 TP / 11 FP) and missed explicit shell vectors
(command 0 TP / 13 FN). The local model rejected five sampled SQL false positives and retained
five true positives. Fixing the syntax rule removed those false positives without model help;
that earlier result is **not evidence of local-model value for the final implementation**.
The final result is `owasp-final.json`; source snapshots and hashes are alongside it.

On the three saved X1 `x1-chunks` hybrid workspaces, comparison with the task starter found zero
new candidates in 7/10/11 changed supported files within the file cap. Debug runs took
10.0/10.3/12.3 seconds. This is a small ordinary-diff smoke check, not a false-positive-rate claim:
large files are outside this measurement's cap and the rules cover little of a Rust SQL parser.
Details are in `normal-diffs.jsonl`.

Reproduce with the existing external build/tmp locations:

```sh
export CARGO_TARGET_DIR=/Volumes/EXT_DISK/duet_v2/target-gate
export TMPDIR=/Volumes/EXT_DISK/duet_v2/tmp-gate
cargo run -p duet-boundary --example review_benchmark -- \
  /path/to/BenchmarkPython /path/to/rules.json
# Optional local opinions, up to 64 per expected label, through owner config:
cargo run -p duet-boundary --example review_benchmark -- \
  /path/to/BenchmarkPython /path/to/local.json ~/.config/duet/config.toml 64
cargo run -p duet-boundary --example review_diff -- BEFORE AFTER [AFTER...]
```

Regression coverage includes syntax/literal controls, shadowed bindings, static parameterized
SQL, shell vectors, schema-field flows in three languages, duplicate subtraction, changed inputs,
parse/candidate caps, private baseline permissions, dirty starts, ignored/command-written files,
resume corruption, symlinks/oversized files, cancellation, concurrent writes, model opinions that
try to create or dismiss blockers, malformed reports, local-only provider roles, tool-free review,
private-value/protected-source filtering, and the real finish–block–fix loop.

Verification passed: the full `tools/gate.sh` (format, workspace lints/tests, dependency policy,
license, privacy/egress construction and provenance), final focused auditor tests, and the fast
gate. Logs are `/Volumes/EXT_DISK/duet_v2/security-review-{final-gate,final-lints,final-focused,final-fast}.log`.
The release `duet` and `duet-eval` binaries were rebuilt in the external target directory;
their hashes are in the experiment's `release-sha256.txt`. The new settings also appear in the
TUI's Limits screen. Changes remain uncommitted alongside the earlier X1 grader/retry work.

## Revision 2: mutation tracing and private-value flows

Revision 2 follows augmented assignments, member/index writes and loop bindings, with a
twelve-level expansion bound. It conservatively merges container keys and possible branches;
it does not prove that a later overwrite or sanitizer removed the sensitive input. This closes
the eight command-injection misses from revision 1 and introduces five false positives.

Privacy candidates now use syntax identifiers/selectors and local value ranges rather than
matching sensitive words anywhere in argument text. This avoids ordinary messages such as
`password rotated` and literal object labels. The boundary supplies ranges from both the vault
and structured-data index, plus configured detectors; actual values remain inside the boundary.
Protected-source tokens do not become personal-data sources, and cannot mask overlapping
private values: the reviewer searches overlapping vault matches without code tokens, then merges
structured values. A regression with an already-indexed sealed file exposed and fixed that
ordering bug after the live measurement; the checked corpus's rule counts were re-verified.
Detector matches of literal
`redacted`/`withheld` markers are ignored; known private values are still recognized. Schema and
structured-value coverage requires structure views, as before. Value ranges currently match
string literals, not numeric literals. Encoded flows are found when
the source is traced before encoding; there is no general decoder for already-encoded literals.

The candidate description tells the local reviewer when the privacy boundary recognized a
literal, and that its sensitivity may come from indexed data outside the supplied code. It
reveals no matching private record. Baselines carry revision 2; a revision-1 baseline cannot be
silently reused with different rules.

Artifacts: `/Volumes/EXT_DISK/duet_v2/experiments/security-review-v2/`. The OWASP checkout and
36-case selection are unchanged. The new authored corpus is
`crates/duet-boundary/tests/fixtures/review_privacy.json`: 12 positive flows and 12 safe controls
in Python, Rust and TypeScript. It includes schema selectors, encoding, mutations, loops,
known values with neutral names, a newly detected email, literal labels, redaction helpers,
constant overwrites and local-only uses. These are **development cases**, used to implement and
tune the rules and candidate description, not an independent privacy benchmark or holdout.

| Revision 2 rules | TP | FP | FN | TN | Precision | Recall |
|---|---:|---:|---:|---:|---:|---:|
| OWASP SQL injection | 5 | 0 | 0 | 11 | 100% | 100% |
| OWASP command injection | 13 | 5 | 0 | 2 | 72.2% | 100% |
| OWASP selected categories | 18 | 5 | 0 | 13 | 78.3% | 100% |
| Authored privacy corpus | 12 | 4 | 0 | 8 | 75% | 100% |

All 23 OWASP candidates received live local opinions through the same configured `omlx-coding`
endpoint. Every verdict was `likely`, so retaining everything except `unlikely` gives the same
matrix as the rules: **no accuracy improvement**. On cases 00350 and 00736, the explanation
correctly says the input is overwritten or the safe dictionary value is selected, but the verdict
still says `likely`; these count as failures, not corrected judgments. Case 01097's explanation
mis-evaluates a constant arithmetic branch. Some remaining cases require route/helper context
outside the bounded function supplied to the reviewer. The 23 opinions took 187.8 seconds
(188.2 seconds end to end), 13,651 input and 4,126 output tokens, no cache hits or malformed retries.
Results are in `owasp-local.json`; no prompt or rule tuning was done to repair these OWASP results.

The first live privacy pass rejected the four safe candidates but also rejected the known private
date literal, treating a hard-coded value as public. It scored 11 TP / 0 FP / 1 FN / 12 TN;
the sanitized explanations and timings are preserved in `privacy-local.json`. This prompted the
private-literal provenance description above. The initial `privacy-rules.json` is a setup
diagnostic, not a valid measurement: its one-column CSV did not seed the schema, and its reserved
example-domain email was deliberately excluded by the existing detector. The harness now asserts
that its schema and known date were indexed before scoring; corrected rules-only results are in
`privacy-rules-corrected.json`.

The final privacy rerun still scored **11 TP / 0 FP / 1 FN / 12 TN** when `unlikely` opinions
are excluded (100% precision, 91.7% recall). All 12 positive flows remain rule candidates in the
product: opinions **do not suppress findings**. The local reviewer again rejected all four safe
redaction/overwrite candidates, but dismissed the known private date even with the provenance
description. Its claim that a static literal is not personal data is wrong for this fixture.
This is a useful distinction on four controls, alongside an unsafe dismissal; it does not justify
trusting local judgments as a security decision. The 16 opinions took 122.7 seconds (123.3 seconds
end to end), 5,748 input and 2,455 output tokens, no cache hits, unavailable opinions or malformed
retries. Results are in `privacy-final.json`.

All privacy source was supplied under a sealed path. Local explanations/fixes passed through
the production `Source::Explore` privacy and protected-source filter before being stored by the
harness. Across the 16 final reports and 24 separate forced-echo probes, none contained the
checked date, email or protected-function canaries. This checks those exact canaries, not every
possible encoding or disclosure. Existing boundary/gate tests cover hostile copied, transformed
and encoded output separately; no frontier second opinion was used here.
The rejected date case's explanation retained a shortened form of the protected function name
without its canary suffix. The exact-match checker does not count that fragment. This illustrates
the existing [protected-code fragment/paraphrase limits](../SECURITY.md#protected-source-ip-levels);
these results must not be described as proving zero source disclosure.

On the three recorded X1 diffs, revision 2 again produced zero new candidates and zero incomplete
parses in 7/10/11 supported changed files within the cap, taking 12.3/12.5/12.6 seconds in debug.
This remains a narrow smoke check, not a general false-positive-rate estimate.

Reproduce the privacy run with the build/tmp environment above:

```sh
cargo run -p duet-boundary --example review_privacy -- /path/to/rules.json
cargo run -p duet-boundary --example review_privacy -- \
  /path/to/local.json ~/.config/duet/config.toml
```

New regression coverage checks mutation/loop tracing and scope separation, index writes that do
not taint their keys, literal-value ranges with invalid/UTF-8 offsets, all 24 privacy cases, and an
actual finish-time review that recognizes a neutral-named private literal while withholding it
from the report. An already-indexed sealed-file regression also verifies that code tokens do not
hide a structured date or a vault secret nested in a longer code literal, and that code-only
identifiers are not privacy sources. The local model is still the configured alias, not pinned weights. No defaults
or model enforcement authority changed.

Final revision-2 verification: the full `tools/gate.sh` passed, including the new overlap
regression and the finish-time private-literal regression. Its log is
`/Volumes/EXT_DISK/duet_v2/security-review-v2-verified-gate.log`. Final source snapshots and
hashes are in the revision-2 experiment directory; the earlier measurement files remain intact.
The release `duet` and `duet-eval` binaries were rebuilt in the external target directory
(`security-review-v2-verified-release.log`); `release-sha256.txt` records their hashes. Both
review settings still read `false`. Changes remain uncommitted alongside the previous work.

Revision-2 next steps (subsequently addressed above): validate on independently chosen vulnerability/privacy cases, improve contextual evidence
and investigate contradictory verdicts and private-literal dismissals. Keep every rule candidate
visible and both settings off by default. Repository scanning and the optional fresh-context
open-code second opinion remain M5.3 work; the milestone is not complete.
