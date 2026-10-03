#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Render genuine recorded PTY bytes in xterm.js; no model/UI text is synthesized.
// NODE_PATH=/path/to/capture/node_modules node tools/render-tui-cast.cjs \
//   CAST OUTPUT_DIR XTERM_DIR CHROME_PATH TIMESTAMPS_JSON
// TIMESTAMPS_JSON is a list of original recording timestamps in seconds.
const fs = require('node:fs');
const path = require('node:path');
(async () => {
  if (process.argv.length !== 7) {
    throw new Error('Usage: render-tui-cast.cjs CAST OUTPUT_DIR XTERM_DIR CHROME_PATH TIMESTAMPS_JSON');
  }
  const [cast, out, xterm, chrome, timesFile] = process.argv.slice(2);
  const lines = fs.readFileSync(cast, 'utf8').trim().split('\n').map(JSON.parse);
  const header = lines.shift();
  if (!header || header.version !== 2 ||
      ![header.width, header.height].every(n => Number.isInteger(n) && n > 0)) {
    throw new Error('cast must have an asciicast v2 header with positive terminal dimensions');
  }
  if (!lines.every((e, i) => Array.isArray(e) && Number.isFinite(e[0]) && e[0] >= 0 &&
      typeof e[1] === 'string' && typeof e[2] === 'string' && (!i || e[0] >= lines[i - 1][0]))) {
    throw new Error('cast events must contain sorted nonnegative timestamps, event types, and strings');
  }
  const events = lines.filter(e => e[1] === 'o');
  const times = JSON.parse(fs.readFileSync(timesFile, 'utf8'));
  if (!Array.isArray(times) || !times.length ||
      !times.every((n, i) => Number.isFinite(n) && n >= 0 && (!i || n >= times[i - 1]))) {
    throw new Error('timestamps must be a nonempty array of nonnegative, sorted numbers');
  }
  const { chromium } = require('@playwright/test');
  fs.mkdirSync(out, { recursive: true });
  const browser = await chromium.launch({ executablePath: chrome, headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 });
    await page.setContent('<style>body{margin:0;background:#11151c}#terminal{padding:18px;display:inline-block}.xterm-viewport{overflow:hidden!important}</style><div id="terminal"></div>');
    await page.addStyleTag({ path: path.join(xterm, 'css/xterm.css') });
    await page.addScriptTag({ path: path.join(xterm, 'lib/xterm.js') });
    await page.evaluate(({ width, height }) => {
      window.term = new Terminal({ cols: width, rows: height, fontFamily: 'Menlo, monospace', fontSize: 16,
        lineHeight: 1.15, cursorBlink: false, theme: { background: '#11151c', foreground: '#e0e5ee' } });
      term.open(document.getElementById('terminal'));
    }, header);
    let at = 0;
    const frames = [];
    for (let i = 0; i < times.length; i++) {
      let bytes = '';
      while (at < events.length && events[at][0] <= times[i]) bytes += events[at++][2];
      await page.evaluate(s => new Promise(resolve => term.write(s, resolve)), bytes);
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      const file = String(i).padStart(5, '0') + '.png';
      await page.locator('#terminal').screenshot({ path: path.join(out, file) });
      const text = await page.evaluate(() => Array.from({ length: term.rows }, (_, n) => term.buffer.active.getLine(n + term.buffer.active.viewportY)?.translateToString(true) || '').join('\n'));
      frames.push({ file, source_seconds: times[i], text });
    }
    fs.writeFileSync(path.join(out, 'frames.json'), JSON.stringify(frames, null, 2) + '\n');
  } finally { await browser.close(); }
})().catch(error => {
  console.error(`render-tui-cast: ${error.message}`);
  process.exitCode = 1;
});
