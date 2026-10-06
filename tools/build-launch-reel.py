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
import math
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
    parser.add_argument('--evidence', type=Path,
                        default=root / 'docs/evidence/launch-refresh-2026-10-01')
    parser.add_argument('--name', default='declass-boundary', help='Output media basename')
    args = parser.parse_args()
    if not args.name or Path(args.name).name != args.name or args.name in ('.', '..'):
        parser.error('--name must be a filename without directories')
    evidence = args.evidence
    edit = json.loads((evidence / 'reel-edit.json').read_text())
    fps = edit['fps']
    if not isinstance(fps, (int, float)) or isinstance(fps, bool) or not math.isfinite(fps) or fps <= 0:
        parser.error('reel fps must be a positive finite number')
    if not isinstance(edit['source_seconds'], list) or not edit['source_seconds']:
        parser.error('reel source_seconds must be a nonempty list')
    selected_times = edit['source_seconds'] + list(edit['stills'].values())
    if any(not isinstance(t, (int, float)) or isinstance(t, bool) or not math.isfinite(t) or t < 0
           for t in selected_times):
        parser.error('reel timestamps must be nonnegative finite numbers')
    if any(not name or Path(name).name != name or name in ('.', '..') for name in edit['stills']):
        parser.error('reel still names must be filenames without directories')
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='declass-reel-') as temp:
        temp = Path(temp)
        cast = temp / 'hybrid.cast'
        cast.write_bytes(gzip.decompress((evidence / edit['cast']).read_bytes()))
        times = sorted(set(selected_times))
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
            '-loop', '0', str(args.output / f'{args.name}.gif'),
        ], check=True)
        subprocess.run(common + [
            '-vf', 'fps=30,pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p',
            '-c:v', 'libx264', '-crf', '18',
            '-movflags', '+faststart', str(args.output / f'{args.name}.mp4'),
        ], check=True)
    print(json.dumps({'output': str(args.output), 'frames': len(edit['source_seconds']),
                      'seconds': len(edit['source_seconds']) / edit['fps']}))


if __name__ == '__main__':
    main()
