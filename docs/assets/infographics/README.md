<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Duet figures

Each figure comes in a light and a dark variant drawn on GitHub's page colours. Pages pick one with
`<picture>`.

| Figure | Light | Dark |
| --- | --- | --- |
| How Duet handles a task | [duet-flow-light.svg](duet-flow-light.svg) | [duet-flow-dark.svg](duet-flow-dark.svg) |
| Hybrid and local-only modes | [duet-modes-light.svg](duet-modes-light.svg) | [duet-modes-dark.svg](duet-modes-dark.svg) |
| Benchmark: test scores and planted secrets | [duet-results-light.svg](duet-results-light.svg) | [duet-results-dark.svg](duet-results-dark.svg) |
| Benchmark: every run of every task | [duet-results-by-task-light.svg](duet-results-by-task-light.svg) | [duet-results-by-task-dark.svg](duet-results-by-task-dark.svg) |

[`tools/render-infographics.py`](../../../tools/render-infographics.py) writes all of them. It needs
only Python 3 and has no dependencies. The two benchmark figures are computed from the
[October 4 benchmark report](../../evidence/benchmark-54-2026-10-04/report.json). The script refuses to
run unless the report matches the hash in its
[publication verdict](../../evidence/benchmark-54-2026-10-04/audit-verdict.json).

```sh
python3 tools/render-infographics.py           # regenerate
python3 tools/render-infographics.py --check   # confirm the committed files are current
```

The two series colours (Duet teal, no-protection amber) were checked on each background for
lightness, chroma, colour-vision separation and contrast. Each benchmark figure also labels its
series with text, so the colours are never the only cue. The same numbers are available as text
in the [visual guide](../../DUET_VISUAL_GUIDE.md#results-by-task) and the
[evidence overview](../../evidence/benchmark-54-2026-10-04/README.md).
