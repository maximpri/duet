#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Record a real, scripted Declass session and render it as GIF, MP4 and stills.

    python3 tools/declass-recorder/record.py                       # the billing demo
    python3 tools/declass-recorder/record.py --scene SCENE.json --out DIR
    python3 tools/declass-recorder/record.py --render-only DIR/session.cast

The scene's fixture is copied into a fresh git workspace and the real `declass`
binary runs there in a pseudo-terminal, against the models in your owner
config. Keystrokes are typed into that terminal the way a person would type
them; a `pyte` screen follows the output so a step can wait for text to
appear. Every byte Declass writes is kept in an asciicast v2 file. Rendering
replays those bytes in xterm.js: no screen text is generated or edited. Only
playback time changes, where the scene asks for it, and the window shows the
speed while it does.

Needs Python 3.9+ with pyte (pip install pyte), Node, ffmpeg and Chrome or
Chromium. --install-deps puts xterm.js and Playwright in the recorder's cache
directory, outside the checkout. Record only synthetic workspaces for
publication: a cast holds everything shown on the terminal.
"""
import argparse
import codecs
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import urllib.parse

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CACHE = Path(os.environ.get('DECLASS_RECORDER_CACHE', Path.home() / '.cache/declass-recorder'))
NODE_DEPS = ['@xterm/xterm@5.5.0', 'playwright-core@1.58.2']

# What each named key sends on an xterm-256color terminal.
KEYS = {
    'enter': '\r', 'tab': '\t', 'shift-tab': '\x1b[Z', 'esc': '\x1b', 'backspace': '\x7f',
    'up': '\x1b[A', 'down': '\x1b[B', 'right': '\x1b[C', 'left': '\x1b[D',
    'ctrl-up': '\x1b[1;5A', 'ctrl-down': '\x1b[1;5B', 'ctrl-end': '\x1b[1;5F',
    'pageup': '\x1b[5~', 'pagedown': '\x1b[6~',
    'f1': '\x1bOP', 'f2': '\x1bOQ', 'f3': '\x1bOR', 'f4': '\x1bOS',
}
KEYS.update({'ctrl-' + c: chr(ord(c) - 96) for c in 'abcdefghijklmnopqrstuvwxyz'})


def fail(message):
    sys.exit('declass-recorder: ' + message)


class Session:
    """The real program in a pseudo-terminal, its screen, and the cast."""

    def __init__(self, command, cwd, env, cols, rows, cast_path, title):
        import pyte  # Checked in main() with a helpful message.
        self.screen = pyte.Screen(cols, rows)
        self.stream = pyte.Stream(self.screen)
        self.decoder = codecs.getincrementaldecoder('utf-8')('replace')
        self.cols, self.rows = cols, rows
        self.pending = ''
        self.started = time.monotonic()
        self.cast = os.fdopen(os.open(cast_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600),
                              'w', encoding='utf-8')
        self.cast.write(json.dumps({
            'version': 2, 'width': cols, 'height': rows, 'timestamp': int(time.time()),
            'title': title, 'env': {'TERM': env['TERM'], 'SHELL': env.get('SHELL', '')},
        }) + '\n')
        self.pid, self.fd = os.forkpty()
        if self.pid == 0:
            try:
                os.chdir(cwd)
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
                os.execvpe(command[0], command, env)
            finally:
                os._exit(127)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))

    def now(self):
        return round(time.monotonic() - self.started, 4)

    def event(self, kind, data):
        self.cast.write(json.dumps([self.now(), kind, data], ensure_ascii=False) + '\n')
        self.cast.flush()

    def answer_queries(self, text):
        """Answers what Terminal.app answers: cursor position and primary device
        attributes. A keyboard-protocol query (`CSI ? u`) gets no answer there,
        and none here, so the program sees the same terminal."""
        text = self.pending + text
        for match in re.finditer(r'\x1b\[(\??)(\d*)([nc])', text):
            private, number, final = match.groups()
            if final == 'n' and number == '6' and not private:
                self.write(f'\x1b[{self.screen.cursor.y + 1};{self.screen.cursor.x + 1}R', record=False)
            elif final == 'c' and number in ('', '0') and not private:
                self.write('\x1b[?1;2c', record=False)
        # Keep a possible partial query for the next read.
        cut = text.rfind('\x1b')
        self.pending = text[cut:] if cut >= 0 and len(text) - cut < 8 else ''

    def pump(self, seconds):
        """Reads output for up to `seconds`. Returns False once the program has exited."""
        deadline = time.monotonic() + seconds
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                return True
            try:
                ready = select.select([self.fd], [], [], min(left, 0.05))[0]
                data = os.read(self.fd, 65536) if ready else None
            except OSError:
                return False
            if data is None:
                continue
            if not data:
                return False
            text = self.decoder.decode(data)
            if text:
                self.event('o', text)
                self.stream.feed(text)
                self.answer_queries(text)

    def write(self, data, record=True):
        if record:
            self.event('i', data)
        raw = data.encode()
        while raw:
            raw = raw[os.write(self.fd, raw):]

    def text(self):
        return '\n'.join(self.screen.display)

    def wait(self, pattern, timeout):
        regex = re.compile(pattern)
        deadline = time.monotonic() + timeout
        while not regex.search(self.text()):
            if time.monotonic() > deadline:
                raise TimeoutError(f'no match for /{pattern}/ within {timeout}s; screen:\n{self.text()}')
            if not self.pump(0.1):
                raise RuntimeError(f'declass exited while waiting for /{pattern}/; screen:\n{self.text()}')

    def close(self):
        self.pump(0.3)
        tail = self.decoder.decode(b'', final=True)
        if tail:
            self.event('o', tail)
        for sig in (signal.SIGHUP, signal.SIGTERM, signal.SIGKILL):
            try:
                if os.waitpid(self.pid, os.WNOHANG) != (0, 0):
                    break
                os.kill(self.pid, sig)
            except (ChildProcessError, ProcessLookupError):
                break
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                try:
                    if os.waitpid(self.pid, os.WNOHANG) != (0, 0):
                        break
                except ChildProcessError:
                    break
                time.sleep(0.05)
            else:
                continue
            break
        os.close(self.fd)
        self.cast.close()


def run_steps(session, steps, log):
    """Performs the scene's steps. Markers go into the cast for the renderer."""
    for i, step in enumerate(steps):
        kind = next((k for k in ('wait', 'type', 'key', 'press', 'sleep', 'hold', 'speed', 'still',
                                 'start', 'end', 'command') if k in step), None)
        if kind is None:
            fail(f'step {i + 1} has no known action: {step}')
        log(f'step {i + 1}: {kind} {json.dumps(step[kind], ensure_ascii=False)[:70]}')
        if kind == 'wait':
            session.wait(step['wait'], step.get('timeout', 60))
        elif kind == 'type':
            # Human typing: a steady rate with small variation, a pause after words.
            cps = step.get('cps', 28)
            for n, ch in enumerate(step['type']):
                session.write(ch)
                delay = 1 / cps * (0.6 + 0.8 * ((n * 7919) % 100) / 100)
                session.pump(delay + (0.05 if ch in ' ,.' else 0))
        elif kind == 'key':
            keys = step['key'] if isinstance(step['key'], list) else [step['key']]
            for key in keys:
                if key not in KEYS:
                    fail(f'unknown key {key!r}; known: {", ".join(sorted(KEYS))}')
                session.write(KEYS[key])
                session.pump(step.get('gap', 0.35))
        elif kind == 'press':
            # Press a key until the screen matches, e.g. to select a record.
            regex = re.compile(step['until'])
            for _ in range(step.get('max', 20)):
                if regex.search(session.text()):
                    break
                session.write(KEYS[step['press']])
                session.pump(step.get('gap', 0.3))
            else:
                if not step.get('optional'):
                    raise TimeoutError(f'/{step["until"]}/ not reached by pressing {step["press"]}; '
                                       f'screen:\n{session.text()}')
                log(f'  /{step["until"]}/ not reached; continuing (optional)')
        elif kind == 'sleep':
            session.pump(step['sleep'])
        elif kind == 'hold':
            # A pause playback keeps (other pauses are cut to --idle).
            session.event('m', f'hold:{float(step["hold"]):g}')
            session.pump(step['hold'])
        elif kind == 'speed':
            session.event('m', f'speed:{float(step["speed"]):g}')
        elif kind == 'still':
            session.pump(step.get('settle', 0.4))
            session.event('m', f'still:{step["still"]}')
        elif kind == 'start':
            session.event('m', 'start')
        elif kind == 'end':
            session.event('m', 'end')
        elif kind == 'command':
            # Off-camera input (after `end`), e.g. leaving the session.
            session.write(step['command'])
            session.pump(step.get('gap', 0.5))


def prepare_workspace(scene, scene_dir, base):
    fixture = (scene_dir / scene['fixture']).resolve()
    if not fixture.is_dir():
        fail(f'fixture {fixture} is not a directory')
    workspace = Path(tempfile.mkdtemp(prefix='declass-demo-', dir=base)) / fixture.name
    shutil.copytree(fixture, workspace)
    for target, source in scene.get('copy', {}).items():
        shutil.copyfile(workspace / source, workspace / target)
    git = ['git', '-C', str(workspace), '-c', 'user.name=Declass Demo', '-c', 'user.email=demo@declass.invalid']
    subprocess.run(git[:3] + ['init', '-q'], check=True)
    subprocess.run(git + ['add', '-A'], check=True)
    subprocess.run(git + ['commit', '-q', '-m', 'Synthetic demo fixture'], check=True)
    return fixture, workspace


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def verify(scene, fixture, workspace, out):
    """After the session: the scene's acceptance command and the planted-value
    check over every recorded frontier request."""
    report = {}
    if scene.get('check'):
        result = subprocess.run(scene['check'], shell=True, cwd=workspace, capture_output=True, text=True)
        report['check'] = {'command': scene['check'], 'exit_code': result.returncode}
        (out / 'check-after.txt').write_text(result.stdout + result.stderr)
    audits = sorted((workspace / '.declass/audit').glob('*.jsonl')) if (workspace / '.declass/audit').is_dir() else []
    report['runs'] = [a.stem for a in audits]
    if scene.get('canaries') and audits:
        checks = []
        for audit in audits:
            result = subprocess.run([sys.executable, str(ROOT / 'tools/check-launch-canaries.py'),
                                     str(audit), str(fixture)], capture_output=True, text=True)
            (out / f'canaries-{audit.stem}.json').write_text(result.stdout)
            checks.append({'run': audit.stem, 'exit_code': result.returncode})
        report['canaries'] = checks
    diff = subprocess.run(['git', '-C', str(workspace), 'diff'], capture_output=True, text=True).stdout
    (out / 'change.patch').write_text(diff)
    return report


class LocalRelay:
    """A loopback port that forwards to the owner's local model server, and a
    throwaway owner config that points Declass at it. The recording then shows a
    loopback endpoint instead of a development machine's network address. The
    hop to the real server is unchanged: plain HTTP when it was before."""

    def __init__(self, declass, env, log):
        self.env = env
        url = subprocess.run([declass, 'config', 'get', 'local.base_url'], env=env,
                             capture_output=True, text=True).stdout.strip().strip('"')
        parts = urllib.parse.urlsplit(url)
        # Only a plain-HTTP server off loopback is relayed: TLS would not match
        # a loopback name, and a loopback server needs no relay.
        self.active = parts.scheme == 'http' and parts.hostname not in (
            None, '127.0.0.1', 'localhost', '::1')
        if not self.active:
            return
        self.upstream = (parts.hostname, parts.port or 80)
        self.scheme = parts.scheme
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen(16)
        port = self.listener.getsockname()[1]
        threading.Thread(target=self.accept, daemon=True).start()
        self.home = Path(tempfile.mkdtemp(prefix='declass-recorder-config-'))
        owner = Path(env['DECLASS_CONFIG_HOME']) if env.get('DECLASS_CONFIG_HOME') else Path.home() / '.config/declass'
        for name in ('config.toml', 'DECLASS.md'):
            if (owner / name).is_file():
                shutil.copyfile(owner / name, self.home / name)
        self.env = dict(env, DECLASS_CONFIG_HOME=str(self.home))
        loopback = urllib.parse.urlunsplit(('http', f'127.0.0.1:{port}', parts.path, '', ''))
        for key, value in [('local.base_url', json.dumps(loopback)),
                           ('local.allow_plaintext', 'false'), ('local.allowlist', '[]')]:
            result = subprocess.run([declass, 'config', 'set', key, value, '--confirm'], env=self.env,
                                    capture_output=True, text=True)
            if result.returncode != 0:
                fail(f'cannot set {key} in the recording config: {result.stderr.strip()}')
        log(f'local model relayed through {loopback} (your owner config is unchanged)')

    def accept(self):
        while True:
            try:
                client, _ = self.listener.accept()
            except OSError:
                return
            try:
                server = socket.create_connection(self.upstream, timeout=10)
                server.settimeout(None)
            except OSError:
                client.close()
                continue
            for a, b in ((client, server), (server, client)):
                threading.Thread(target=self.pump, args=(a, b), daemon=True).start()

    @staticmethod
    def pump(source, sink):
        try:
            while True:
                data = source.recv(65536)
                if not data:
                    break
                sink.sendall(data)
        except OSError:
            pass
        finally:
            for sock in (source, sink):
                try:
                    sock.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass

    def close(self):
        if self.active:
            self.listener.close()
            shutil.rmtree(self.home, ignore_errors=True)

    def describe(self):
        if not self.active:
            return None
        return {'via': 'loopback relay for this recording', 'upstream_scheme': self.scheme,
                'note': 'the relay forwards to the owner-configured local server unchanged; '
                        'the hop beyond loopback keeps its original transport'}


def node_modules(arg):
    for candidate in [arg, os.environ.get('DECLASS_RECORDER_NODE_MODULES'), CACHE / 'node_modules']:
        if candidate and (Path(candidate) / '@xterm/xterm/lib/xterm.js').is_file():
            return Path(candidate)
    return None


def find_chrome(arg):
    candidates = [arg, os.environ.get('CHROME'),
                  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
                  '/Applications/Chromium.app/Contents/MacOS/Chromium',
                  shutil.which('google-chrome'), shutil.which('chromium'), shutil.which('chromium-browser')]
    return next((c for c in candidates if c and Path(c).exists()), None)


def render(cast, out, args, log):
    modules = node_modules(args.node_modules)
    if modules is None:
        fail('xterm.js and Playwright are missing; run with --install-deps '
             f'(installs {" ".join(NODE_DEPS)} into {CACHE})')
    chrome = find_chrome(args.chrome)
    if chrome is None:
        fail('no Chrome or Chromium found; pass --chrome PATH')
    if shutil.which('ffmpeg') is None:
        fail('ffmpeg is required to encode the GIF and MP4')
    frames = Path(tempfile.mkdtemp(prefix='declass-frames-'))
    try:
        options = {'fps': args.fps, 'idle': args.idle, 'scale': args.scale, 'fontSize': args.font_size,
                   'title': args.title}
        env = dict(os.environ, NODE_PATH=str(modules))
        log('rendering frames with xterm.js')
        subprocess.run(['node', str(HERE / 'render.cjs'), str(cast), str(frames), str(modules),
                        chrome, json.dumps(options)], check=True, env=env)
        summary = json.loads((frames / 'frames.json').read_text())
        for name, file in summary['stills'].items():
            shutil.copyfile(frames / file, out / f'{name}.png')
        name = args.name
        common = ['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
                  '-framerate', str(args.fps), '-i', str(frames / '%05d.png')]
        width = f'scale={args.gif_width}:-2:flags=lanczos,' if args.gif_width else ''
        log('encoding GIF and MP4')
        subprocess.run(common + [
            '-filter_complex',
            f'[0:v]{width}split[a][b];[a]palettegen=stats_mode=diff:max_colors=200[p];'
            '[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle',
            '-loop', '0', str(out / f'{name}.gif')], check=True)
        subprocess.run(common + [
            '-vf', 'pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p',
            '-c:v', 'libx264', '-crf', '20', '-preset', 'slow', '-movflags', '+faststart',
            str(out / f'{name}.mp4')], check=True)
        return summary
    finally:
        shutil.rmtree(frames, ignore_errors=True)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--scene', type=Path, default=HERE / 'scenes/billing.json')
    p.add_argument('--out', type=Path, help='output directory (default: a new directory under $TMPDIR)')
    p.add_argument('--declass', default=os.environ.get('DECLASS_BIN', 'declass'), help='the declass binary to record')
    p.add_argument('--render-only', type=Path, metavar='CAST', help='render an existing cast')
    p.add_argument('--no-render', action='store_true', help='record the cast only')
    p.add_argument('--install-deps', action='store_true', help=f'install {" ".join(NODE_DEPS)} into {CACHE}')
    p.add_argument('--node-modules', help='a node_modules directory with @xterm/xterm and playwright-core')
    p.add_argument('--chrome', help='Chrome or Chromium executable')
    p.add_argument('--name', default='declass-demo', help='basename of the GIF and MP4')
    p.add_argument('--fps', type=int, default=12)
    p.add_argument('--idle', type=float, default=1.5, help='longest pause kept in playback, seconds')
    p.add_argument('--scale', type=float, default=2, help='device pixel ratio of the rendered frames')
    p.add_argument('--gif-width', type=int, default=1200, help='GIF width in pixels (0 keeps full size)')
    p.add_argument('--font-size', type=int, default=16)
    p.add_argument('--title', help='window title (default: the scene title)')
    p.add_argument('--no-local-relay', action='store_true',
                   help='show a non-loopback local model endpoint as configured instead of '
                        'relaying it through loopback for the recording')
    args = p.parse_args()

    def log(message):
        print(f'declass-recorder: {message}', file=sys.stderr, flush=True)

    if args.install_deps:
        if shutil.which('npm') is None:
            fail('npm is required for --install-deps')
        CACHE.mkdir(parents=True, exist_ok=True)
        subprocess.run(['npm', 'install', '--no-audit', '--no-fund', '--prefix', str(CACHE)] + NODE_DEPS,
                       check=True)
        if not (args.render_only or args.scene):
            return

    if args.render_only:
        cast = args.render_only.resolve()
        out = args.out or cast.parent
        out.mkdir(parents=True, exist_ok=True)
        header = json.loads(cast.read_text().split('\n', 1)[0])
        args.title = args.title or header.get('title', 'declass')
        summary = render(cast, out, args, log)
        log(f'{summary["frames"]} frames, {summary["seconds"]:.1f}s of playback in {out}')
        return

    try:
        import pyte  # noqa: F401
    except ImportError:
        fail('pyte is required to follow the screen: python3 -m pip install --user pyte')
    if not args.no_render:
        # Fail before a live session, not after it.
        if node_modules(args.node_modules) is None:
            fail('xterm.js and Playwright are missing; run once with --install-deps')
        if find_chrome(args.chrome) is None or shutil.which('ffmpeg') is None:
            fail('rendering needs Chrome or Chromium (--chrome PATH) and ffmpeg')
    scene = json.loads(args.scene.read_text())
    declass = shutil.which(args.declass) or fail(f'{args.declass} not found; pass --declass PATH')
    version = subprocess.run([declass, '--version'], capture_output=True, text=True).stdout.strip()
    stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S')
    out = (args.out or Path(tempfile.gettempdir()) / f'declass-recording-{stamp}').resolve()
    out.mkdir(parents=True, exist_ok=True)
    if (out / 'session.cast').exists():
        fail(f'{out / "session.cast"} exists; choose another --out')
    fixture, workspace = prepare_workspace(scene, args.scene.resolve().parent,
                                           Path(tempfile.gettempdir()))
    log(f'workspace {workspace}')
    if scene.get('check'):
        before = subprocess.run(scene['check'], shell=True, cwd=workspace, capture_output=True, text=True)
        (out / 'check-before.txt').write_text(before.stdout + before.stderr)
    cols, rows = scene.get('cols', 120), scene.get('rows', 36)
    env = {k: v for k, v in os.environ.items() if k not in ('NO_COLOR', 'COLUMNS', 'LINES')}
    env.update({'TERM': 'xterm-256color', 'COLORTERM': 'truecolor'})
    title = args.title = args.title or scene.get('title', f'declass — {workspace.name}')
    command = [declass] + scene.get('args', [])
    relay = None if args.no_local_relay else LocalRelay(declass, env, log)
    if relay is not None:
        env = relay.env
    log(f'running {" ".join(command)} at {cols}×{rows} with {version}')
    session = Session(command, workspace, env, cols, rows, out / 'session.cast', title)
    started = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds')
    error = None
    try:
        run_steps(session, scene['steps'], log)
    except (TimeoutError, RuntimeError) as e:
        error = str(e)
    finally:
        session.close()
        if relay is not None:
            relay.close()
    report = verify(scene, fixture, workspace, out)
    manifest = {
        'recorded_at': started, 'declass_version': version, 'declass_binary_sha256': sha256(declass),
        'recorder_commit': subprocess.run(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'],
                                        capture_output=True, text=True).stdout.strip(),
        'scene': str(args.scene), 'scene_sha256': sha256(args.scene),
        'terminal': {'cols': cols, 'rows': rows, 'TERM': env['TERM']},
        'workspace': str(workspace), 'cast_sha256': sha256(out / 'session.cast'),
        'local_model_relay': relay.describe() if relay is not None else None,
        'error': error, **report,
    }
    (out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    if error:
        fail(f'the scene stopped: {error}\ncast and manifest kept in {out}')
    if not args.no_render:
        summary = render(out / 'session.cast', out, args, log)
        manifest['playback'] = {'frames': summary['frames'], 'seconds': round(summary['seconds'], 2),
                                'fps': args.fps, 'speeds': summary['speeds']}
        (out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    log(f'done: {out}')
    print(json.dumps({k: manifest.get(k) for k in ('check', 'runs', 'canaries', 'playback')}, indent=2))


if __name__ == '__main__':
    main()
