# Secure by design: inspect the boundary

Duet's design goal is to keep content classified as sensitive or protected out of the cloud frontier's context. The enforcement belongs to the host application and OS, outside the model's instructions. This document links the claim to inspectable mechanisms and tests; it is not a formal proof of security.

![Duet's data flow: local handling and a separate host policy gate precede the frontier model.](assets/infographics/duet-boundary-gpt.png)

[Read the diagram step by step](DUET_VISUAL_GUIDE.md#how-the-boundary-works). The gate is part of the host application; neither model can authorize disclosure. An approved self-hosted local endpoint and its network path belong inside the trusted environment.

## Evidence you can check

| Evidence | What it establishes | What it cannot establish |
| --- | --- | --- |
| [Gate implementation](../crates/duet-boundary/src/gate.rs) | Where requests are checked and recorded | That classification recognizes every sensitive value |
| [Adversarial transport tests](../crates/duet-cli/tests/privacy_scenarios.rs) | What a scripted frontier actually receives under specific attacks | Every possible model behavior or side channel |
| [Frozen paired development benchmark](VALUE_EVIDENCE.md) | Observed privacy, quality, cost and latency on a specified suite | Universal frontier parity, cost savings or an independent assessment |
| [New live dogfood and original audit](launch/DEMO.md) | Actual model execution, the code change, acceptance tests and recorded outbound content | Provider receipt, exhaustive traffic capture or production readiness |
| [Failed dogfood and its regression](evidence/launch-2026-10-01/regression-before.txt) | A concrete disclosure path, found and reproduced before the fix | That fixing it eliminates all disclosure paths |

The first launch run exposed arbitrary customer values through a diagnostic preview. Its audit chain was intact: **an intact log proves integrity of the record, not safety of the content.** The [before/after packet](launch/DEMO.md#what-the-first-run-found) is part of the evidence, not discarded demo footage.

## 1. Outbound requests have one checked path

`OutboundGate` constructs `GatedFrontier`; the agent receives the gated interface. Text is filtered, checked, then recorded before sending. The check includes tool arguments and replayed context, not just the latest user message. A refused part may be withheld and the remainder rechecked; a request that cannot be cleared fails.

- Code: [gate](../crates/duet-boundary/src/gate.rs), [outbound filtering](../crates/duet-boundary/src/engine/outbound.rs), [CLI composition](../crates/duet-cli/src/lib.rs).
- Tests: [outbound gate](../crates/duet-cli/tests/outbound_gate.rs), [filter/check agreement](../crates/duet-boundary/tests/outbound_agreement.rs), [provider dialects](../crates/duet-boundary/tests/dialect_gate.rs).
- Limit: known-value checks cannot catch a value that no layer recognizes. A summary can disclose meaning without quoting a secret.

## 2. Tool permissions live outside the model

Ordinary commands cannot read sensitive paths, Git history, Duet state or configured credential stores. Commands that explicitly require sensitive data use a different path: network disabled, output retained behind a handle, and derived files treated as sensitive. macOS uses Seatbelt; Linux uses bubblewrap with seccomp. The operator's host remains trusted.

- Code: [sandbox](../crates/duet-sandbox/src/lib.rs), [sensitive-file handling](../crates/duet-boundary/src/engine.rs).
- Tests: `a_sensitive_data_command_cannot_read_the_vault`, `a_file_derived_by_a_sensitive_data_command_stays_sensitive_when_read`, and `a_file_derived_into_target_stays_sensitive_when_read` in [privacy scenarios](../crates/duet-cli/tests/privacy_scenarios.rs).
- Network evidence: [sandbox network tests](../crates/duet-sandbox/tests/network.rs), [independent egress-oracle scenarios](../crates/duet-cli/tests/egress_oracle.rs). Those are separate from the new demo's audit inspection.
- Limit: normal registry access is allowed by default. `sandbox.network = "off"` tightens that. Do not describe hybrid mode as air-gapped.

## 3. The repository cannot promote its own permissions

Repository content is untrusted. Project config can tighten policy but cannot choose owner endpoints, credentials or looser privacy settings. An explicitly configured policy must load successfully; failure does not silently remove it. Core's policy file is unsigned; a deployment must protect it or supply a verifying policy source.

- Code and tests: [configuration](../crates/duet-config/src/lib.rs), [policy implementation](../crates/duet-config/src/policy.rs), [CLI secure defaults](../crates/duet-cli/tests/secure_defaults.rs).
- Reproduce: `cargo test --locked -p duet-config` and `cargo test --locked -p duet-cli --test secure_defaults`.
- Limit: this is not a shipped fleet-management, identity or policy-signing service. The trusted owner can change policy; a compromised owner account is outside the boundary.

## 4. The local model is not a trusted declassifier

In hybrid mode the local reader has no agent tools and cannot decide the next action. Its output passes copied-span, known-value and encoding checks. Probe budgets limit repeated attempts to recover values. Protected code can be worked on locally while the frontier receives allowed interfaces and test outcomes.

- Code: [local reader](../crates/duet-boundary/src/local.rs), [engine](../crates/duet-boundary/src/engine.rs), [protected-code path](../crates/duet-boundary/src/engine/protected.rs).
- Tests: careless-local scenarios cover direct copying, digit fragments, spelled-out values and encoding; [structure tests](../crates/duet-boundary/tests/structure_views.rs) cover shapes and synthetic data.
- New regression: `error_words_in_customer_data_do_not_publish_rows_as_diagnostics` captures the frontier transport and asserts that the dogfood's ID, reserved-domain email, name and amount do not arrive. The boundary regression also covers repeated rows with structure views disabled. A second live failure led to `local_digest_cannot_quote_short_structured_values`, which checks amounts and identifiers repeated by a local digest. This extra matcher covers structured values of at least four characters up to the existing per-run index cap; it is not a guarantee for every scalar, fragment or paraphrase.
- Limit: semantic leakage, inference from structure and unknown value formats remain relevant. Open source files are cloud-visible unless marked otherwise.

## 5. Top clearance removes the frontier

`duet --mode top-clearance` makes the configured local model the coding agent. Web tools and command networking are disabled; networked MCP servers are not offered. `clearance.required = "top"` prevents a session from switching to hybrid or passthrough for that repository.

- Tests: `only_the_local_model_works_and_nothing_leaves`, `a_repository_can_require_top_clearance`, and `a_session_continues_in_top_clearance_and_cannot_leave_it` in [top-clearance scenarios](../crates/duet-cli/tests/top_clearance.rs). The tests use a frontier trap endpoint and assert it receives nothing.
- [Live local-model task](launch/DEMO.md): separate from those scripted tests.
- Limit: “local” is the configured endpoint, which can be a remote self-hosted machine. Top clearance is not a guarantee of same-device processing, no networking anywhere on the host, or frontier-level quality. The launch changes correct the TUI banner to describe disabled frontier/web/command networking; a self-hosted LAN endpoint still receives local-model requests.

## 6. Audit records are inspectable and tamper-evident

Duet records outbound request bodies, hashes and boundary events in a chain whose head is anchored outside the repository. You can inspect the exact prepared request, run verification and review disclosure counts. CLI verification checks the anchor as well as internal chain consistency.

```sh
duet audit show <run-id>
duet audit verify <run-id>
duet audit disclosure <run-id>
```

- Code: [audit implementation](../crates/duet-boundary/src/audit.rs).
- Tests: `chain_verifies_and_detects_tampering`, `anchor_detects_a_rewritten_or_truncated_log`, and the [CLI rewrite test](../crates/duet-cli/tests/secure_defaults.rs).
- New observation: [live verification and deliberate corruption of a copy](launch/DEMO.md).
- Limits: the record is not an acknowledgement from the provider. Someone controlling both log and anchor can rewrite both. Local transcripts/handles can contain sensitive content; top-clearance audit bodies can contain raw requests to the local model. A disclosure bug can also leave sensitive content in a hybrid audit. Protect storage, backups and access. Tamper evidence is not encryption or an immutable external archive. A session's disclosure report can lack per-result counts when no run `summary.json` exists; do not mistake that for a zero.

See the [detailed institutional evaluation guide](INSTITUTIONAL_EVALUATION.md) for actual audit fields, a synthetic pilot and a control-to-evidence map.

## An institutional evaluation path

These are engineering evaluation suggestions, not a compliance determination. Canadian banking guidance makes classification, protection and security logging useful evaluation dimensions; it does not approve Duet. See [OSFI B-13](https://www.osfi-bsif.gc.ca/en/guidance/guidance-library/technology-cyber-risk-management). The Cyber Centre recommends avoiding sensitive corporate information in generative-AI prompts and managing deployment risks; see [ITSAP.00.041](https://www.cyber.gc.ca/en/guidance/generative-artificial-intelligence-ai-itsap00041). The [NIST Generative AI Profile](https://www.nist.gov/publications/artificial-intelligence-risk-management-framework-generative-artificial-intelligence) provides a broader risk-management reference, not a product certification.

| Evaluate before production | Available foundation | Work still required |
| --- | --- | --- |
| Data boundary | Hybrid filtering, protected source, top clearance | Your classification policy, endpoint approval and observed network testing |
| Ownership and access | Owner/project separation; policy interface | Managed installation, operator identity, privilege separation, policy distribution |
| Audit custody | JSONL requests/events; external local anchor | Access controls, approved retention, encrypted storage and off-host immutable collection |
| Incident handling | Published advisories and regressions | Replace the placeholder security contact; tested private reporting and response process |
| Software supply chain | Source and lockfile; release tooling | Reviewed signed artifacts, SBOM, provenance, vulnerability process and deployment validation |
| Security assurance | Tests and development canaries | Independent review/red team, endpoint and OS coverage, new attack classes |
| Quality and economics | Paired development evidence | Fresh paired runs on representative workloads, real usage charges and local operating costs |

Start with invented records and adversarial canaries in an isolated repository. Verify allowed and denied flows independently. Only then evaluate a narrowly scoped deployment under the organization's own approvals. A bank or public-sector team can assess these controls; this repository does not claim to have passed their approval process.
