---
name: code-review
description: Review a diff or selected code for concrete correctness, security, and performance regressions. Use when the operator asks for a code review or a review before merging.
---

# Review code changes

Use the operator's requested files or comparison range. If none is given, review
the uncommitted changes, preserving edits already in the workspace. If there are
no changes, report that and ask for the intended scope.

Read the relevant instruction files and the changed code's callers and tests.
Follow data across trust boundaries when it can reach a command, network request,
file path, credential, or public response. For performance concerns, identify the
repeated work and the conditions that make it costly before proposing a change.

Report defects with an observable consequence, a specific path and line, and the
input or execution path that triggers them. Check apparent defects against nearby
guards and existing tests. Label uncertainty when evidence is incomplete; avoid
turning preferences about style into correctness findings.

Choose the smallest relevant existing check or reproduction that can resolve a
finding. Report the command and its actual outcome. A check that could not run is
unverified. Do not change files during a review unless the operator also requested
fixes; in that case, make focused corrections and verify their affected behavior.

Present findings in severity order, then list checks performed and any material
coverage gaps. If no actionable findings remain, say so without claiming the code
is secure or defect-free. Stop when the requested scope is reviewed and supported
findings and verification limits are reported.
