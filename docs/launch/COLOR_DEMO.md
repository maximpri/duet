# Duet in Terminal

The README GIF is made from **direct macOS screenshots of Duet running in Terminal.app**, including the native window frame. The PNGs below are the original `screencapture` output. No browser renderer, terminal replay, generated screen or text replacement was used for these images.

| Sensitive-file handling | Outbound record |
| --- | --- |
| ![Duet running in Terminal: sensitive-file handling](../assets/launch/duet-native-privacy.png) | ![Duet running in Terminal: outbound record](../assets/launch/duet-native-outbound.png) |

| Filtered local answer | Code change |
| --- | --- |
| ![Duet running in Terminal: private amounts withheld from the local answer](../assets/launch/duet-native-summary.png) | ![Duet running in Terminal: billing code diff](../assets/launch/duet-native-changes.png) |

Open a PNG to inspect the full-size screenshot. The GIF holds each of these four screenshots for five seconds, in order. It is a review of a completed session, not a recording of the time spent coding.

## Source

Captured October 2, 2026 in Terminal.app 2.15, Pro profile, Menlo 16, with `NO_COLOR` unset. Duet reopened a disposable copy of session `20261002-031043-4ce452`. No new model requests were made. The capture binary's hash and each image's hash are in the [manifest](../evidence/readme-native-2026-10-02/manifest.json).

The original task passed **4/4 tests**, with **zero matches for 13 planted values in five recorded frontier requests**. Those results come from the [original run and independent checks](FRESH_DOGFOOD.md), not from the model's on-screen explanation.

The fixture contains fictional data. Its local model endpoint was an owner-allowlisted plaintext LAN server; the screenshot retains that notice. The model's original explanation also retains its incorrect “reactivated” example—the test fixture concerns “inactive”. The [source run documentation](FRESH_DOGFOOD.md) explains both. Reopening the disposable copy adds lifecycle events to that copy; the published original audit is unchanged.

## Capture and rebuild

With Duet running in a native Terminal window, capture that window directly:

```sh
screencapture -x -o -l <window-id> screenshot.png
```

The four committed PNGs are unchanged capture output. Rebuild the GIF from them with ffmpeg, from the repository root:

```sh
ffmpeg -hide_banner -y -safe 0 -f concat \
  -i docs/evidence/readme-native-2026-10-02/frames.ffconcat \
  -filter_complex '[0:v]split[a][b];[a]palettegen=stats_mode=full[p];[b][p]paletteuse=dither=bayer:bayer_scale=3' \
  -fps_mode vfr -t 20 -final_delay 500 -loop 0 /tmp/duet-security.gif
```

The earlier xterm.js replay has been [archived separately](../evidence/readme-color-2026-10-02/manifest.json). It is not used by the README.
