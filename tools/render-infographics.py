#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Render Duet's documentation figures as SVG, in light and dark variants.

    python3 tools/render-infographics.py           # write docs/assets/infographics/*.svg
    python3 tools/render-infographics.py --check   # verify the committed files are current

No dependencies. The results figures are computed from the verified October 4
benchmark report, which must match the hash in its publication verdict. The
light and dark variants use GitHub's page colours, so they sit on the page the
way text does; the README picks one with <picture>.
"""

import argparse
import hashlib
import json
import sys
from collections import defaultdict
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "docs/evidence/benchmark-54-2026-10-04"
OUT = ROOT / "docs/assets/infographics"

# Duet and the unprotected baseline: validated as a two-series palette
# (lightness, chroma, colour-vision separation and contrast) on each surface.
THEMES = {
    "light": dict(surface="#ffffff", card="#f6f8fa", ink="#1f2328", soft="#59636e", line="#d1d9e0",
                  duet="#00897a", base="#c85a1e", duet_tint="#e6f4f1", base_tint="#fbeee6"),
    "dark": dict(surface="#0d1117", card="#151b23", ink="#f0f6fc", soft="#9198a1", line="#3d444d",
                 duet="#16a596", base="#d2692c", duet_tint="#0f2a27", base_tint="#2d1b10"),
}
SANS = "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans', Helvetica, Arial, sans-serif"
MONO = "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, monospace"

TASKS = [  # benchmark order, with the names the evidence uses
    ("S1", "Configuration"), ("S2", "Crash diagnosis"), ("M1", "Billing export"),
    ("M2", "Data-subject export"), ("M3", "Hostile logs"), ("L1", "Ledger reconciliation"),
    ("L2", "Protected pricing"), ("X1", "SQL gateway"), ("X2", "Partner exports"),
]
# Why two Duet runs scored zero (docs/evidence/benchmark-54-2026-10-04/README.md).
ZERO_NOTES = {("M1", 1): "did not compile", ("X1", 3): "stopped at deadline"}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def load_report():
    raw = (EVIDENCE / "report.json").read_bytes()
    verdict = json.loads((EVIDENCE / "audit-verdict.json").read_text())
    assert verdict["status"] == "passed" and verdict["publication_ready"]
    assert sha(raw) == verdict["report_json_sha256"], "report.json differs from its verdict"
    report = json.loads(raw)
    assert len(report["runs"]) == 54
    return report


class Svg:
    def __init__(self, width, height, theme, title, desc):
        self.w, self.h, self.t = width, height, THEMES[theme]
        self.parts = [
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
            f'viewBox="0 0 {width} {height}" role="img" font-family="{SANS}">',
            f"<title>{escape(title)}</title><desc>{escape(desc)}</desc>",
            f'<rect width="{width}" height="{height}" fill="{self.t["surface"]}"/>',
            '<defs><marker id="arrow" viewBox="0 0 10 10" refX="8.5" refY="5" markerWidth="7" '
            f'markerHeight="7" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{self.t["soft"]}"/>'
            "</marker></defs>",
        ]

    def c(self, name):
        return self.t.get(name, name)

    def text(self, x, y, s, size=14, fill="ink", weight=400, anchor="start", mono=False, italic=False):
        family = f' font-family="{MONO}"' if mono else ""
        style = ' font-style="italic"' if italic else ""
        self.parts.append(
            f'<text x="{x}" y="{y}" font-size="{size}" font-weight="{weight}" fill="{self.c(fill)}" '
            f'text-anchor="{anchor}"{family}{style}>{escape(s)}</text>')

    def rect(self, x, y, w, h, fill="card", stroke="line", r=10, dash=False, width=1):
        d = ' stroke-dasharray="5 4"' if dash else ""
        self.parts.append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{self.c(fill)}" '
            f'stroke="{self.c(stroke)}" stroke-width="{width}"{d}/>')

    def line(self, x1, y1, x2, y2, stroke="line", width=1, arrow=False, dash=False):
        a = ' marker-end="url(#arrow)"' if arrow else ""
        d = ' stroke-dasharray="3 4"' if dash else ""
        self.parts.append(
            f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{self.c(stroke)}" '
            f'stroke-width="{width}"{a}{d}/>')

    def circle(self, x, y, r, fill, stroke="surface", width=2):
        self.parts.append(f'<circle cx="{x}" cy="{y}" r="{r}" fill="{self.c(fill)}" '
                          f'stroke="{self.c(stroke)}" stroke-width="{width}"/>')

    def render(self):
        return "\n".join(self.parts + ["</svg>"]) + "\n"


def card(s, x, y, w, h, title, lines, accent=None):
    s.rect(x, y, w, h, fill=f"{accent}_tint" if accent else "card", stroke=accent or "line",
           width=1.5 if accent else 1)
    s.text(x + 16, y + 28, title, 15, weight=600)
    for i, line in enumerate(lines):
        s.text(x + 16, y + 52 + i * 21, line, 13, "soft")


def flow(theme):
    s = Svg(960, 440, theme, "How Duet handles a coding task",
            "Code is shared with the frontier model as-is. Sensitive files are read only by your local "
            "model, which answers the frontier's questions. Every request to the cloud passes Duet's "
            "checks and is logged.")
    s.rect(24, 24, 612, 346, fill="surface", stroke="line", dash=True, r=14)
    s.text(44, 54, "Your machine", 13, "soft", 600)
    card(s, 48, 74, 200, 108, "Code", ["src/, tests/, docs/", "shared as written"])
    card(s, 48, 204, 200, 140, "Sensitive files", [".env, keys", "data/*.csv, databases", "logs/", "never sent raw"])
    card(s, 272, 204, 160, 140, "Local model", ["reads sensitive", "files and answers", "the frontier's", "questions"])
    card(s, 456, 74, 156, 270, "Duet", ["replaces secrets and", "personal data with", "placeholders", "",
                                         "checks every", "outgoing request", "", "logs exactly what", "was sent"],
         accent="duet")
    s.line(248, 128, 452, 128, "soft", 1.5, arrow=True)
    s.line(248, 274, 268, 274, "soft", 1.5, arrow=True)
    s.line(432, 274, 452, 274, "soft", 1.5, arrow=True)
    card(s, 724, 128, 212, 162, "Frontier model", ["Claude, GPT, Gemini, GLM…", "", "plans the work", "writes the code",
                                                     "asks for files and tools"])
    s.line(612, 190, 720, 190, "soft", 1.5, arrow=True)
    s.line(720, 236, 616, 236, "soft", 1.5, arrow=True)
    s.text(668, 166, "checked", 12, "soft", anchor="middle")
    s.text(668, 180, "requests", 12, "soft", anchor="middle")
    s.text(668, 256, "tool calls", 12, "soft", anchor="middle")
    s.text(668, 270, "and edits", 12, "soft", anchor="middle")
    s.text(48, 398, "“Local” can also be a self-hosted server you approve.", 12.5, "soft")
    s.text(48, 417, "A local model's answer can still reveal facts (“3 customers are overdue”): Duet blocks "
                    "the values, not the meaning.", 12.5, "soft")
    return s.render()


def modes(theme):
    s = Svg(960, 330, theme, "Duet's two privacy modes",
            "Hybrid: the frontier model writes the code and sensitive files stay with the local model. "
            "Local-only: the local model does everything and nothing goes to the cloud.")
    rows = [("Writes the code", "Frontier model", "Your local model"),
            ("Reads sensitive files", "Your local model", "Your local model"),
            ("Requests to the cloud", "Checked and logged", "None"),
            ("Web tools", "On", "Off"),
            ("Network for commands", "Package registries only", "Off")]
    x0, cols = 32, [(250, "hybrid", "Hybrid", "default", "duet"), (600, "local", "Local-only", "duet --mode local-only", "ink")]
    for x, _, name, how, colour in cols:
        s.rect(x - 16, 24, 330, 282, fill="card", stroke="duet" if colour == "duet" else "line",
               width=1.5 if colour == "duet" else 1)
        s.text(x, 58, name, 18, weight=600)
        s.text(x, 80, how, 12.5, "soft", mono=True)
    for i, (label, hybrid, local) in enumerate(rows):
        y = 124 + i * 40
        s.line(x0, y + 14, 928, y + 14, "line")
        s.text(x0, y, label, 13.5, "soft")
        s.text(250, y, hybrid, 14, "ink", 500)
        s.text(600, y, local, 14, "ink", 500)
    return s.render()


def lane_numbers(report):
    lanes = report["lanes"]
    return {lane: (v["mean_counted_hidden_pass_rate"], v["literal_canary_observations"], v["captured_frontier_requests"])
            for lane, v in lanes.items()}


def headline(theme, report):
    (d_score, d_found, d_req), (b_score, b_found, b_req) = (
        lane_numbers(report)["duet-hybrid"], lane_numbers(report)["duet-passthrough"])
    s = Svg(960, 262, theme, "Benchmark: test scores and planted secrets",
            f"Across 9 tasks run 3 times each, Duet scored {d_score:.1%} on hidden tests versus {b_score:.1%} "
            f"for the same model with no protection. Planted secrets were found 0 times in Duet's {d_req:,} "
            f"cloud requests and {b_found:,} times in {b_req:,} unprotected requests.")
    s.text(32, 46, "Hidden-test score", 15, weight=600)
    s.text(32, 68, "mean of 27 runs: 9 tasks × 3 runs, failures counted as zero", 12.5, "soft")
    bar_x, bar_w = 230, 280
    for i, (label, value, colour) in enumerate([("Duet", d_score, "duet"), ("No protection", b_score, "base")]):
        y = 104 + i * 44
        s.text(32, y + 16, label, 14, weight=500)
        s.rect(bar_x, y, bar_w, 22, fill="card", stroke="card", r=4)
        s.rect(bar_x, y, bar_w * value, 22, fill=colour, stroke=colour, r=4)
        s.text(bar_x + bar_w + 12, y + 16, f"{value:.1%}", 15, weight=600)
    s.line(600, 30, 600, 196, "line")
    s.text(632, 46, "Planted secrets found in cloud requests", 15, weight=600)
    s.text(632, 68, "complete values, in literal, base64, hex or URL form", 12.5, "soft")
    for i, (label, found, requests, colour) in enumerate(
            [("Duet", d_found, d_req, "duet"), ("No protection", b_found, b_req, "base")]):
        y = 104 + i * 44
        s.text(632, y + 16, label, 14, weight=500)
        s.text(928, y + 17, f"{found:,}", 22, colour, 700, anchor="end")
        s.text(928, y + 34, f"in {requests:,} requests", 11.5, "soft", anchor="end")
    s.text(32, 234, "Same frontier model (glm-5.3-flash) in both lanes. Zero matches does not rule out "
                    "disclosure through summaries or inference.", 12.5, "soft")
    return s.render()


def per_run(theme, report):
    runs = defaultdict(list)
    found = defaultdict(int)
    for r in report["runs"]:
        lane = "duet" if r["lane"] == "duet-hybrid" else "base"
        runs[(r["task"], lane)].append((r["seed"], r["counted_hidden_pass_rate"]))
        found[(r["task"], lane)] += r["frontier_traffic"]["canary_observations"]
    assert all(len(v) == 3 for v in runs.values()) and {t for t, _ in runs} == {t for t, _ in TASKS}
    row_h, top = 46, 124
    height = top + row_h * len(TASKS) + 70
    s = Svg(960, height, theme, "Benchmark results for each task and run",
            "Each dot is one run's hidden-test score. Duet matches the unprotected model on most tasks; "
            "two Duet runs scored zero (a build that did not compile and a run stopped at its deadline). "
            "Planted secrets were never found in Duet's requests.")
    px, pw = 220, 430  # plot x and width
    s.circle(32, 34, 6, "duet")
    s.text(46, 39, "Duet", 13.5, weight=500)
    s.circle(110, 34, 6, "base")
    s.text(124, 39, "Same model, no protection", 13.5, weight=500)
    s.text(px, 80, "Hidden-test score, each run", 13, "soft", 600)
    s.text(928, 80, "Planted secrets found in requests", 13, "soft", 600, anchor="end")
    s.text(810, 98, "Duet", 12.5, "duet", 600, anchor="end")
    s.text(928, 98, "No protection", 12.5, "base", 600, anchor="end")
    for v in (0, 0.5, 1):
        x = px + pw * v
        s.line(x, top - 14, x, top + row_h * len(TASKS) - 10, "line", dash=v != 1)
        s.text(x, top + row_h * len(TASKS) + 10, f"{v:.0%}", 12, "soft", anchor="middle")
    for i, (task, name) in enumerate(TASKS):
        y = top + i * row_h + 8
        s.text(32, y + 5, name, 14, weight=500)
        for lane, dy in (("base", 6), ("duet", -6)):
            for seed, score in sorted(runs[(task, lane)]):
                s.circle(px + pw * score, y + dy, 6, lane, width=1.5)
                note = ZERO_NOTES.get((task, seed)) if lane == "duet" and score == 0 else None
                if note:
                    s.text(px + 14, y - 2, note, 12, "soft", italic=True)
        s.text(810, y + 6, f"{found[(task, 'duet')]:,}", 14, "duet", 600, anchor="end")
        s.text(928, y + 6, f"{found[(task, 'base')]:,}", 14, "base", 600, anchor="end")
        if i < len(TASKS) - 1:
            s.line(32, y + row_h / 2 + 2, 928, y + row_h / 2 + 2, "line")
    s.text(32, height - 18, "Each task ran 3 times per lane with the same frontier model. Overlapping dots are "
                            "runs with the same score.", 12.5, "soft")
    return s.render()


def figures():
    report = load_report()
    out = {}
    for theme in THEMES:
        out[f"duet-flow-{theme}.svg"] = flow(theme)
        out[f"duet-modes-{theme}.svg"] = modes(theme)
        out[f"duet-results-{theme}.svg"] = headline(theme, report)
        out[f"duet-results-by-task-{theme}.svg"] = per_run(theme, report)
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--check", action="store_true", help="fail if a committed figure is missing or stale")
    args = p.parse_args()
    stale = []
    for name, svg in figures().items():
        path = OUT / name
        if args.check:
            if not path.is_file() or path.read_text() != svg:
                stale.append(name)
        else:
            path.write_text(svg)
    if stale:
        sys.exit("stale figures (run tools/render-infographics.py): " + ", ".join(stale))
    print("figures are current" if args.check else f"wrote {len(figures())} figures to {OUT}")


if __name__ == "__main__":
    main()
