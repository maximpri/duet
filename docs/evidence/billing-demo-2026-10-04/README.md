# Billing demo: actual Duet session, October 4, 2026

Run `20261004-152905-6020a9` used the unchanged [fictional billing fixture](../../launch/billing-demo/README.md) in a disposable workspace. Duet read the sensitive CSV, repaired the billing bug in one line and passed all four tests. The original fixture remains deliberately broken for future demos.

## Screenshots

Actual Duet session using fictional customer data. These are screenshots of the running application's PTY displayed by the repository's [live xterm.js viewer](https://github.com/maximpri/duet/blob/657b8a40d537b64fd81e063d26f1bd93e8caaa31/tools/capture-tui.py) (since replaced by [`tools/duet-recorder`](../../../tools/duet-recorder/README.md)), captured through the Codex in-app browser. They are not native Terminal.app captures. The JPEGs are unchanged browser screenshot output; no TUI text was reconstructed, substituted or retouched.

| Screenshot | What it shows |
| --- | --- |
| [Completed task](../../assets/billing-demo/04-completed.jpg) | Four passing tests, completed acceptance checks and the Changes panel; the conversation is scrolled up three rows to include the full inline patch. |
| [Privacy](../../assets/billing-demo/02-privacy.jpg) | The selected `data/customers.csv` event and how its filtered result was handled. |
| [Task in progress](../../assets/billing-demo/01-working.jpg) | The supplied prompt, hybrid mode and the sensitive CSV read while the task was running. |
| [Expanded outbound record](../../assets/billing-demo/03-outbound.jpg) | `Ctrl-O` on that CSV event: audit record 6, model, handling decision and beginning of the recorded outbound view. Scroll in Duet to read the rest. |

The terminal was 140 columns by 44 rows, using xterm.js 5.5.0 and Menlo 16. The browser viewport was 1440 × 990. [The original PTY recording](session.cast.gz) preserves output from startup through normal exit; it was not edited into a replay for these screenshots. [The manifest](manifest.json) records the binary, capture settings and artifact hashes.

## Result and verification

| Check | Evidence |
| --- | --- |
| Supplied task and acceptance command | [Run metadata](run.json) |
| Before | [One failing test out of four](before-tests.txt) |
| Fix | [One-line patch](billing-fix.patch): replace substring matching with `row["status"] == "active"`. |
| After | [All four tests pass](after-tests.txt); the TUI also reports successful acceptance checks. |
| Complete planted-value matches | [Zero matches for 13 values in four frontier requests](canary-check.json) |
| Recorded CSV content | [Exact tool-result text from audit record 6](csv-outbound.txt), extracted without edits from the [full audit](audit.jsonl). |
| Audit integrity | [CLI verification](audit-verify.txt), [independent check](audit-check.json) and [matching anchor](anchor.json): nine records after normal exit. |
| Task history | [Transcript](transcript.jsonl), [summary](summary.json) and [disclosure counts](disclosure.txt) |
| Settings | [Project configuration](project-config.toml), [privacy preview](privacy.txt) and [request list](audit-show.txt) |

The four recorded model requests all target the frontier. One local digest request is accounted for separately in [economics](economics.json) and the summary. The coding turn took 33.14 seconds; later screenshot navigation is not coding time. Normal TUI exit saves the session as `open` so it can be resumed; the turn had already finished and its checks passed.

The outbound CSV view contains field names, counts and shapes, a generated sample, and a filtered local-model summary. It retains the status labels and a customer-ID pattern. Original monetary examples in the summary are replaced with `⟨withheld:data-value⟩`. The complete-value check covers fictional customer IDs, names, emails, amounts and the fixture password in literal, base64, hex and URL-encoded forms. It does not detect every fragment, encoding, paraphrase or inference, and neither the audit nor a screenshot establishes provider receipt. The model's explanation is not the privacy verification.

The frontier identifier was `glm-5.3-flash`; the local alias was `omlx-coding`. The configured local endpoint was an owner-approved plaintext LAN service, so raw synthetic data left the workstation for that local service. “Local” in the TUI describes that configured endpoint, not same-device processing. Commands had networking disabled, web tools were disabled, and subagents were disabled. Only fictional data was used.

The model's explanation also says the substring check would match `reactivated`; that word does not contain `active`. The actual failing test concerns `inactive`. The saved patch and tests establish the correction.

## Verify offline

From the repository root:

```sh
python3 tools/check-launch-canaries.py \
  docs/evidence/billing-demo-2026-10-04/audit.jsonl \
  docs/launch/billing-demo

python3 tools/verify-launch-audit.py \
  docs/evidence/billing-demo-2026-10-04/audit.jsonl \
  docs/evidence/billing-demo-2026-10-04/anchor.json
```

Both exit 0. A bundled anchor establishes artifact consistency, not independent custody or authorship. [Reproduce the task](../../launch/DEMO.md#reproduce-the-task) with your own configured endpoints, using `duet --mode hybrid --check 'python3 -m unittest -v'` and the exact objective in `run.json`.
