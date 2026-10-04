#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Render Duet's documentation figures from the verified October benchmark.

Install tools/infographic-requirements.txt in a virtual environment, then run
this script from any directory. --check verifies source and generated hashes
without importing plotting dependencies or changing files.
"""

import argparse
import hashlib
import html
import json
import re
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "docs/evidence/benchmark-54-2026-10-04"
OUT = ROOT / "docs/assets/infographics"
INK = "#142D29"
PAPER = "#F4F1E8"
TEAL = "#087F74"
MINT = "#7DE1BF"
AMBER = "#B75E2E"
SAND = "#EAB283"
MUTED = "#536962"
LINE = "#CCD2C6"
SOFT = "#E5E9DE"
WHITE = "#FCFAF3"
BODY = ["DejaVu Sans", "Trebuchet MS", "sans-serif"]
DISPLAY = ["DejaVu Serif", "Georgia", "serif"]


def sha(data):
    return hashlib.sha256(data).hexdigest()


def load_results():
    raw = (EVIDENCE / "report.json").read_bytes()
    verdict = json.loads((EVIDENCE / "audit-verdict.json").read_text())
    assert verdict["status"] == "passed" and verdict["publication_ready"]
    assert sha(raw) == verdict["report_json_sha256"], "Report differs from its verdict"
    report = json.loads(raw)
    runs = report["runs"]
    assert len(runs) == len({r["case"] for r in runs}) == 54
    assert report["coverage"]["selected_pairs"] == 27
    for lane in report["lanes"]:
        selected = [r for r in runs if r["lane"] == lane]
        assert len(selected) == 27 and all(r["selected"] for r in selected)
        mean = sum(r["counted_hidden_pass_rate"] for r in selected) / 27
        assert abs(mean - report["lanes"][lane]["mean_counted_hidden_pass_rate"]) < 1e-12
    return report


class Canvas:
    def __init__(self, width, height, dark=False):
        import matplotlib.pyplot as plt

        self.width, self.height, self.dark = width, height, dark
        self.fig = plt.figure(figsize=(width / 100, height / 100), dpi=100)
        self.fig.patch.set_facecolor(INK if dark else PAPER)
        self.ax = self.fig.add_axes((0, 0, 1, 1))
        self.ax.set(xlim=(0, width), ylim=(height, 0))
        self.ax.axis("off")
        self.labels = []

    def text(self, x, y, value, size=22, color=None, weight="normal", serif=False,
             align="left", linespacing=1.6, mono=False):
        if "\n" in value:
            for i, line in enumerate(value.splitlines()):
                artist = self.text(x, y + i * size * linespacing, line, size, color,
                                   weight, serif, align, linespacing, mono)
            return artist
        family = ["DejaVu Sans Mono", "monospace"] if mono else (DISPLAY if serif else BODY)
        artist = self.ax.text(x, y, value, fontsize=size * .72,
                              color=color or (WHITE if self.dark else INK),
                              fontfamily=family, fontweight=weight,
                              ha=align, va="baseline", linespacing=1)
        self.labels.append(artist)
        return artist

    def rect(self, x, y, width, height, fill, edge="none", radius=0, lw=1):
        from matplotlib.patches import FancyBboxPatch, Rectangle

        if radius:
            p = FancyBboxPatch((x, y), width, height,
                              boxstyle=f"round,pad=0,rounding_size={radius}",
                              facecolor=fill, edgecolor=edge, linewidth=lw)
        else:
            p = Rectangle((x, y), width, height, facecolor=fill,
                          edgecolor=edge, linewidth=lw)
        self.ax.add_patch(p)

    def line(self, x1, y1, x2, y2, color=LINE, lw=1, style="-"):
        self.ax.plot([x1, x2], [y1, y2], color=color, linewidth=lw,
                     linestyle=style, solid_capstyle="round")

    def dot(self, x, y, radius, color, edge=None):
        from matplotlib.patches import Circle

        self.ax.add_patch(Circle((x, y), radius, facecolor=color,
                                edgecolor=edge or color, linewidth=1.2))

    def arrow(self, x1, y1, x2, y2, color=TEAL, lw=2):
        from matplotlib.patches import FancyArrowPatch

        self.ax.add_patch(FancyArrowPatch((x1, y1), (x2, y2),
                                         arrowstyle="-|>", mutation_scale=14,
                                         linewidth=lw, color=color))

    def brand(self, section):
        accent = MINT if self.dark else TEAL
        self.rect(54, 41, 8, 22, accent)
        self.rect(67, 47, 8, 16, SAND if self.dark else AMBER)
        self.text(90, 60, "DUET", 19, weight="bold")
        self.text(self.width - 54, 58, section, 14,
                  color=MINT if self.dark else MUTED, align="right")

    def save(self, name, title, description):
        import matplotlib.pyplot as plt

        self.fig.canvas.draw()
        renderer = self.fig.canvas.get_renderer()
        bounds = self.fig.bbox
        for artist in self.labels:
            box = artist.get_window_extent(renderer)
            assert (box.x0 >= 0 and box.y0 >= 0 and box.x1 <= bounds.width
                    and box.y1 <= bounds.height), f"Text outside {name}: {artist.get_text()}"
        for i, a in enumerate(self.labels):
            ba = a.get_window_extent(renderer)
            for b in self.labels[i + 1:]:
                bb = b.get_window_extent(renderer)
                overlap_x = min(ba.x1, bb.x1) - max(ba.x0, bb.x0)
                overlap_y = min(ba.y1, bb.y1) - max(ba.y0, bb.y0)
                assert not (overlap_x > 1 and overlap_y > 1), (
                    f"Overlapping labels in {name}: {a.get_text()} / {b.get_text()}")
        OUT.mkdir(parents=True, exist_ok=True)
        metadata = {"Title": title, "Description": description,
                    "Creator": "Duet documentation / tools/render-infographics.py"}
        self.fig.savefig(OUT / f"{name}.png", dpi=200, metadata=metadata)
        self.fig.savefig(OUT / f"{name}.svg", metadata={**metadata, "Date": None})
        p = OUT / f"{name}.svg"
        svg = p.read_text()
        svg = re.sub(r'<!DOCTYPE svg[^>]+>', '', svg, flags=re.S)
        svg = re.sub(r'<svg\b([^>]*)>',
                     r'<svg\1 role="img" aria-labelledby="figure-title figure-desc">', svg, count=1)
        opening = svg.index('>', svg.index('<svg')) + 1
        accessible = (f'\n<title id="figure-title">{html.escape(title)}</title>'
                      f'\n<desc id="figure-desc">{html.escape(description)}</desc>')
        svg = svg[:opening] + accessible + svg[opening:]
        svg = svg.replace('?>', '?>\n<!-- SPDX-License-Identifier: GPL-3.0-or-later -->', 1)
        p.write_text("\n".join(line.rstrip() for line in svg.splitlines()) + "\n")
        plt.close(self.fig)
        return {"title": title, "description": description,
                "width": self.width, "height": self.height,
                "png_scale": 2}


def boundary_figure():
    c = Canvas(1280, 860, dark=True)
    c.brand("01 / HOW DUET WORKS")
    c.text(54, 137, "Frontier coding.", 53, serif=True)
    c.text(54, 199, "Private context under your control.", 43, serif=True, color=MINT)
    c.text(54, 240, "A local model handles sensitive content. Duet checks what the frontier receives.", 22)

    c.rect(48, 300, 834, 298, "#1A3A32", edge="#53776A", radius=14)
    c.text(73, 285, "YOUR TRUSTED ENVIRONMENT", 15, color=MINT, weight="bold")
    c.text(1230, 285, "CHOSEN PROVIDER", 15, color=SAND, align="right", weight="bold")

    # Files and local handling are inside the trusted environment; the host gate
    # is a separate component, not part of either model.
    c.rect(73, 341, 211, 212, "#203F37", edge="#53776A", radius=8)
    for yy, width in [(365, 42), (374, 60), (383, 48)]:
        c.line(95, yy, 95 + width, yy, MINT, 2)
    c.text(95, 425, "Sensitive files", 23, weight="bold")
    c.text(95, 465, "Customer records\nConfidential logs\nProtected source", 19, color="#D0DFD3")
    c.arrow(291, 440, 327, 440, MINT)

    c.text(346, 368, "LOCAL HANDLING", 14, color=MINT, weight="bold")
    c.text(346, 408, "Local reader", 27, weight="bold")
    c.text(346, 440, "Answers specific questions", 18, color="#D0DFD3")
    c.line(346, 461, 594, 461, "#53776A")
    c.text(346, 493, "Structure views", 23, weight="bold")
    c.text(346, 525, "Schemas + synthetic examples", 17, color="#D0DFD3")
    c.arrow(606, 440, 642, 440, MINT)

    c.rect(651, 338, 198, 219, TEAL, radius=8)
    c.text(676, 373, "HOST APPLICATION", 12, color=WHITE, weight="bold")
    c.text(676, 415, "Policy gate", 26, weight="bold")
    c.text(676, 454, "Filter + check", 20)
    c.text(676, 489, "Record outbound", 17)
    c.text(676, 514, "context", 17)

    c.text(937, 387, "Permitted code", 15, color=SAND, align="center")
    c.text(937, 409, "+ checked context", 15, color=SAND, align="center")
    c.arrow(861, 440, 1007, 440, SAND, 2.5)
    c.rect(1013, 338, 216, 219, INK, edge=SAND, radius=8, lw=1.5)
    c.text(1037, 389, "Frontier", 28, color=SAND, weight="bold")
    c.text(1037, 424, "model", 28, color=SAND, weight="bold")
    c.text(1037, 467, "Plans the work", 18)
    c.text(1037, 495, "Writes code", 18)
    c.text(1037, 523, "Repairs failures", 18)

    c.line(54, 639, 1226, 639, "#53776A")
    for x, number, title, detail in [
        (54, "01", "Set the policy", "Choose files, code visibility\nand approved endpoints."),
        (462, "02", "Enforce the boundary", "Host checks and sandbox rules\napply outside model instructions."),
        (870, "03", "Inspect what leaves", "Review the prepared context\nand verify saved audit records."),
    ]:
        c.text(x, 679, number, 17, color=MINT, mono=True)
        c.text(x, 715, title, 25, weight="bold")
        c.text(x, 750, detail, 19, color="#D0DFD3")
    c.text(54, 822, "Local = your approved endpoint. The local reader cannot authorize disclosure. Checked context can reveal meaning.", 16, color="#B9CEC0")
    return c.save("duet-boundary", "How Duet uses frontier models with private context",
                  "Architecture diagram: sensitive files and local handling stay inside the approved trusted environment. "
                  "A separate host policy gate filters, checks and records outbound context before permitted code and checked "
                  "context reach the frontier model. Local can mean a workstation or self-hosted endpoint; checks do not guarantee semantic secrecy.")


def results_figure(report):
    h, p = report["lanes"]["duet-hybrid"], report["lanes"]["duet-passthrough"]
    c = Canvas(1280, 960)
    c.brand("02 / VERIFIED RESULTS · 04 OCT 2026")
    c.text(54, 138, "Coding results.", 53, serif=True)
    c.text(54, 200, "Disclosure measured separately.", 45, serif=True, color=TEAL)
    c.text(54, 245, "Same frontier model · Nine tasks · Three paired seeds", 23, color=MUTED)
    c.line(54, 276, 1226, 276)
    c.line(650, 310, 650, 690)
    c.text(54, 320, "MEAN PER-CASE HIDDEN-TEST SCORE", 15, weight="bold")
    c.text(698, 320, "CAPTURED FRONTIER TRAFFIC", 15, weight="bold")
    for y, label, lane, color in [(374, "Duet hybrid", h, TEAL),
                                   (523, "Frontier · boundary disabled", p, AMBER)]:
        c.text(54, y, label, 22, color=color, weight="bold")
        c.text(54, y + 67, f"{lane['mean_counted_hidden_pass_rate'] * 100:.2f}%", 59, color=color, serif=True)
        c.rect(54, y + 89, 530, 19, SOFT)
        c.rect(54, y + 89, 530 * lane["mean_counted_hidden_pass_rate"], 19, color)
    c.text(54, 663, "0%", 16, color=MUTED)
    c.text(584, 663, "100%", 16, color=MUTED, align="right")
    c.text(319, 663, "Same 0–100 scale", 16, color=MUTED, align="center")
    c.text(698, 454, f"{h['literal_canary_observations']:,}", 144, color=TEAL, serif=True)
    c.text(698, 493, "planted-value matches", 25, color=TEAL, weight="bold")
    c.text(698, 531, f"in {h['captured_frontier_requests']:,} captured hybrid requests", 20)
    c.text(698, 603, f"{p['literal_canary_observations']:,}", 40, color=AMBER, serif=True)
    c.text(698, 636, f"matches in {p['captured_frontier_requests']:,} requests", 20, color=MUTED)
    c.text(698, 665, "with the boundary disabled", 20, color=MUTED)
    c.line(54, 716, 1226, 716)
    for x, value, label in [(54, report['coverage']['selected_cases'], "outcomes included"),
                             (467, report['coverage']['selected_pairs'], "task / seed pairs"),
                             (879, report['coverage']['native_graded_runs'], "native grader records")]:
        c.text(x, 779, str(value), 48, serif=True)
        c.text(x, 816, label, 21, color=MUTED)
    c.text(54, 870, "All outcomes count, including one externally stopped hybrid case scored zero. No fresh quality judges.", 17, color=MUTED)
    c.text(54, 900, "Literal matches are observed occurrences, not unique secrets or a measure of semantic secrecy.", 17, color=MUTED)
    c.text(54, 933, "Source: verified 54-case report · b9eb511 · github.com/maximpri/duet", 14, color=MUTED)
    return c.save("duet-results-2026-10-04", "Duet's verified 54-case coding and disclosure results",
                  f"Mean counted per-case hidden-test scores: Duet hybrid {h['mean_counted_hidden_pass_rate']*100:.2f}%, "
                  f"same frontier with boundary disabled {p['mean_counted_hidden_pass_rate']*100:.2f}%. "
                  f"Hybrid had {h['literal_canary_observations']:,} planted-value matches in {h['captured_frontier_requests']:,} "
                  f"captured requests, versus {p['literal_canary_observations']:,} in {p['captured_frontier_requests']:,} passthrough requests. "
                  "All 54 outcomes count, including one external failure scored zero. There are 53 native grader records; no fresh quality judges ran.")


def modes_figure():
    c = Canvas(1280, 970)
    c.brand("03 / CHOOSE YOUR DATA FLOW")
    c.text(54, 138, "Choose what reaches", 52, serif=True)
    c.text(54, 199, "the frontier.", 52, serif=True, color=TEAL)
    c.text(54, 246, "Keep the same workflow. Set the boundary for the task.", 23, color=MUTED)
    c.rect(654, 283, 572, 577, INK, radius=12)
    c.text(78, 338, "HYBRID", 17, color=TEAL, weight="bold")
    c.text(684, 338, "TOP CLEARANCE", 17, color=MINT, weight="bold")
    c.text(78, 381, "Frontier + local", 36, serif=True)
    c.text(684, 381, "Your local model", 36, serif=True, color=WHITE)
    c.text(78, 420, "duet", 21, color=TEAL, mono=True)
    c.text(684, 420, "duet --mode top-clearance", 20, color=MINT, mono=True)
    rows = [
        (476, "CODING AGENT", "Frontier model", "Approved local model"),
        (575, "SENSITIVE CONTENT", "Handled locally", "Handled locally"),
        (674, "FRONTIER RECEIVES", "Permitted code + checked context", "No frontier calls"),
        (773, "WEB + COMMAND NETWORKING", "Governed by policy", "Disabled"),
    ]
    for y, label, left, right in rows:
        c.line(78, y - 32, 598, y - 32)
        c.line(684, y - 32, 1196, y - 32, "#53776A")
        c.text(78, y, label, 13, color=MUTED, weight="bold")
        c.text(684, y, label, 13, color="#B9CEC0", weight="bold")
        c.text(78, y + 39, left, 25)
        c.text(684, y + 39, right, 25, color=WHITE)
    c.text(684, 841, "Networked MCP servers are also unavailable.", 17, color="#B9CEC0")
    c.text(54, 905, "“Local” can mean your workstation or an approved self-hosted endpoint.", 21, color=TEAL, weight="bold")
    c.text(54, 940, "That endpoint and its network path remain part of your trusted environment.", 19, color=MUTED)
    return c.save("duet-modes", "Choose between hybrid and top-clearance mode",
                  "Hybrid uses a frontier coding agent and local sensitive-content processing; permitted code and checked context "
                  "can reach the frontier. Top clearance uses the approved local model as the coding agent, with no frontier calls, "
                  "web tools, command networking or networked MCP servers. Local may be a workstation or self-hosted endpoint; it is not an air-gap guarantee.")


TASK_NAMES = {
    "S1": "Configuration", "S2": "Crash diagnosis", "M1": "Billing export †",
    "M2": "Data-subject export", "M3": "Hostile logs", "L1": "Ledger reconciliation",
    "L2": "Protected pricing", "X1": "SQL gateway *", "X2": "Partner exports",
}


def task_means(report):
    groups = defaultdict(list)
    for run in report["runs"]:
        groups[(run["task"], run["lane"])].append(run["counted_hidden_pass_rate"])
    assert set(task for task, _ in groups) == set(TASK_NAMES)
    assert all(len(values) == 3 for values in groups.values())
    return {task: {lane: sum(groups[(task, lane)]) / 3 for lane in report["lanes"]}
            for task in TASK_NAMES}


def tasks_figure(report):
    means = task_means(report)
    c = Canvas(1280, 1190)
    c.brand("04 / EVERY TASK, ALL THREE SEEDS")
    c.text(54, 138, "The complete comparison.", 49, serif=True)
    c.text(54, 186, "Per-task hidden-test scores · Mean of three counted outcomes", 23, color=MUTED)
    c.rect(54, 221, 24, 12, TEAL)
    c.text(91, 234, "Duet hybrid", 20, color=TEAL, weight="bold")
    c.rect(291, 221, 24, 12, AMBER)
    c.text(328, 234, "Same frontier, boundary disabled", 20, color=AMBER, weight="bold")
    start, width = 407, 694
    for value in [0, 25, 50, 75, 100]:
        x = start + width * value / 100
        c.line(x, 310, x, 996, LINE, .8)
        c.text(x, 291, f"{value}%", 16, color=MUTED, align="center")
    for i, (task, label) in enumerate(TASK_NAMES.items()):
        y = 344 + i * 76
        c.text(54, y + 1, task, 21, color=TEAL, weight="bold", mono=True)
        c.text(117, y + 1, label, 21)
        for dy, lane, color in [(-15, "duet-hybrid", TEAL),
                                 (12, "duet-passthrough", AMBER)]:
            rate = means[task][lane]
            c.rect(start, y + dy, width * rate, 19, color)
            c.text(start + width * rate + 12, y + dy + 16,
                   f"{rate * 100:.2f}%", 18, color=color, weight="bold")
    c.line(54, 1021, 1226, 1021)
    c.text(54, 1061, "All three seeds count. No case is removed because its result was poor.", 22, weight="bold")
    c.text(54, 1100, "* X1 hybrid includes an externally stopped case scored zero.", 18, color=MUTED)
    c.text(54, 1129, "† M1 hybrid includes a compilation failure with no observed hidden-test results, scored zero.", 18, color=MUTED)
    c.text(54, 1170, "Source: verified 54-case report · 4 October 2026 · Mechanical scores; no fresh quality judges", 15, color=MUTED)
    return c.save("duet-task-results-2026-10-04", "Duet hidden-test scores for every benchmark task",
                  "Each task uses all three seeds in each lane, including zero-score outcomes. "
                  + "; ".join(f"{task}: hybrid {v['duet-hybrid']*100:.2f}%, boundary disabled {v['duet-passthrough']*100:.2f}%"
                              for task, v in means.items())
                  + ". X1 hybrid includes one external deadline failure. M1 hybrid includes a compilation failure with no observed hidden results.")


def check_assets(report):
    manifest = json.loads((OUT / "manifest.json").read_text())
    assert manifest["report_sha256"] == sha((EVIDENCE / "report.json").read_bytes())
    assert manifest["verdict_sha256"] == sha((EVIDENCE / "audit-verdict.json").read_bytes())
    assert manifest["generator_sha256"] == sha(Path(__file__).read_bytes())
    assert manifest["task_means"] == task_means(report)
    for name, digest in manifest["assets_sha256"].items():
        assert sha((OUT / name).read_bytes()) == digest, f"Changed asset: {name}"
    print(f"Verified {len(manifest['assets_sha256'])} assets against the report, verdict and renderer.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    report = load_results()
    if args.check:
        check_assets(report)
        return
    import matplotlib

    matplotlib.use("Agg")
    matplotlib.rcParams.update({"svg.fonttype": "none", "svg.hashsalt": "duet-infographics-2026-10-04",
                                "font.family": BODY, "axes.unicode_minus": False})
    figures = {}
    for name, draw in [
        ("duet-boundary", boundary_figure),
        ("duet-results-2026-10-04", lambda: results_figure(report)),
        ("duet-modes", modes_figure),
        ("duet-task-results-2026-10-04", lambda: tasks_figure(report)),
    ]:
        figures[name] = draw()
    assets = {f"{name}.{ext}": sha((OUT / f"{name}.{ext}").read_bytes())
              for name in figures for ext in ("svg", "png")}
    manifest = {"schema_version": 1, "license": "GPL-3.0-or-later",
                "report": EVIDENCE.relative_to(ROOT).as_posix() + "/report.json",
                "report_sha256": sha((EVIDENCE / "report.json").read_bytes()),
                "verdict_sha256": sha((EVIDENCE / "audit-verdict.json").read_bytes()),
                "generator_sha256": sha(Path(__file__).read_bytes()),
                "matplotlib_version": matplotlib.__version__, "figures": figures,
                "task_means": task_means(report), "assets_sha256": assets}
    (OUT / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    check_assets(report)


if __name__ == "__main__":
    main()
