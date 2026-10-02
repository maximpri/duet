# Privacy review in color

![Native Duet colors while inspecting a completed billing task](../assets/launch/duet-security.gif)

This 18-second clip reviews a copy of completed session `20261002-031043-4ce452`: the customer CSV's privacy decision, its outbound record, a credential-filtering record and the code diff. It makes no new model calls. The original task's **4/4 passing tests** and **zero matches for 13 planted values in five frontier requests** come from the [original recording and independent checks](FRESH_DOGFOOD.md).

The original capture inherited `NO_COLOR=1`. On October 2, 2026, the same capture binary was used to reopen a disposable copy of that synthetic workspace with `NO_COLOR` unset. The new recording contains Duet's native ANSI colors. No terminal text was recolored, synthesized or replaced. Selected states are held for readability; playback length is not task duration.

[Original color PTY recording](../evidence/readme-color-2026-10-02/review.cast.gz) · [Frame selection](../evidence/readme-color-2026-10-02/reel-edit.json) · [Manifest](../evidence/readme-color-2026-10-02/manifest.json)

The source run used an owner-allowlisted plaintext LAN model endpoint and fictional data. The recording retains its endpoint notice and the model's original prose, including its incorrect “reactivated” example; the fixture's bug concerns “inactive”. See the [original run's scope and checks](FRESH_DOGFOOD.md) for the evidence behind the result. Reopening a copied session adds lifecycle events to that copy; the published original audit remains unchanged.

## Rebuild

Use the dependencies described in the [original media guide](FRESH_DOGFOOD.md#rebuild-the-media), then run from the repository root:

```sh
NODE_PATH=/path/to/capture/node_modules python3 tools/build-launch-reel.py \
  --xterm /path/to/capture/node_modules/@xterm/xterm \
  --chrome '/path/to/Google Chrome' \
  --evidence docs/evidence/readme-color-2026-10-02 \
  --name duet-security \
  --output /tmp/duet-color-media
```

For future captures, run `tools/capture-tui.py` with `env -u NO_COLOR` so the child terminal can emit colors. The builder replays the recorded bytes with xterm.js and preserves their colors through GIF palette generation. Browser, font and encoder versions can change pixel hashes.
