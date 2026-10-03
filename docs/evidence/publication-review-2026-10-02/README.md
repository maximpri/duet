# Publication review evidence

Verification of the source changes based on commit
`f7eb3e894dcc54e662d20e9625a44c5e3e96fe23`. The [manifest](manifest.json) records
commands, results, log hashes and the changed source-file hashes. This is a
development-preview review, not a signed production release or accreditation.

| Check | Retained result |
| --- | --- |
| macOS ARM64 workspace gate | [Log](macos-gate.log.gz): formatting, Clippy, 1,235 tests passed; 22 environment-dependent tests ignored; dependency, license-header, privacy, egress and provenance checks passed. |
| Linux ARM64 sandbox matrix | [Log](linux-sandbox.log.gz): 476 tests passed and 12 ignored in each privileged, unprivileged and root mode. The registry-network integration passed in privileged and unprivileged modes. Three namespace-denial tests passed. |
| Final Linux regressions | [Log](linux-regressions.log.gz): proxy regression (12 isolated subprocess cases), 17 setup/doctor tests and 3 release-script tests passed; none ignored. This run includes the final proxy fix and dependency update. |
| Vendored source build | [Log](offline-build.log.gz), [archive and toolchain record](../licensing-2026-10-02/offline-source-validation.json): a frozen release build succeeded with an empty Cargo home. Every product source file and Cargo manifest/lock matched the reviewed tree. |
| Evaluation packages | `duet-eval validate` passed for all 13 task packages and seals. `duet-eval selftest` passed for canaries, proxy, statistics and pricing. |

Logs are gzip-compressed. Only machine-specific workspace, user-home and external
cache paths were replaced; the manifest retains both original and normalized
hashes. Read a log with `gzip -dc <file>.log.gz`.

The Linux sandbox matrix preceded the model-proxy fix and `yoke-derive` update;
the final Linux regressions and macOS gate ran after both changes. The frozen
build used a fresh Cargo home and an existing build-artifact cache. A mismatched
local Xcode/Command Line Tools SDK required explicit matching child-process
`DEVELOPER_DIR` and `SDKROOT`; no global settings changed.

The native README captures retain their original pixels and metadata. Their
[separate manifest](../readme-native-2026-10-02/manifest.json) records source and
image hashes. The review checked changed text, four retained logs and 62 image
frames for accidental credentials. The visible local username and documented
LAN endpoint remain in the screenshots. This was a scoped publication check,
not a scan of the repository's entire history.

The placeholder's deployed assets were compared byte-for-byte with `site/public`
and visually checked at desktop and mobile widths. Its Pages address is
https://duet-site-9oe.pages.dev; `duet.priezjev.com` still needs its CNAME record.
See [publication readiness](../../PUBLISH_READINESS.md) for remaining work.
