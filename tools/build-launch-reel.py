#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Rebuild the fresh launch GIF, MP4 and stills from genuine recorded PTY output.

Requires Node, ffmpeg, and the dependencies of render-tui-cast.cjs.
Point NODE_PATH at the directory containing @playwright/test; pass xterm's
package directory and a Chrome executable. No UI text is generated or edited.
"""
import argparse
import gzip
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--xterm', type=Path, required=True)
    parser.add_argument('--chrome', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    evidence = root / 'docs/evidence/launch-refresh-2026-10-01'
    edit = json.loads((evidence / 'reel-edit.json').read_text())
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='duet-reel-') as temp:
        temp = Path(temp)
        cast = temp / 'hybrid.cast'
        cast.write_bytes(gzip.decompress((evidence / edit['cast']).read_bytes()))
        times = sorted(set(edit['source_seconds'] + list(edit['stills'].values())))
        (temp / 'times.json').write_text(json.dumps(times))
        rendered = temp / 'rendered'
        subprocess.run([
            'node', str(root / 'tools/render-tui-cast.cjs'), str(cast), str(rendered),
            str(args.xterm.resolve()), str(args.chrome.resolve()), str(temp / 'times.json'),
        ], check=True)
        frames = {t: rendered / f'{i:05d}.png' for i, t in enumerate(times)}
        sequence = temp / 'sequence'
        sequence.mkdir()
        for i, t in enumerate(edit['source_seconds']):
            shutil.copyfile(frames[t], sequence / f'{i:05d}.png')
        for name, t in edit['stills'].items():
            shutil.copyfile(frames[t], args.output / name)
        common = ['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
                  '-framerate', str(edit['fps']), '-i', str(sequence / '%05d.png')]
        subprocess.run(common + [
            '-filter_complex',
            '[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3',
            '-loop', '0', str(args.output / 'duet-boundary.gif'),
        ], check=True)
        subprocess.run(common + [
            '-vf', 'fps=30,format=yuv420p', '-c:v', 'libx264', '-crf', '18',
            '-movflags', '+faststart', str(args.output / 'duet-boundary.mp4'),
        ], check=True)
    print(json.dumps({'output': str(args.output), 'frames': len(edit['source_seconds']),
                      'seconds': len(edit['source_seconds']) / edit['fps']}))


if __name__ == '__main__':
    main()
