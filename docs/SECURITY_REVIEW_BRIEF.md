# Security review brief

Prepared 2026-10-03 for an independent review of Declass's development preview.
This document defines proposed scope; no external review has been commissioned
or completed. It is not approval for classified, regulated or production data.

## Boundary to review

In hybrid mode, the configured frontier receives policy-approved views of tool
results and conversation messages. The local model receives raw content needed
for local tasks. “Local” includes an owner-allowlisted remote endpoint: that
server, its operators, retention and transport are inside the trust decision.
Approved model clients disable redirects and environment proxies. Local URLs
must pass endpoint approval before inference, discovery or context probing.
API adapters cannot switch a transport's approved origin. This is endpoint
policy, not certificate pinning or a defence against a compromised model host.

The frontier gate checks serialized requests. The sandbox restricts ordinary
commands' access to sensitive paths, process/network capabilities and writable
locations. Sensitive commands may read private inputs; their outputs are
withheld and their written files become sensitive. Workspace classifications
survive new run IDs and run-state purge. A durable pending marker is written
before sensitive execution; unfinished classification prevents the next hybrid
startup. Inspection reads classification paths without reading file contents.
Local-only/top-clearance operation has different controls and must be reviewed
separately from hybrid mode.

The owner, owner-controlled configuration, host OS and protected `.declass` state
are trusted. Deleting classification state while keeping derived outputs can
remove their classification. Declass does not defend against an owner or malware
that can rewrite this state, the executable or the workspace lock. Private run
files are permission-restricted, not encrypted by Declass. Sensitive information
can remain in local model infrastructure and local state until its configured
retention or an explicit purge takes effect.

## Evidence and tested attacks

Tests use synthetic data and deterministic model stand-ins unless explicitly
identified as live. They establish behavior for their cases, not a disclosure
rate for arbitrary natural language.

| Surface | Evidence | What the check observes |
| --- | --- | --- |
| Model redirects and recipient substitution | `declass-provider` unit tests `http_transport_does_not_forward_requests_to_redirect_recipients` and `approved_transport_refuses_an_adapter_switching_the_recipient` | Independent loopback recipients see no redirected or substituted request. |
| Environment proxy routing | `crates/declass-cli/tests/provider_proxy.rs` | Isolated subprocesses set six HTTP/HTTPS/ALL proxy-variable spellings. A receiving proxy must see no model request, listing or context probe; direct HTTP receives the expected requests. |
| Adapter construction | `tools/gate.sh`, model HTTP policy check | Additional client constructors and proxy/redirect overrides outside the private factory fail the repository gate. This source check supplements review; it is not a language-level proof. |
| Rephrased questions, repeated requests and joins | `crates/declass-boundary/tests/adversarial_disclosure.rs` | A separate TCP receiver records actual serialized frontier request bodies, across repeated questions about two private sources. Known synthetic identifiers and credentials must be absent. |
| New run and sensitive output lineage | Same socket-observed suite; `engine::prime_tests::derived_files_remain_sensitive_in_a_new_workspace_run` | An ordinary-path export with sensitive prose remains hidden in a different run and absent from outbound request bytes. The unit regression failed before the workspace-wide store fix. |
| Timestamp-preserving copies | `tools::tests::timestamp_preserving_sensitive_*` | Two regressions failed before the fix: restoring mtime on an equal-sized rewrite, and a new build export with an old source timestamp. Before/after inode metadata now catches both without classifying untouched recent build outputs. Failed tree inspection leaves the pending marker. |
| Incomplete or invalid state | `derived::tests` | Pending commands, corrupt manifests, invalid relative paths and symlinks refuse startup/preview; an injected disk-full write leaves the pending marker. Read-only inspection creates no state and imports retained legacy classifications. |
| Interrupted sensitive commands | `crates/declass-agent/src/tools.rs`, `interrupted_sensitive_commands_keep_their_written_files_private_on_resume` | Files written before command interruption remain private when the run resumes. |
| Cancellation while a shell waits for a child | `declass-sandbox::tests::a_stop_kills_the_whole_tree_at_once` (unchanged) | The original full sandbox suite reproduced a post-cancellation marker on a complete x86-64 Linux kernel under QEMU. Freezing ancestors before child termination and bounded rescans fix the race; all 29 sandbox and six network tests pass with kernel seccomp enabled. See DECLASS-2026-039 and the platform evidence. |
| Other disclosure forms | Existing privacy scenarios, canary matcher and boundary tests | Literal values, selected encodings, short number extraction, images, tool arguments, logs and operator text. Consult each test's assertions rather than extrapolating to all transformations. |

Focused local checks passed during this change: provider unit tests (94 passed,
2 live ignored), three socket-observed disclosure cases, six derived-state
regression cases, the pinned-parent write test, all 24 existing privacy
scenarios, and the extended proxy subprocess regression. Focused clippy checks
passed with warnings denied. The final agent tool subset passed 11 tests,
including both timestamp-preserving regressions and failed-inspection handling.
After replacing the build timestamp-window heuristic with before/after snapshots,
all six sensitive-command tests and both timestamp regressions passed.
The final combined macOS gate passed 1,278 tests (22 ignored), Clippy,
formatting, dependency/license policy and architecture checks. A complete
x86-64 Linux guest passed 29 sandbox tests, six network tests and 17
setup/doctor tests. It reproduced the cancellation race above before the fix;
the original regression passes unchanged afterward. The
[verification record](evidence/release-hardening-2026-10-03/README.md) identifies
source snapshots, toolchains and limitations; [publication readiness](PUBLISH_READINESS.md)
separates these results from historical checks and live benchmark evidence.

## Current live comparison and deadline incident

The [October 4 evidence packet](evidence/benchmark-54-2026-10-04/README.md) contains 54 counted outcomes across 27 paired cases, with 53 native records and grader records. Hybrid's mean per-case hidden-test score was 89.26%, versus 96.54% for passthrough. Independent capture checks found zero literal planted-value occurrences in 1,124 hybrid frontier requests; all 27 hybrid cases had sink rescans with zero measured violations. These observations do not establish semantic secrecy or constitute an external security review.

X1 hybrid seed 3 remained live after a frontier response reported HTTP 200 while its captured body stayed empty. A separately reviewed procedure used a recorded post-construction monotonic anchor, the original native time budget and a conservative grace period before an external deadline intervention. Scheduling was frozen; the same attempt was ended and counted as a zero-score product failure without rerunning it. No native run record, summary or grade was manufactured, and the outcome is not labeled as a native-reported timeout. Final verification checked process, writer and lease quiescence and retained unknown usage. This evaluation procedure is not a shipped automatic recovery feature.

The stall's cause remains unresolved. Review the frozen HTTP/SSE body handling, scheduling and whole-run cancellation paths, then seek a bounded offline reproduction that independently observes deadline progress, task cancellation and descendant cleanup. The observation alone does not identify a dependency bug or prove a timer fix. No fresh quality judges or commissioned external reviewers participated in this comparison.

## Demonstrated limit: semantic disclosure

The socket-observed test
`semantic_inference_is_a_documented_limit_even_when_literal_content_is_withheld`
intentionally verifies that a nonliteral conclusion about a private decision
can reach the frontier while the literal private reference is withheld. A
summary saying a customer cannot proceed with a purchase discloses meaning,
even without repeating the source's words. Passing this characterization is
**evidence of a limit**, not evidence that semantic disclosure is prevented.

Paraphrases, yes/no answers, correlations between files and repeated adaptive
queries can reveal information without repeating protected values. Current
filters and disclosure budgets do not establish general semantic secrecy.
Tasks requiring that property need local-only processing or a separately
specified, reviewed release policy for permitted conclusions.

## Questions for the independent reviewer

- Can an agent or tool change a request destination, exploit URL normalization,
  DNS behavior, redirects, credentials, provider retries or a new adapter to
  bypass the approved client or frontier gate?
- Can a command, MCP server, image route, language server, plugin, browser/web
  result or subagent obtain private content outside its assigned boundary?
- Do write-ahead classification, directory handles, interruptions, disk errors,
  migration and retention preserve sensitivity across all relevant crashes?
  Review simultaneous actors and both Linux and macOS filesystem behavior.
- What inference policy is required for the deployment? Measure adaptive
  paraphrase, yes/no, cross-file and multi-session attacks with real models and
  receiving-side observation. Report semantic inferences separately from exact
  canary escapes and task quality.
- Are Linux x86_64 and ARM64 sandbox rules, macOS sandbox behavior and fallback
  paths equivalent to their documented claims on supported OS versions?
- Are signed release verification, dependency/source provenance, private
  reporting, incident response, model-host retention and operator recovery
  procedures sufficient for the intended organization?

A useful deliverable is a reproducible threat-model review with receiving-side
traces for synthetic attacks, platform/version details, severity-ranked
findings, patch verification and explicit exclusions. Bank or government use
requires the organization's own controls and acceptance criteria; the project
makes no certification or universal frontier-quality claim.
