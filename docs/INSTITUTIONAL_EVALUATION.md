# Evaluate Duet for sensitive engineering work

Duet gives a security team something concrete to evaluate: the boundary between a coding model and the information it may receive. Start with the [real synthetic-data run](launch/FRESH_DOGFOOD.md), inspect its requests, then reproduce the controls in your own environment.

**Development preview, not a production approval.** Core includes local processing, policy separation, tool isolation and verifiable logs. Fleet identity, managed policy distribution, independent assurance and durable audit custody still require deployment work. The guide covers Duet Core; it makes no claims about separately developed enterprise products.

## Choose the data flow first

| Deployment | Processing and disclosure | Evaluate |
| --- | --- | --- |
| Hybrid + loopback local model | Sensitive inputs are handled on the workstation; open code and checked context can reach the chosen frontier | Classification coverage, inference from summaries, cloud contractual/retention terms, tool egress |
| Hybrid + self-hosted local model | The local server also receives sensitive inputs; the host and network path join the trusted environment | Approved host/jurisdiction, TLS or secured tunnel, authentication, server logging/backups, access control |
| Top clearance + approved local endpoint | The local model performs coding; no frontier, web tools or command networking | Local-model quality, endpoint trust, pre-staged dependencies, host-level network observation |

The word “local” does not establish data residency or encryption. A remote endpoint configured for the local role can receive raw sensitive content. The launch footage uses fictional data on an explicitly allowed plaintext LAN connection; it does not demonstrate production transport. Open code is cloud-visible in hybrid unless its policy says otherwise.

## What the audit actually records

The [request record type](../crates/duet-boundary/src/audit.rs) includes:

| Field | Meaning |
| --- | --- |
| `seq`, `prev` | Record sequence and previous-record hash |
| `unix_ms` | Timestamp supplied by the host; not a trusted external time attestation |
| `endpoint`, `model` | Configured destination and provider/model identifier |
| `request_sha256`, `request` | Digest and prepared JSON request; image payloads are represented by digests and sizes |
| `interventions` | Filtering and blocking decisions recorded for the request |

The same log carries typed security events, including mode, tool/network decisions and security-review outcomes where those paths run. Event variants have their own fields; not every line is a model request. A head anchor outside the repository detects rewriting/truncation against that anchor. [Implementation and negative-control evidence](SECURE_BY_DESIGN.md#6-audit-records-are-inspectable-and-tamper-evident).

```sh
duet audit show <run-id>
duet audit verify <run-id>
duet audit disclosure <run-id>
```

**Verification checks record integrity, not disclosure safety.** Also inspect content and independently observe allowed/denied network traffic. The failed launch run had an intact audit chain. A person who controls both the log and anchor can replace both; a bundled anchor is not independent custody. Interactive disclosure summaries can lack counts when no `summary.json` exists.

Treat logs as sensitive. Local request bodies, transcripts, handles and a failed filter can retain private content. Decide who can read them, how they are encrypted, how long each artifact is retained, and how deletion interacts with investigations. A metadata-only external collector should preserve sequence/hash/endpoint/model/size and gap alerts without automatically exporting raw requests. That collector and immutable storage are deployment requirements, not a shipped Core service.

## A reproducible synthetic pilot

1. **Freeze the environment.** Record source/binary hashes, OS, policy, endpoints, model/quantization, credential source and dependency versions. Use an isolated repository with invented records. Assign an engineering owner and security reviewer.
2. **Run the coding task.** Copy the [billing fixture](launch/DEMO.md#reproduce-the-task); observe the initial failure, run with its acceptance check, review the diff and rerun all tests. Retain attempts and failures.
3. **Check the disclosure.** Run the [canary checker](../tools/check-launch-canaries.py) against the recorded frontier bodies. Inspect summaries for inferred sensitive facts as well as exact values. Capture traffic independently on the approved endpoint/proxy for a deployment evaluation.
4. **Exercise the controls.** Test hostile file instructions, arbitrary identifiers, encoded values, derived files and repeated questions with the [adversarial scenarios](../crates/duet-cli/tests/privacy_scenarios.rs). Verify commands cannot read protected state or send unapproved traffic; [egress tests](../crates/duet-cli/tests/egress_oracle.rs) supply examples.
5. **Check audit custody.** Verify the original, change a copy, truncate a copy and confirm rejection against the preserved anchor. Simulate a missing record/collector outage in your proposed storage design. Keep the source records intact.
6. **Repeat with no frontier.** Require top clearance in a fresh fixture. Use a receiving/trap endpoint and host network observations, not only the TUI label. Check behavior when the local endpoint is unavailable. Measure coding quality independently.
7. **Write the decision.** List observed flows, outcomes, failures, residual risks, false positives, costs, support ownership and exclusions. Approve only the specific deployment assessed; synthetic success alone does not approve real data.

## Control-to-evidence map

| Evaluation question | Available evidence | Remaining decision or gap |
| --- | --- | --- |
| Can private records reach the frontier? | [Classification, local-output checks and gate](SECURE_BY_DESIGN.md); [failed and fixed runs](launch/DEMO.md) | Organization-specific categories, semantic inference, unrecognized formats and independent adversarial review |
| Can tools bypass the gate? | [Sandbox and network scenarios](SECURE_BY_DESIGN.md#2-tool-permissions-live-outside-the-model) | Validate your OS, extensions, network policy and privileged-user threat model |
| Can a repository loosen controls? | [Owner/project separation](SECURE_BY_DESIGN.md#3-the-repository-cannot-promote-its-own-permissions) | Protect/distribute owner policy; Core does not supply fleet IAM or a policy-signing service |
| Can we investigate an incident? | Request/event JSONL, local anchor, verification CLI | Central collection, access controls, retention, backup, alert routing and response ownership |
| Can we trust the installed build? | Source, lockfile, tests and release tooling | Reviewed signed artifacts, SBOM/provenance validation, update and rollback process |
| Can developers complete real work? | [Current 54-outcome mechanical comparison](evidence/benchmark-54-2026-10-04/README.md): 89.26% hybrid versus 96.54% passthrough mean per-case hidden-test scores; 53 native/grader records, one externally enforced zero-score failure; real TUI dogfood | Representative internal tasks, human review, deployment latency and cost per accepted task; no fresh quality judges or controlled timing comparison |

## Research basis

These sources guide the evaluation questions. This is an engineering map, not a compliance opinion or endorsement.

- **Canadian banking:** OSFI B-13 addresses classification (3.1.4), secure design (3.2.1), data protection (3.2.5), configuration (3.2.8) and centralized security logging/retention (3.3.1). These motivate checking Duet's mechanisms and the surrounding operational controls separately. [OSFI B-13](https://www.osfi-bsif.gc.ca/en/guidance/guidance-library/technology-cyber-risk-management).
- **Sensitive prompts:** the Canadian Cyber Centre advises against putting personal or sensitive corporate information into generative-AI prompts. Our application of that advice is to test every context path, including tool output and local summaries, and to approve the local endpoint itself. [ITSAP.00.041](https://www.cyber.gc.ca/en/guidance/generative-artificial-intelligence-ai-itsap00041).
- **AI risk management:** NIST's Generative AI Profile offers a cross-sector risk-management reference. Use it to structure the organization's assessment alongside its own policies; a local model or a passed canary test is not NIST certification. [NIST AI 600-1](https://www.nist.gov/publications/artificial-intelligence-risk-management-framework-generative-artificial-intelligence).

Sources rechecked 2026-10-01, America/Toronto. Jurisdiction and institutional policy determine additional requirements.

## Priorities before real-data adoption

First close disclosure findings, establish a working private vulnerability-reporting route, verify installation provenance and commission independent assessment. Then validate managed policy, endpoint security and external audit custody in the intended deployment. Use the [current mechanical results](evidence/benchmark-54-2026-10-04/README.md) as a starting point, retain the [historical judged evidence](VALUE_EVIDENCE.md#historical-comparison-september-30) separately, and publish representative acceptance results for the intended deployment alongside those controls.

These are the steps toward becoming a tool a regulated team can trust. The README should invite that evaluation rather than imply it has already happened.
