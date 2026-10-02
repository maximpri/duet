# Fresh dogfood: code, boundary and audit in one terminal

**Run `20261002-031043-4ce452`: 4/4 tests passed; zero matches for 13 complete planted values in five recorded frontier requests.** The coding turn took **29.9 seconds**. This is a small real run, not a paired benchmark or proof against every disclosure.

Recorded October 1, 2026, America/Toronto (October 2 UTC), while preparing `launch/duet-evidence-campaign`. All customer records and the database password are fictional. The [earlier failed and fixed runs](DEMO.md) remain published separately.

![Actual Duet terminal run, privacy inspection and audit verification](../assets/launch/duet-boundary.gif)

[Native MP4](../assets/launch/duet-boundary.mp4) · [Original compressed PTY recording](../evidence/launch-refresh-2026-10-01/hybrid.cast.gz) · [Edit map](../evidence/launch-refresh-2026-10-01/reel-edit.json) · [Manifest and hashes](../evidence/launch-refresh-2026-10-01/manifest.json)

## What happened

The same [billing fixture](billing-demo/README.md) began with one failing test. Its substring status check included inactive accounts. Duet read the code and customer CSV through the boundary, changed the comparison to equality, ran the tests and passed the required acceptance check. The final diff changes one line.

| Inspect | Saved artifact |
| --- | --- |
| Input and configured task check | [Run metadata](../evidence/launch-refresh-2026-10-01/run.json) |
| Actual patch | [Diff](../evidence/launch-refresh-2026-10-01/billing-fix.patch) |
| Before and after correctness | [Initial failing tests](../evidence/launch-refresh-2026-10-01/before-tests.txt), [four passing tests](../evidence/launch-refresh-2026-10-01/after-tests.txt) |
| Prepared frontier requests | [Original audit JSONL](../evidence/launch-refresh-2026-10-01/hybrid-audit.jsonl) |
| Complete planted-value checks | [Canary report](../evidence/launch-refresh-2026-10-01/canary-check.json) |
| Record integrity | [CLI verification](../evidence/launch-refresh-2026-10-01/audit-verify.txt), [independent script](../evidence/launch-refresh-2026-10-01/audit-check.json), [anchor](../evidence/launch-refresh-2026-10-01/hybrid-anchor.json) |
| Model output, timing and accounting | [Transcript](../evidence/launch-refresh-2026-10-01/transcript.jsonl), [economics](../evidence/launch-refresh-2026-10-01/economics.json) |

The privacy inspector shows the source classified as sensitive and the outbound summary with amounts replaced by `⟨withheld:data-value⟩`. Schema, row count, status vocabulary and a customer-ID pattern remain visible. **Zero full-value matches is not zero information disclosure.** The private data's structure is deliberately useful context in hybrid mode.

The model's final explanation incorrectly gives “reactivated” as a substring-match example. The fixture demonstrates **inactive**, and the patch/tests establish the correction. The recording is unaltered; the model's claims about either correctness or privacy are not the verification evidence.

## Real screenshots

| Privacy decision | Prepared outbound text |
| --- | --- |
| ![Real TUI privacy decision for the sensitive customer CSV](../assets/launch/duet-fresh-privacy.png) | ![Real TUI outbound record with customer amounts withheld](../assets/launch/duet-fresh-outbound.png) |

[Full code-change screenshot](../assets/launch/duet-fresh-changes.png) · [Full audit-verification screenshot](../assets/launch/duet-fresh-audit.png). Open the full image for readable terminal text.

## Verify without a model or API key

From the repository root:

```sh
python3 tools/check-launch-canaries.py \
  docs/evidence/launch-refresh-2026-10-01/hybrid-audit.jsonl \
  docs/launch/billing-demo

python3 tools/verify-launch-audit.py \
  docs/evidence/launch-refresh-2026-10-01/hybrid-audit.jsonl \
  docs/evidence/launch-refresh-2026-10-01/hybrid-anchor.json
```

Both exit 0. The first checks complete fixture values in literal, base64, hex and URL forms against recorded request bodies. It does not cover all encodings, fragments, paraphrases, every network path or provider receipt. The second checks this packet's chain/body hashes and supplied anchor; bundled artifacts do not establish independent custody. The [older tampered-copy demonstration](DEMO.md#verify-the-log-then-detect-a-changed-copy) supplies the negative control, with original failed disclosure checks retained too.

## Reproduce the coding task

Follow the [fixture setup](DEMO.md#reproduce-the-task) with your own approved endpoints. Use the objective in the saved [run metadata](../evidence/launch-refresh-2026-10-01/run.json). Results and timing can differ across runs. Inspect the output and run the independent checks rather than relying on the assistant's final message.

The capture binary was copied from the earlier launch build and verified against its published SHA-256: `44703f8c42fffa1b607420fd17b731c5166f74283a7b8721f6fda52992782e14`, source revision `c9e1d5aaa427ad19b4650529f73079cbfe81c973`. The refreshed documentation branch starts at `6a751e2`. This capture does not silently stand in for a build of every later source change.

## Model, transport and cost limits

- Frontier: configured Z.ai endpoint, `glm-5.3-flash` provider identifier.
- Local: `omlx-coding` alias on the owner's explicitly allowed **plaintext LAN endpoint**. The underlying weights were not independently verified. Synthetic content left the workstation for that server. This is not same-device or encrypted-transport evidence. Use approved loopback inference or a secured self-hosted path for a real deployment.
- One local digest request; five frontier requests; nine audit records. The hybrid frontier audit does not itself capture the local model's raw request body.
- Modeled frontier cost: **$0.00146704**, using saved usage and the configured price catalog. Local token rates were zero; local hardware, energy and operator time were not included. This is not a provider invoice or a cost-saving comparison.
- Command networking was off; subagents disabled; task/session frontier caps were $0.30/$0.60.
- The recorded binary contains an older phrase “on this machine” in a handle description. Read it in light of the actual LAN endpoint; it is not a transport guarantee.

## Rebuild the media

The recording is actual output from a controlling PTY, rendered by xterm.js 5.5.0 at 130×36 cells in Menlo 16. The 20-second GIF shows 60 selected frames at 3 fps; its MP4 contains the same sequence at 30 fps. It opens with the outcome, replays the working task, then shows operator inspection. Idle pauses are removed and key states held. **No screen text is synthesized or replaced.** Playback length is not task duration.

Use Node, Python 3, ffmpeg, Chrome, `@playwright/test@1.58.2` and `@xterm/xterm@5.5.0`. Install the Node dependencies into a separate temporary directory and point `NODE_PATH` at its `node_modules`:

```sh
NODE_PATH=/path/to/capture/node_modules python3 tools/build-launch-reel.py \
  --xterm /path/to/capture/node_modules/@xterm/xterm \
  --chrome '/path/to/Google Chrome' \
  --output /tmp/duet-rebuilt-media
```

The [builder](../../tools/build-launch-reel.py) decompresses the saved cast, applies the exact [frame-selection map](../evidence/launch-refresh-2026-10-01/reel-edit.json), renders genuine terminal states and encodes GIF/MP4. Fonts, browser and encoder versions can change pixel hashes; the original recording and source timestamps identify the content.
