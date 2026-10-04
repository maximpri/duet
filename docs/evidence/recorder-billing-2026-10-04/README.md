# Recorded billing demo, October 4, 2026

The README's demo GIF. Recorded with [`tools/duet-recorder`](../../../tools/duet-recorder/README.md)
from [`scenes/billing.json`](../../../tools/duet-recorder/scenes/billing.json): the real `duet`
binary ran in a pseudo-terminal on a fresh copy of the [synthetic billing fixture](../../launch/billing-demo/README.md),
with `glm-5.3-flash` as the frontier model and `omlx-coding` as the local model.

| Check | Result |
| --- | --- |
| Tests before the session | 1 of 4 failing ([output](check-before.txt)) |
| Tests after the session | 4 of 4 passing ([output](check-after.txt)) |
| Code change | One line in `billing.py` ([patch](change.patch)) |
| Planted values in recorded frontier requests | **0 matches** for 13 complete values in 5 requests ([report](canaries-20261004-231159-c47c47.json)) |
| Session cost shown by Duet | $0.0016 (frontier tokens at catalog rates; local model at zero) |

[`session.cast.gz`](session.cast.gz) holds every byte Duet wrote to the terminal, the typed input and the
scene markers (asciicast v2). [`manifest.json`](manifest.json) records the Duet version and binary hash,
the scene and cast hashes, the run ID and the playback timing.

The [GIF and MP4](../../assets/demo/) replay those bytes in xterm.js. No screen text was generated or
edited. While the agent works, playback runs at 6× speed, and the window says so. Pauses longer
than 1.5 seconds outside scene holds are shortened. Playback length is not task duration.

The fixture's names, emails, amounts and database password are fictional. The planted-value check
looks for complete values in literal, base64, hex and URL-encoded forms in the recorded request
bodies. It does not detect fragments, paraphrase or inference, and it does not observe the network.
The local model runs on the maintainer's development LAN host. For the recording, the recorder
relayed it through a loopback port and gave Duet a throwaway config pointing there, so the
screen shows `127.0.0.1` rather than the development network. The relay forwards bytes
unchanged: the hop from this machine to that host was still plain HTTP on the LAN
(`local_model_relay` in the [manifest](manifest.json)).

Re-render from the cast:

```sh
gunzip -k docs/evidence/recorder-billing-2026-10-04/session.cast.gz
python3 tools/duet-recorder/record.py --render-only docs/evidence/recorder-billing-2026-10-04/session.cast --out /tmp/duet-rerender
```
