# Publication readiness — October 4, 2026

Declass is being prepared for publication as a **GPL-3.0-or-later development
preview**. Its code, tests and recorded tasks support evaluation of frontier
coding with local handling of sensitive content. They do not establish that all
product goals are met or approve a deployment for confidential institutional data.

The benchmarked application revision is commit
`b9eb511d9e96c2d4fbb9a816ba6b6b0470c8dfa4`. The retained
[verification record](evidence/release-hardening-2026-10-03/README.md) identifies
the checked source and environments. The older
[acceptance ledger](https://github.com/maximpri/duet/blob/9b0ac104ba6947e338ca3baacfd31de2c2823e83/docs/ACCEPTANCE.md) preserves historical measurements and verdicts;
its individual rows have not all been re-audited.

## Current implementation review

The follow-up adds six release and operations improvements:

- A private model HTTP factory enforces approved destinations for inference,
  discovery and probes. New proxy and recipient-change regressions accompany it.
- Receiving-side adversarial tests cover repeated questions, cross-file values
  and new-run derived files. The [independent-review brief](SECURITY_REVIEW_BRIEF.md)
  also records a demonstrated semantic-disclosure limit. No review is commissioned.
- The [bounded comparison](evidence/benchmark-54-2026-10-04/README.md) accounts
  for all 54 outcomes and 27 task/seed pairs under the owner's $50 frontier
  allowance. It retains 53 native records and grader records; X1 hybrid seed 3
  counts as an externally adjudicated zero-score product failure, with no native
  terminal or grade fabricated. Conservative accounting across all attempts
  totals $23.55011430, including $0.44564480 retained for two requests with
  unknown usage. No fresh quality judges were run. The
  [original stopped attempt](evidence/release-hardening-2026-10-03/benchmark/README.md)
  remains unchanged.
- [Offline privacy preflight](OPERATIONS.md#preview-privacy-before-starting) reports file rules, destinations
  and policy exceptions before the first terminal-session task.
- A [binary installer](INSTALLATION.md) selects the latest owner-published GitHub
  release for the host and retains matching GPL source and notices. Unsigned
  previews require `--allow-unsigned`; optional SSH verification uses a separately
  trusted signer. macOS candidates also have disk images with an offline installer.
  The first unsigned release is [v0.1.0](https://github.com/maximpri/duet/releases/tag/v0.1.0).
- [Operational commands](OPERATIONS.md) provide metadata-only audit exports,
  integrity alerts, retention review and purge previews. Sensitive derived-file
  classifications survive new runs and purges.

The benchmark-source macOS gate passed **1,278 tests (22 ignored)**,
formatting, Clippy, dependency/license policy and architecture checks. Its
[retained record](evidence/release-hardening-2026-10-03/README.md) identifies the
checked source. A full x86-64 Linux guest passes 29 sandbox tests, six network
tests and 17 setup/doctor tests. This check reproduced and fixed a cancellation
race (DECLASS-2026-039); the original failing regression passes unchanged. The
guest uses an emulated complete x86 machine and kernel, not user-mode container
translation. Physical x86 hardware has not been tested in this review.

## Goals and evidence

| Goal | Evidence available | What remains |
| --- | --- | --- |
| Frontier-level coding results | [Frozen nine-task comparison](evidence/benchmark-54-2026-10-04/README.md): 54 outcomes/27 pairs; mean counted per-case hidden-test scores of 89.26% hybrid and 96.54% passthrough. Mechanical successes: 14/27 and 15/27. [Earlier judged results](VALUE_EVIDENCE.md) remain historical. | Fresh quality judging, broader representative tasks and deployment-specific acceptance. These mechanical results do not establish general frontier parity. Hosted and local model weights were not immutably pinned. |
| Sensitive content handled locally | Classified files use local processing, placeholders and structure views; outbound requests are filtered and checked. Recorded synthetic tasks include zero-canary outcomes and retained failures. [Security design](SECURE_BY_DESIGN.md), [launch evidence](launch/DEMO.md) | Independent adversarial assessment and validation of each deployment's data types. Classification limits, semantic inference and probing remain in the [threat model](../SECURITY.md). Hybrid intentionally sends permitted code and checked context to the frontier. |
| Local-only operation | Top-clearance tests exercise a local endpoint, frontier trap, disabled web/command networking and restricted MCP startup. [Tests](../crates/declass-cli/tests/top_clearance.rs) | Validate the approved endpoint and its transport. An owner-allowlisted remote local model receives sensitive data; this mode alone does not establish an air gap. |
| Tool isolation and policy enforcement | OS sandbox, owner/project policy separation, reserved-path protection and gated egress have automated coverage. The publication work fixed **DECLASS-2026-033–039**, including proxy routing, persistent derived classifications and cancellation. Offline diagnostics skip network-enabled MCP servers. Linux runtime evidence covers ARM64 and a complete emulated x86-64 kernel with seccomp enabled. | Reassess configured extensions and local endpoint trust in the deployment environment; validate its exact OS and hardware. |
| Recoverable runs and inspectable audits | Tests cover termination, interruption, resume and hash-chain verification. Derived classifications survive new runs and purge; audit export, checks, retention reports and purge previews have CLI regressions. [Operations](OPERATIONS.md), [runtime tests](../crates/declass-cli/tests/termination.rs) | Operational retention, access control and independent custody of audit anchors. A valid chain verifies recorded integrity, not whether its contents were safe to disclose. |
| GPL source and binary distribution | License grant, retained upstream notices, dependency policy and scripts for matching vendored source accompany the release procedure. An isolated source archive rebuilt successfully with Cargo's frozen mode. [Licensing and distribution](../LICENSES.md) | For a binary release, package from the final clean commit, verify checksums, and publish matching source and notices alongside the binary. Publisher signing is optional; identify unsigned previews clearly. |

## Current verification

| Check | Status |
| --- | --- |
| Release automation follow-up | **Passed:** complete macOS gate with 1,296 tests (22 ignored), including latest-release installation, explicit unsigned mode, signed-failure handling and offline DMG installation. Both unsigned macOS images passed disk-image and payload checks; native ARM64 installation passed in an isolated directory, and the Intel binary passed a Rosetta version check. This does not add native Intel sandbox qualification. No release was published. |
| Benchmark-source macOS ARM64 gate | **Passed:** formatting, Clippy, 1,278 tests (22 ignored), dependency/license policy, headers, privacy/egress construction and provenance. Includes the final cancellation and benchmark scheduling changes. |
| Linux x86-64 sandbox | **Passed:** original 29 sandbox and six network tests in a complete Debian 6.1 x86 guest; 17 setup/doctor tests also passed. A single-thread TCG rerun passed without the multi-thread emulator's memory-ordering warning. Logs retain the original cancellation failure and both passing runs. |
| Rust 1.90 Linux x86-64 build | **Passed:** optimized application build at `b9eb511`, evaluation harness compilation, version/help checks and all 11 release tests. Operations/privacy/proxy tests passed on the preceding implementation; their sources did not change. [Platform record](evidence/release-hardening-2026-10-03/linux-x86/README.md) |
| GPL corresponding source | **Passed:** archive at `1e4f2e4`, all tracked entries and 320 vendored dependency packages verified, no macOS metadata or external links. All build inputs match the successful optimized frozen build from `ae66b4a`, performed with an initially empty Cargo home. [Build and equivalence record](evidence/release-hardening-2026-10-03/offline-source/README.md). No production release is signed or published by this review. |
| Fresh bounded benchmark | **Full matrix artifact verification passed:** 54 outcomes, 27 pairs and 27 counted scores per lane; 53 actual native records and grader records. Successful native terminals: hybrid 26/27, passthrough 27/27. X1 hybrid seed 3 received an external zero-score disposition and was not rerun. All-attempt conservative accounting is $23.55011430, including two retained unknown-usage reservations totaling $0.44564480. [Outcomes, provenance and audit verdict](evidence/benchmark-54-2026-10-04/README.md). Quality judging remains unscored; mixed execution timing is not a controlled latency comparison. |

## Retained October 2 baseline

These checks belong to the earlier source snapshot. They complement the current
record rather than asserting that every platform was rerun after every change.

| Check | Status |
| --- | --- |
| Focused security integration | **Passed:** 2 no-local-model tests, 4 outbound-gate tests and 5 top-clearance tests; required-clearance unit test also passed. |
| Sensitive-command regression suite | **Passed:** 6 tests, including interruption/resume, derived build output and denial of run-state/git access. |
| Full workspace gate on the combined changes | **Passed:** format, Clippy, 1,235 tests (22 environment-dependent tests ignored), dependency policy, headers, privacy and provenance. The yanked `yoke-derive` 0.8.3 was updated to 0.8.4; no policy exception was added. |
| Linux sandbox runtime | **Passed:** 476 tests in each privileged, unprivileged and root mode, with 12 external/manual tests ignored per mode; registry bridge checks passed in privileged and unprivileged modes; all 3 namespace-denial checks passed. ARM64 Linux, bubblewrap 0.8.0. This run does not verify the x86-64 seccomp implementation. |
| Model connection, setup and release regressions | **Passed on macOS and ARM64 Linux:** 12 isolated HTTP/HTTPS environment-proxy cases, 17 setup/doctor tests, and 3 release-script tests. Signed manifests containing NUL, CR, tab, duplicate entries or malformed records are refused on macOS Bash 3 and Linux Bash 5. |
| Evaluation harness and fixtures | **Passed:** all 13 task packages and seals validate; harness self-test passed for canaries, proxy, statistics and pricing. X1 fixture changes add licensing comments only; the original seal and before/after hashes are retained. |
| GPL source package and offline build | **Passed:** release-script checks and frozen release build from a vendored archive with an empty Cargo home. The archive includes the final proxy fix and updated lockfile; product sources were compared byte-for-byte with the reviewed tree. [Build record](evidence/licensing-2026-10-02/offline-source-validation.json) |
| Release identity | The verification record identifies this review's source files. A signed production release has not been created; generate its archive, build information and signature from its clean release commit. |

## Publication and adoption

The repository and unsigned `v0.1.0` release are public. Binary artifacts and
their matching source are from `9aa25892f2c8e3b545dbeaee466d2178cf21d73c`;
the disk-image installer and packaging scripts are from `2a8fb6a`.
All six downloads retain their corresponding source and license notices.
The macOS images are not Apple-signed or notarized. A dedicated SSH public key is
documented in the [installation guide](INSTALLATION.md#optional-ssh-signatures)
for future signed releases; it does not authenticate these unsigned artifacts.

Publication follow-up:

- GitHub private vulnerability reporting is enabled. The
  [reporting form](https://github.com/maximpri/duet/security/advisories/new) sends
  reports to repository maintainers. The website does not accept reports;
  `docs/security.txt` remains an undeployed template with the GitHub contact.
- Outside code contributions remain closed until the CLA required by the
  [contribution policy](../CONTRIBUTING.md) is published.
- Retain source and notices alongside every binary release, following
  [LICENSES.md](../LICENSES.md).

At the time of this review, the placeholder was deployed on [Cloudflare Pages](https://declass-site-9oe.pages.dev).
`duet.priezjev.com` was registered with Pages but still required a proxied CNAME:
`declass` → `declass-site-9oe.pages.dev`. The reviewed deployment login had no DNS-write permission. See [site deployment](../site/README.md) for the maintained procedure; this dated record does not verify current DNS state.

The October 2 baseline used Rust 1.97.1 on macOS and 1.99 on Linux. The current
Linux check additionally builds with the declared Rust 1.90 minimum.

Before using real private data, evaluate representative
tasks and threat scenarios, approve local and frontier endpoints, and establish
policy administration, audit custody and incident response. The
[deployment evaluation guide](SECURE_BY_DESIGN.md#evaluate-your-deployment) sets out that work.
These are deployment decisions; neither the project name “top clearance” nor a
passed test suite is a security accreditation.
