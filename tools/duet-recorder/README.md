# Duet recorder

Records a **real** Duet session from a script and turns it into a README-ready GIF, an MP4 and
still PNGs.

```sh
python3 tools/duet-recorder/record.py --install-deps   # once: xterm.js + Playwright into ~/.cache/duet-recorder
python3 tools/duet-recorder/record.py                  # record and render the billing demo
```

Output (default: a new `duet-recording-*` directory under `$TMPDIR`; choose one with `--out DIR`):

| File | What it is |
| --- | --- |
| `duet-demo.gif`, `duet-demo.mp4` | The rendered session |
| `completed.png`, `privacy.png`, `outbound.png` | Stills named by the scene's `still` steps |
| `session.cast` | Every byte Duet wrote, plus the typed input and scene markers (asciicast v2, mode 0600) |
| `manifest.json` | Duet version and binary hash, scene hash, cast hash, run IDs, test and canary results, playback speeds |
| `check-before.txt`, `check-after.txt`, `change.patch` | The task's tests before and after, and the code change |
| `canaries-<run>.json` | [Planted-value check](../check-launch-canaries.py) over every recorded frontier request |

## What is real

- The scene's fixture is copied into a fresh git workspace, and the actual `duet` binary runs
  there in a pseudo-terminal against the models in your owner config. Nothing is mocked.
- Keystrokes are typed into that terminal at a human pace. The recorder answers the two terminal
  queries Terminal.app answers (cursor position and device attributes), so Duet sees an
  ordinary terminal.
- Rendering replays the recorded bytes in xterm.js with Terminal.app's ANSI palette, Menlo and a
  macOS window frame. Screen text is never generated or edited.
- If your local model server is off loopback over plain HTTP (a development LAN host, say), the
  recording relays it through a loopback port and gives Duet a throwaway config pointing there, so
  your network address and the plain-HTTP notice stay out of the published media. Your owner
  config is untouched, and the manifest records the relay. `--no-local-relay` records the
  endpoint as configured.
- Only **playback time** changes. `speed` steps speed up the agent's work, and the window shows
  a `▸▸ N× speed` badge while they apply. Pauses longer than `--idle` seconds are shortened unless a
  `hold` step asked for them. Playback length is not task duration.

## Scenes

A scene is JSON: the fixture, terminal size, Duet arguments and a list of steps.
[`scenes/billing.json`](scenes/billing.json) is the billing demo.

| Step | Effect |
| --- | --- |
| `{"wait": "regex", "timeout": 60}` | Wait until the screen matches |
| `{"type": "text", "cps": 30}` | Type text |
| `{"key": "enter"}` or `{"key": ["tab", "ctrl-o"]}` | Press keys (`enter`, `tab`, `shift-tab`, `esc`, arrows, `ctrl-up`, `ctrl-a`…`ctrl-z`, `f1`…`f4`, `pageup`…) |
| `{"press": "ctrl-down", "until": "regex", "max": 20}` | Press a key until the screen matches (`"optional": true` continues if it never does) |
| `{"hold": 3}` | Pause, kept in full in playback |
| `{"sleep": 1}` | Pause, shortened like any idle time |
| `{"speed": 6}` | Play from here at 6× (shown in the window) |
| `{"still": "name"}` | Save the frame here as `name.png` |
| `{"start": true}`, `{"end": true}` | Trim playback; steps after `end` are off camera |
| `{"command": "\u0003"}` | Send raw input (off camera, e.g. to leave) |

Scene keys: `fixture` (directory, relative to the scene), `copy` (`{"target": "source"}` inside the
workspace, e.g. `.env` from `env.example`), `check` (the acceptance command, run before and after),
`canaries` (run the planted-value check), `args` (passed to `duet`), `cols`, `rows`, `title`.

## Options

`--duet PATH` (default `duet` on `PATH`) · `--no-local-relay` · `--render-only CAST` re-renders an existing cast ·
`--no-render` · `--fps 12` · `--idle 1.5` · `--scale 2` (pixel ratio) · `--gif-width 1200` ·
`--font-size 16` · `--name duet-demo` · `--chrome PATH` · `--node-modules DIR`

Needs Python 3.9+ with `pyte`, Node, ffmpeg, and Chrome or Chromium. A recording shows whatever
appeared on the terminal, including your model endpoints, so record only synthetic workspaces
for publication.
