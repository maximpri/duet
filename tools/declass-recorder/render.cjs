#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Replays a recorded asciicast in xterm.js and writes one PNG per frame.
// Only playback time is changed: the scene's `speed:N` markers speed it up
// (the window shows the factor), pauses longer than `idle` seconds are cut
// short unless a `hold:N` marker asked for them, and `start`/`end` markers trim
// it. Output bytes are written to the terminal unchanged.
//
//   NODE_PATH=MODULES node render.cjs CAST OUT_DIR MODULES CHROME OPTIONS_JSON
const fs = require('node:fs');
const path = require('node:path');

// macOS Terminal.app's ANSI palette (Declass styles text with the terminal's own
// 16 colours), on the dark background of the native captures.
const THEME = {
  background: '#10121a', foreground: '#e3e5ea', cursor: '#9ea3ad', cursorAccent: '#10121a',
  selectionBackground: '#3a4150',
  black: '#000000', red: '#c23621', green: '#25bc24', yellow: '#adad27',
  blue: '#492ee1', magenta: '#d338d3', cyan: '#33bbc8', white: '#cbcccd',
  brightBlack: '#818383', brightRed: '#fc391f', brightGreen: '#31e722', brightYellow: '#eaec23',
  brightBlue: '#5833ff', brightMagenta: '#f935f8', brightCyan: '#14f0f0', brightWhite: '#e9ebeb',
};

const PAGE = `<!doctype html><meta charset="utf-8"><style>
html,body{margin:0;background:transparent}
#stage{display:inline-block;padding:34px 40px 40px;
  background:radial-gradient(120% 140% at 0% 0%,#1f4440 0%,#16222a 45%,#11141b 100%)}
#win{border-radius:11px;overflow:hidden;background:${THEME.background};
  box-shadow:0 0 0 1px rgba(255,255,255,.09),0 24px 60px rgba(0,0,0,.55),0 6px 18px rgba(0,0,0,.35)}
#bar{position:relative;height:30px;display:flex;align-items:center;
  background:linear-gradient(#34363b,#2a2c30);border-bottom:1px solid #1b1c1f}
.dots{display:flex;gap:8px;padding-left:13px}
.dots i{width:12px;height:12px;border-radius:50%;display:block;box-shadow:inset 0 0 0 .5px rgba(0,0,0,.25)}
#title{position:absolute;left:0;right:0;text-align:center;pointer-events:none;
  font:500 13px/30px -apple-system,BlinkMacSystemFont,"Helvetica Neue",sans-serif;color:#b4b7bd}
#speed{position:absolute;right:12px;top:6px;height:18px;padding:0 8px;border-radius:9px;display:none;
  font:600 11px/18px -apple-system,BlinkMacSystemFont,"Helvetica Neue",sans-serif;letter-spacing:.3px;
  color:#0c1a1c;background:#33bbc8}
#terminal{padding:10px 12px 12px}
.xterm-viewport{overflow:hidden!important}
</style><div id="stage"><div id="win"><div id="bar"><span class="dots"><i style="background:#ff5f57"></i>
<i style="background:#febc2e"></i><i style="background:#28c840"></i></span><span id="title"></span>
<span id="speed"></span></div><div id="terminal"></div></div></div>`;

function timeline(events, idle) {
  // Playback time for each output event, from the markers between them.
  let speed = 1, hold = 0, started = false, ended = false, clock = 0, last = null;
  const out = [], segments = [], stills = {};
  const startAt = events.some(e => e[1] === 'm' && e[2] === 'start');
  const preroll = [];
  for (const [t, kind, data] of events) {
    if (ended) break;
    const live = started || !startAt;
    if (live && last !== null) {
      const real = (t - last) / speed;
      clock += hold > 0 ? Math.min(real, hold) : Math.min(real, idle);
    }
    if (live) last = t;
    if (kind === 'm') {
      if (data === 'start') { started = true; last = t; }
      else if (data === 'end') ended = true;
      else if (data.startsWith('speed:')) speed = Math.max(Number(data.slice(6)) || 1, 0.1);
      else if (data.startsWith('hold:')) { hold = Number(data.slice(5)) || 0; continue; }
      else if (data.startsWith('still:')) stills[data.slice(6)] = clock;
      if (data.startsWith('speed:') || data === 'start') segments.push({ at: clock, speed });
      hold = 0;
      continue;
    }
    if (kind !== 'o') continue;
    if (live) { out.push({ at: clock, data }); hold = 0; }
    else preroll.push(data);
  }
  return { preroll: preroll.join(''), out, segments, stills, duration: clock };
}

(async () => {
  if (process.argv.length !== 7) throw new Error('usage: render.cjs CAST OUT_DIR MODULES CHROME OPTIONS_JSON');
  const [castFile, outDir, modules, chrome, optionsJson] = process.argv.slice(2);
  const options = JSON.parse(optionsJson);
  const [header, ...events] = fs.readFileSync(castFile, 'utf8').trim().split('\n').map(JSON.parse);
  if (header.version !== 2) throw new Error('only asciicast v2 is supported');
  const plan = timeline(events, options.idle);
  const fps = options.fps;
  const total = Math.max(1, Math.ceil(plan.duration * fps) + 1);
  const { chromium } = require(path.join(modules, 'playwright-core'));
  fs.mkdirSync(outDir, { recursive: true });
  const browser = await chromium.launch({ executablePath: chrome, headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 2400, height: 1600 }, deviceScaleFactor: options.scale });
    await page.setContent(PAGE);
    await page.addStyleTag({ path: path.join(modules, '@xterm/xterm/css/xterm.css') });
    await page.addScriptTag({ path: path.join(modules, '@xterm/xterm/lib/xterm.js') });
    await page.evaluate(async ({ cols, rows, theme, fontSize, title }) => {
      await document.fonts.ready;
      document.getElementById('title').textContent = title;
      window.term = new Terminal({
        cols, rows, theme, fontSize, lineHeight: 1.0, letterSpacing: 0, cursorBlink: false,
        cursorStyle: 'block', fontFamily: 'Menlo, "SF Mono", "DejaVu Sans Mono", monospace',
        drawBoldTextInBrightColors: false, minimumContrastRatio: 1, scrollback: 0, allowProposedApi: true,
      });
      term.open(document.getElementById('terminal'));
    }, { cols: header.width, rows: header.height, theme: THEME, fontSize: options.fontSize,
         title: options.title || header.title || 'declass' });
    const write = s => page.evaluate(d => new Promise(r => term.write(d, r)), s);
    const setSpeed = s => page.evaluate(v => {
      const el = document.getElementById('speed');
      el.style.display = v > 1 ? 'block' : 'none';
      el.textContent = `▸▸ ${v}× speed`;
    }, s);
    if (plan.preroll) await write(plan.preroll);
    const stage = page.locator('#stage');
    let next = 0, segment = 0, shown = null, previous = null;
    const speedAt = t => {
      while (segment + 1 < plan.segments.length && plan.segments[segment + 1].at <= t) segment++;
      return plan.segments.length ? plan.segments[segment].speed : 1;
    };
    const stillFrames = {};
    for (let k = 0; k < total; k++) {
      const t = k / fps;
      let bytes = '';
      while (next < plan.out.length && plan.out[next].at <= t) bytes += plan.out[next++].data;
      const speed = speedAt(t);
      const file = String(k).padStart(5, '0') + '.png';
      if (bytes) await write(bytes);
      if (speed !== shown) { await setSpeed(speed); shown = speed; bytes = bytes || 'speed'; }
      if (bytes || previous === null) {
        await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
        await stage.screenshot({ path: path.join(outDir, file) });
      } else {
        fs.copyFileSync(path.join(outDir, previous), path.join(outDir, file));
      }
      previous = file;
      // A still is the last frame at or before its marker: the marker is
      // recorded just before the next key, whose output can follow at once.
      for (const [name, at] of Object.entries(plan.stills)) {
        if (t <= at + 1e-9) stillFrames[name] = file;
      }
      if (k % (fps * 5) === 0) process.stderr.write(`\rrender: ${Math.round(100 * k / total)}%`);
    }
    process.stderr.write('\rrender: 100%\n');
    const speeds = [...new Set(plan.segments.map(s => s.speed))];
    fs.writeFileSync(path.join(outDir, 'frames.json'), JSON.stringify(
      { frames: total, seconds: total / fps, stills: stillFrames, speeds }, null, 2));
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(`render: ${error.stack || error.message}`);
  process.exitCode = 1;
});
