# Coverage review — 2026-10-01

The previous complete gate passed 1,220 tests, with 22 intentionally ignored.
This pass adds seven tests chosen for failures that would affect real runs.
They exercise the provider transport, outbound privacy gate, operating-system
sandbox, agent tool dispatcher and CLI binary. Existing tests that cover
related helpers remain in place.

| Area | New behavior checked | Failure it would catch |
| --- | --- | --- |
| Provider retry | An extreme remote `Retry-After` cannot panic and the run deadline still ends the wait; an ordinary two-minute delay is honored | An untrusted HTTP header panicked `Duration` conversion or caused an unbounded wait |
| Outbound gate | A rejected image is not sent through Chat, Anthropic or Responses and the blocked-send audit contains only safe metadata | One dialect bypassed the image policy or wrote image material to the audit log |
| Sandbox | A long lived server process with a registry proxy route is refused before it runs or connects | A server inherited a proxy route it must not have |
| Agent tools | Ambiguous edit batches leave both files and the write journal intact; a valid edit still works afterward | Partial edits or false write history after a rejected batch |
| Agent privacy | A failed local-only command holds its output without appending public recovery details; an ordinary command receives a useful hint | Sensitive command output or failure metadata reached the frontier |
| CLI history | The built `duet history` command distinguishes current and exact legacy open sessions from real failures and closed sessions; search/detail work without rewriting transcripts | A resumable session looked failed or history changed saved evidence |

The provider regression was run against the prior code first and **failed**:
a valid integer header of `18446744073709551615` panicked during
`Duration::mul_f64`. The client now caps untrusted server-requested waits at
one hour before duration arithmetic. A run with an earlier deadline stops at
that deadline; ordinary two-minute instructions still wait two minutes. No
provider requests, API credentials or paid models were used for these tests.

The first complete gate also exposed an environmental assumption in the
existing macOS image-alias test: it treated any `TMPDIR` as if it were under
`/private`. The test now creates its fixtures directly under `/private/var/tmp`
and `/private/tmp`, the two aliases it is meant to verify. Its focused rerun
passed with `TMPDIR` on the external disk. Product image routing did not need
a change for this failure.

All seven targeted tests passed. The complete gate then passed with **1,227
tests passed, 0 failed, 22 ignored**. All 18 documentation-test targets,
formatting, Clippy, dependency policy, license headers, privacy and egress
construction, and provenance checks passed.

```sh
TMPDIR=/Volumes/EXT_DISK/duet_v2/test-tmp \
CARGO_TARGET_DIR=/Volumes/EXT_DISK/duet_v2/target-gate \
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true bash tools/gate.sh
```

Local gate log: `/private/tmp/duet-coverage-full-gate.log`.
SHA-256: `0514a12347f17efd7db7278456ad3b21a81b18679cfba800bb01a3efda1e5311`.
The increase from 1,220 to 1,227 counts new behavior checks, not a measured
line or branch coverage percentage. Live-service and manual benchmark cases
remain outside this local run.
