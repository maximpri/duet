# Operating Duet with retained audit evidence

Duet keeps run data, audit logs and integrity anchors on the operator's machine.
These commands support local retention review, content-free audit exports and
health checks for an existing monitoring system. They do not contact a model or
send telemetry.

## Storage and retention

| Data | Location | Control |
|---|---|---|
| Raw requests, transcripts, handles, vault and write journal | Workspace `.duet/runs/<run>` | `data.retention_days`, default 14; explicitly remove with `duet purge` |
| Audit requests and security events | Workspace `.duet/audit/<run>.jsonl` | `data.audit_retention_days`, default 90; review with `duet audit retention` |
| Audit integrity anchors | Owner state directory `audit-anchors/` | Preserve separately from the workspace |
| Persistent derived-file classifications | Workspace `.duet/derived.json` and `.duet/derived.pending` | Preserved by purge; classification must remain while derived files exist |

Audit logs are sensitive storage. Hybrid requests contain what passed the
boundary; top-clearance requests can contain raw private material. Event fields
can include commands, paths and configuration values. `audit show --raw` is a
local investigation command, not a safe telemetry export.

Set the retention periods explicitly:

```sh
duet config set data.retention_days 30 --confirm
duet config set data.audit_retention_days 365 --confirm
duet audit retention
duet purge --dry-run
duet purge
```

The retention report uses both configured periods. It emits JSON and exits 1
when run data or audit logs are due for review, or a directory cannot be
inspected. It never removes data. Audit expiry is an **archive/review threshold**:
there is no automatic audit deletion. Decide where to preserve the full logs
and anchors under your organization's retention and access rules. A metadata
export cannot replace the original evidence or verify its chain.

Raw-run expiry uses the latest modification anywhere inside the run, including
nested transcript files. An old directory timestamp alone does not make an
active transcript eligible. Purge refuses an active workspace lock and refuses
symlinked run directories. Close the session before purging. `duet purge RUN_ID`
and `duet purge --all` intentionally remove selected raw data regardless of age;
add `--dry-run` to preview either. A project may shorten the raw retention
period, but only the owner controls audit retention.

Purge retains audit logs, owner anchors and persistent derived-file
classifications. It never turns deletion of raw history into declassification of
private output files. Review access to backups and filesystem snapshots
separately; removing a file is not a secure erase of those copies.

The owner state directory is `$DUET_CONFIG_HOME/state` when configured,
otherwise `$XDG_STATE_HOME/duet` or `~/.local/state/duet`. Keep its permissions and
backups separate from the workspace's. The anchors are local tamper evidence;
they do not protect against an operator or administrator who can rewrite both
the log and the owner state.

## Export metadata without request content

```sh
umask 077
duet audit export RUN_ID > audit-metadata.json
```

Export first verifies the exact snapshot against its hash chain and owner
anchor. It returns 0 only for an intact, fully anchored snapshot. A failed
verification returns 1 with an empty record list and a fixed status label.
Unknown or malformed records fail closed rather than being silently skipped.

Schema version 1 contains:

- `schema_version` and the fixed `kind` label `duet_audit_metadata`.
- `run`: a SHA-256 reference to the run ID, integrity status, record and byte
  counts, unanchored count and first/last timestamps.
- `records`: sequence number, timestamp, fixed request/event kind and admitted
  numeric or Boolean metrics. Request metrics are serialized body size and
  intervention count. Event metrics include counts, sizes, elapsed time, cost,
  exit code and approval/review flags when present.

No stored string field is copied from a request or event. The allowlist omits
prompts, bodies, tool arguments, commands, raw URLs, hostnames, model names,
paths, configuration values, free-text reasons, user-supplied labels and content
digests. Event kinds come from Duet's fixed enum; metrics accept only named
numeric/Boolean fields. New event fields are not automatically exported.

Timestamps, costs, activity counts and stable run references can still be
operationally sensitive. The output minimizes content disclosure; it is not an
anonymity guarantee. The run reference is `SHA256(UTF-8 run ID)` and can be
correlated with the local history when investigating an alert.

## Check integrity for an external alert

```sh
duet audit check
duet audit check RUN_ID
```

The workspace-wide command reads audit logs, raw-run directory names and the
owner anchor inventory. It can therefore report a missing audit file even when
its raw run directory has already been purged. The report uses fixed status
labels and hashed run references; it does not print raw errors, paths or event
content.

| Exit | Meaning |
|---|---|
| 0 | Every discovered or explicitly requested log is intact and fully anchored |
| 1 | Missing log/anchor, broken chain, rewritten or truncated history, unanchored tail, incomplete record, changing file, unsafe path, unreadable state or size/inventory limit |
| Other nonzero | Command invocation or operational failure; treat as an alert too |

An unanchored tail is reported even when its existing prefix matches the
anchor. It can result from an interrupted append. Reports do not repair logs,
re-anchor them, truncate partial records or overwrite evidence. Inspect locally
with `duet audit verify RUN_ID` and preserve the original files before recovery.
Concurrent writes can produce a transient `changed_during_check` or unanchored
status; repeat after the run has settled before treating it as tampering.

A snapshot is limited to 64 MiB; each anchor to 64 KiB; an inventory to 100,000
entries. Exceeding a limit produces nonzero status, not a healthy partial
report. Invalid owner anchor entries make the inventory incomplete and require
attention. Symlinks and nonregular audit files are refused.

An empty workspace with no known anchors has no runs to check. If your monitor
expects a particular run, pin its ID with `audit check RUN_ID`; it fails even
when both its log and anchor are absent. After moving a workspace, existing logs
still match portable anchors. An already-missing log cannot always be assigned
to the new workspace from its old recorded path; use the expected run ID in
that case. A monitor needs its own retained inventory to detect deletion of
both the evidence and its local baseline.

For a scheduler, save JSON privately and preserve the command's exit status:

```sh
#!/bin/sh
set -eu
umask 077
directory="$HOME/.local/state/duet-monitor"
mkdir -p "$directory"
temporary="$(mktemp "$directory/audit-health.XXXXXXXX")"
trap 'rm -f "$temporary"' EXIT HUP INT TERM
status=0
/absolute/path/to/duet audit check --workspace /path/to/workspace > "$temporary" || status=$?
mv "$temporary" "$directory/audit-health.json"
exit "$status"
```

Run this script from your scheduler and connect nonzero exit status to the
alerting system you already operate. Schedule `audit retention` separately to
track expiry review. Duet does not install a scheduler, transmit these reports,
manage organizational identities or replace a centralized evidence store.

After a hard kill or failed classification write, `.duet/derived.pending` makes
hybrid startup and preview refuse to continue. Review every potentially written
file locally, classify affected paths in the sensitivity policy or move/remove
the outputs, and preserve existing `.duet/derived.json` entries. Only after that
owner review should you remove the pending marker. Deleting the classification
manifest to unblock a run can declassify sensitive output.
