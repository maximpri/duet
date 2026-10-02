# Launch validation and limits

Worktree: `duet-launch`; branch: `launch/privacy-first-story`. The original working tree and index were preserved. Its pre-existing development work was snapshotted as `a36cee7`; privacy fixes and corrected top-clearance banners are in `c9e1d5a`. The live successful captures use the release binary from that revision. Later edits clarify handle/tool descriptions and audit-storage documentation; the recorded bytes have not been rewritten.

## Code checks

| Check | Result | Saved evidence |
| --- | --- | --- |
| Release CLI build used for captures | Passed | [Build](../evidence/launch-2026-10-01/build.txt) |
| Boundary unit tests after both fixes | 263 passed | [Log](../evidence/launch-2026-10-01/boundary-tests.txt) |
| Boundary unit tests plus dialect, outbound-agreement and structure integration tests after wording clarification | 269 passed | [Final log](../evidence/launch-2026-10-01/boundary-final-tests.txt) |
| CLI outbound gate | 4 passed | [CLI log](../evidence/launch-2026-10-01/cli-tests.txt) |
| CLI privacy scenarios, including new transport regression | 24 passed | [CLI log](../evidence/launch-2026-10-01/cli-tests.txt) |
| CLI secure defaults | 4 passed | [CLI log](../evidence/launch-2026-10-01/cli-tests.txt) |
| CLI top clearance | **4 passed, 1 failed** | [CLI log](../evidence/launch-2026-10-01/cli-tests.txt) |
| Configuration / sandbox / sandbox networking | 25 / 25 / 5 passed | [Log](../evidence/launch-2026-10-01/config-sandbox-tests.txt) |
| Workspace Clippy, all targets, warnings denied | Passed | [Log](../evidence/launch-2026-10-01/clippy.txt) |
| Fast gate: formatting, license, dependency-boundary and provenance checks | Passed | [Log](../evidence/launch-2026-10-01/fast-gate.txt) |

That is **360 distinct targeted tests passed and one failed**, without double-counting reruns. The full workspace test/dependency gate was **not** run to completion and this branch is not claimed to have passed it.

The failing test is `frontier_alias_uses_openrouter_price_and_unknown_models_stop_before_a_request` in `top_clearance.rs`. Its known-alias case stops with “no usable token price for private-alias,” despite an explicit `pricing.frontier_model` setting. It failed before the launch fixes and again with an isolated build target afterward. Its underlying cause was not resolved here. The four actual top-clearance behavior tests pass; the pricing failure remains a release issue, not a hidden green check. [Initial test output](../evidence/launch-2026-10-01/initial-cli-tests.txt).

Reproduce the targeted checks with an isolated Cargo target directory if another checkout is building concurrently:

```sh
export CARGO_TARGET_DIR=/tmp/duet-review-target
cargo test --release --locked -p duet-boundary --lib \
  --test structure_views --test outbound_agreement --test dialect_gate
cargo test --release --locked -p duet-cli --test outbound_gate \
  --test privacy_scenarios --test secure_defaults --test top_clearance
cargo test --release --locked -p duet-config -p duet-sandbox
cargo clippy --release --workspace --all-targets --locked -- -D warnings
tools/gate.sh --fast
```

The new regression was observed failing before its fix. The live task then found the second short-value disclosure; both failures and the final canary check are in [the demo packet](DEMO.md). Tests are examples of enforced behavior, not a formal or exhaustive proof.

## Artifact checks

- Both final live tasks passed four acceptance tests. The independent hybrid checker returned zero matches; the two negative runs returned 36 and 9.
- Duet's CLI verified the original chain and external anchor. A modified last record in a copy caused exit 1. The standalone packet checker agrees: original matches, modified head differs.
- GIFs and MP4s were generated from saved real PTY output. Selected frames, screenshots and the rendered README were visually inspected. The edit maps identify source recording timestamps, held frames and playback rate.
- Repository-relative links in the new README/launch/security documents were checked. Public post links target a future reviewed public revision; they must be verified after merge.
- The prepared evidence/media were checked for exact values of the credential environment variables used by the local/frontier endpoints. Only fictional fixture credentials are intended for publication. This is a targeted check, not an exhaustive secret-detection guarantee.

No posts, outreach, GitHub push or release publication were performed. [The campaign](CAMPAIGN.md#before-a-broad-launch) identifies remaining work before a broad security-led launch.
