# Publication readiness — October 2, 2026

Duet is being prepared for publication as a **GPL-3.0-or-later development
preview**. Its code, tests and recorded tasks support evaluation of frontier
coding with local handling of sensitive content. They do not establish that all
product goals are met or approve a deployment for confidential institutional data.

This review covers the source changes committed with this document; the retained
[verification record](evidence/publication-review-2026-10-02/README.md) identifies
the checked files and logs. The older
[acceptance ledger](ACCEPTANCE.md) preserves historical measurements and verdicts;
its individual rows have not all been re-audited.

## Goals and evidence

| Goal | Evidence available | What remains |
| --- | --- | --- |
| Frontier-level coding results | The project's paired small-to-large benchmark reported 98.3% hidden-test success for hybrid versus 97.5% for passthrough, with its quality gate passed. [Results and methodology](VALUE_EVIDENCE.md) | A fresh representative benchmark on the final release build, both judge families, protected-code tasks and reproducible public results. Later fixes and unpaired reruns do not establish universal parity. |
| Sensitive content handled locally | Classified files use local processing, placeholders and structure views; outbound requests are filtered and checked. Recorded synthetic tasks include zero-canary outcomes and retained failures. [Security design](SECURE_BY_DESIGN.md), [launch evidence](launch/DEMO.md) | Independent adversarial assessment and validation of each deployment's data types. Classification limits, semantic inference and probing remain in the [threat model](../SECURITY.md). Hybrid intentionally sends permitted code and checked context to the frontier. |
| Local-only operation | Top-clearance tests exercise a local endpoint, frontier trap, disabled web/command networking and restricted MCP startup. [Tests](../crates/duet-cli/tests/top_clearance.rs) | Validate the approved endpoint and its transport. An owner-allowlisted remote local model receives sensitive data; this mode alone does not establish an air gap. |
| Tool isolation and policy enforcement | OS sandbox, owner/project policy separation, reserved-path protection and gated egress have automated coverage. This review fixed interrupted sensitive output classification, implicit model proxies and setup endpoint validation: **DUET-2026-033–035**. Offline diagnostics also skip network-enabled MCP servers. | Reassess configured extensions and local endpoint trust in the deployment environment. The Linux runtime checks below cover ARM64; x86-64 seccomp still needs runtime verification. |
| Recoverable runs and inspectable audits | Tests cover termination, interruption, resume and hash-chain verification. The new interruption regression checks that derived-file classification survives reopening the run. [Runtime tests](../crates/duet-cli/tests/termination.rs), [audit implementation](../crates/duet-boundary/src/audit.rs) | Operational retention, access control and independent custody of audit anchors. A valid chain verifies recorded integrity, not whether its contents were safe to disclose. |
| GPL source and binary distribution | License grant, retained upstream notices, dependency policy and scripts for matching vendored source accompany the release procedure. An isolated source archive rebuilt successfully with Cargo's frozen mode. [Licensing and distribution](../LICENSES.md) | For a binary release, package from the final clean commit, sign and verify artifacts, and publish matching source and notices alongside the binary. |

## Verification for this publication review

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

Before making the source preview public:

- Configure and test a private vulnerability-reporting route. The website is a
  placeholder; it does not accept reports. `docs/security.txt` is an undeployed
  template until a working contact is supplied.
- Publish the CLA required by the existing [contribution policy](../CONTRIBUTING.md).
  Outside code contributions remain closed until then.
- Record the published revision and keep its license, provenance and verification
  records with it. For a binary release, follow [LICENSES.md](../LICENSES.md) and
  distribute the matching vendored source and notices alongside the binary.

The placeholder is deployed on [Cloudflare Pages](https://duet-site-9oe.pages.dev).
`duet.priezjev.com` is registered with Pages but still requires a proxied CNAME:
`duet` → `duet-site-9oe.pages.dev`. The deployment login has no DNS-write permission.

Builds used Rust 1.97.1 on macOS and 1.99 on Linux. Dependency manifests were
checked against the declared Rust 1.90 minimum; this review did not build with
Rust 1.90 itself.

Before real-data adoption in a bank or government team, evaluate representative
tasks and threat scenarios, approve local and frontier endpoints, and establish
policy administration, audit custody and incident response. The
[institutional evaluation guide](INSTITUTIONAL_EVALUATION.md) sets out that work.
These are deployment decisions; neither the project name “top clearance” nor a
passed test suite is a security accreditation.
