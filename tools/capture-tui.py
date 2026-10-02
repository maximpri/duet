#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Record an actual PTY as asciicast v2 and display its bytes with xterm.js.

Install @xterm/xterm@5.5.0 outside the repo; pass its directory with --xterm.
This is a terminal bridge, not a simulated Duet screen. Only the loopback
listener with the printed random URL can read/write the disposable PTY.
"""
import argparse
import base64
import codecs
import fcntl
import http.server
import json
import os
from pathlib import Path
import secrets
import signal
import struct
import termios
import threading
import time
import urllib.parse


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--xterm', type=Path, required=True)
    p.add_argument('--cast', type=Path, required=True)
    p.add_argument('--cwd', type=Path, required=True)
    p.add_argument('--port', type=int, default=8767)
    p.add_argument('--cols', type=int, default=130)
    p.add_argument('--rows', type=int, default=36)
    p.add_argument('command', nargs=argparse.REMAINDER)
    a = p.parse_args()
    command = a.command[1:] if a.command[:1] == ['--'] else a.command
    if not command:
        p.error('a command is required after --')
    assets = {n: (a.xterm / n).read_bytes() for n in ['lib/xterm.js', 'css/xterm.css']}
    token = secrets.token_urlsafe(24)
    events = []
    lock = threading.Lock()
    started = time.monotonic()
    a.cast.parent.mkdir(parents=True, exist_ok=True)
    cast = a.cast.open('x', encoding='utf-8')
    os.chmod(a.cast, 0o600)
    cast.write(json.dumps({'version': 2, 'width': a.cols, 'height': a.rows,
                          'timestamp': int(time.time()), 'env': {'TERM': 'xterm-256color'}}) + '\n')
    cast.flush()
    pid, fd = os.forkpty()
    if pid == 0:
        os.chdir(a.cwd)
        os.environ['TERM'] = 'xterm-256color'
        os.environ['COLORTERM'] = 'truecolor'
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', a.rows, a.cols, 0, 0))
        os.execvpe(command[0], command, os.environ)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', a.rows, a.cols, 0, 0))

    def read():
        decoder = codecs.getincrementaldecoder('utf-8')('replace')
        while True:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            with lock:
                events.append(base64.b64encode(data).decode())
                text = decoder.decode(data)
                if text:
                    cast.write(json.dumps([round(time.monotonic() - started, 4), 'o', text]) + '\n')
                    cast.flush()

    threading.Thread(target=read, daemon=True).start()
    page = '''<!doctype html><meta charset="utf-8"><title>Duet · live PTY</title>
<link rel="stylesheet" href="css/xterm.css">
<style>html,body{margin:0;background:#11151c}#terminal{padding:18px;display:inline-block}
.xterm-viewport{overflow:hidden!important}</style><div id="terminal"></div>
<script src="lib/xterm.js"></script><script>
const term = window.term = new Terminal({cols:COLS,rows:ROWS,fontFamily:'Menlo, monospace',
fontSize:16,lineHeight:1.15,cursorBlink:false,theme:{background:'#11151c',foreground:'#e0e5ee'}});
term.open(document.getElementById('terminal')); term.focus();
window.send = data => fetch('input',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({data})});
term.onData(window.send);
let index=0;
async function poll(){const r=await fetch('output?from='+index);const d=await r.json();
for(const s of d.events){await new Promise(resolve=>term.write(Uint8Array.from(atob(s),c=>c.charCodeAt(0)),resolve));}
index=d.next; window.received=index;setTimeout(poll,80)} poll();
window.screenText=()=>Array.from({length:term.rows},(_,i)=>term.buffer.active.getLine(i+term.buffer.active.viewportY)?.translateToString(true)||'').join('\\n');
</script>'''.replace('COLS', str(a.cols)).replace('ROWS', str(a.rows)).encode()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def reply(self, status, body, content_type='application/json'):
            self.send_response(status)
            self.send_header('Content-Type', content_type)
            self.send_header('Cache-Control', 'no-store')
            self.send_header('X-Content-Type-Options', 'nosniff')
            self.end_headers()
            self.wfile.write(body)

        def authorized(self):
            return self.path.startswith('/' + token + '/') and self.headers.get('Host') == f'127.0.0.1:{a.port}'

        def do_GET(self):
            if not self.authorized():
                return self.reply(404, b'{}')
            path = urllib.parse.urlsplit(self.path)
            name = path.path[len(token) + 2:]
            if not name:
                return self.reply(200, page, 'text/html; charset=utf-8')
            if name in assets:
                return self.reply(200, assets[name], 'text/javascript' if name.endswith('.js') else 'text/css')
            if name == 'output':
                try:
                    offset = max(0, int(urllib.parse.parse_qs(path.query).get('from', ['0'])[0]))
                except ValueError:
                    return self.reply(400, b'{}')
                with lock:
                    payload = json.dumps({'events': events[offset:], 'next': len(events)}).encode()
                return self.reply(200, payload)
            self.reply(404, b'{}')

        def do_POST(self):
            origin = self.headers.get('Origin')
            if not self.authorized() or self.path != '/' + token + '/input' or origin not in (None, f'http://127.0.0.1:{a.port}'):
                return self.reply(403, b'{}')
            try:
                length = int(self.headers.get('Content-Length', '0'))
                if not 0 < length <= 65536:
                    return self.reply(413, b'{}')
                data = json.loads(self.rfile.read(length))['data'].encode()
                os.write(fd, data)
            except (KeyError, ValueError, AttributeError, OSError):
                return self.reply(400, b'{}')
            self.reply(200, b'{}')

    server = http.server.ThreadingHTTPServer(('127.0.0.1', a.port), Handler)
    print(f'http://127.0.0.1:{a.port}/{token}/', flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        try:
            os.kill(pid, signal.SIGHUP)
        except ProcessLookupError:
            pass
        os.close(fd)
        cast.close()


if __name__ == '__main__':
    main()
