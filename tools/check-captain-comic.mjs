#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Acceptance contract for the detailed-spec Duet Captain Comic artifact.
// Run inside the game's workspace with: node check-captain-comic.mjs index.html
// This executes the game's inline script with inert canvas/audio shims. It is
// a regression check for that artifact's structure, not a general game judge.

import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const file = process.argv[2] ?? 'index.html';
const html = readFileSync(file, 'utf8');
const inline = html.match(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/i)?.[1];
if (!inline) {
  console.error('FAIL: no inline game script found');
  process.exit(1);
}

const graphics = new Proxy({}, {
  get(target, key) {
    if (key === 'createImageData') {
      return (width, height) => ({ data: new Uint8ClampedArray(width * height * 4) });
    }
    return target[key] ?? (() => {});
  },
  set(target, key, value) {
    target[key] = value;
    return true;
  },
});
const canvas = () => ({ style: {}, getContext: () => graphics, width: 320, height: 200 });
const context = {
  document: { getElementById: canvas, createElement: canvas, addEventListener() {} },
  window: { innerWidth: 1280, innerHeight: 800, addEventListener() {} },
  requestAnimationFrame() {},
  performance: { now: () => 0 },
  Uint8ClampedArray,
  console: { log() {}, warn() {}, error() {} },
};

const inspect = `
globalThis.acceptance = { LEVEL, game, collect, renderTitle, renderStory };
globalThis.overflow = [];
const originalDrawText = drawText;
drawText = function(g, str, x, y, color, scale) {
  if (x < 0 || x + textW(str, scale) > W)
    overflow.push({ text: str, x, width: textW(str, scale) });
  return originalDrawText(g, str, x, y, color, scale);
};`;

try {
  vm.runInNewContext(`${inline}\n${inspect}`, context, { timeout: 5000 });
} catch (error) {
  console.error(`FAIL: game script could not load: ${error.message}`);
  process.exit(1);
}

const failures = [];
const { LEVEL, game, collect, renderTitle, renderStory } = context.acceptance;
const zones = Object.values(LEVEL);
if (zones.length !== 8 || zones.some(zone => zone.length !== 3)) {
  failures.push('expected eight zones with three segments each');
}
const rows = zones.flatMap(zone => zone.flatMap(segment => segment));
const cola = rows.reduce((count, row) => count + [...row].filter(tile => tile === 'C').length, 0);
if (cola !== 5) failures.push(`expected five actual Blastola Cola pickups, found ${cola}`);

game.screen = 'game';
game.items.gems = false;
game.items.gold = false;
collect('crown', 0, 0);
if (game.screen === 'victory') {
  failures.push('Crown grants victory without the Gems and Gold');
}
game.screen = 'game';
game.items.gems = true;
game.items.gold = true;
game.items.crown = false;
collect('crown', 0, 0);
if (game.screen !== 'victory') {
  failures.push('all three treasures do not grant victory');
}

for (const [name, render] of [['title', renderTitle], ['story', renderStory]]) {
  context.overflow = [];
  render();
  const examples = context.overflow.slice(0, 2).map(line => JSON.stringify(line.text));
  if (examples.length) failures.push(`${name} text extends beyond the canvas: ${examples.join(', ')}`);
}

if (failures.length) {
  for (const failure of failures) console.error(`FAIL: ${failure}`);
  process.exit(1);
}
console.log('PASS: map, pickups, treasure-gated victory and title/story text bounds');
