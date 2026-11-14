#!/usr/bin/env node
// Writes cues-wide.json, cues-tall.json and cues-sting.json from the beat times in
// ../source/index.html. Run it again if the animation timing changes:
//
//   node brand/video/audio/make-cues.mjs
//
// The cue sheets are plain JSON and are what mix.mjs reads. For a one-off nudge, edit the JSON by
// hand. For anything structural, change the tables below and regenerate (this overwrites the JSON).
//
// Every time below is quoted from index.html; the comment on each line names the expression it
// comes from. Scene 4's book events are not hand-placed: simulate() is a line-for-line port of the
// film's own seeded simulation, so a tick lands on exactly the frames where a trade prints.
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const r3 = (x) => Math.round(x * 1000) / 1000;

// ---- scene 4: which 200 ms steps print a trade, add an order, or roll the signed block ----
function rng(seed) { let s = seed >>> 0; return () => { s = (s * 1664525 + 1013904223) >>> 0; return s / 4294967296; }; }
function bookEvents(steps) {
  const r = rng(11), A = [], B = [], ev = [];
  const lv = () => { const n = 1 + (r() * 5 | 0), o = []; for (let i = 0; i < n; i++) o.push(.15 + r() * r() * 1.6); return o; };
  const hex = (n) => { for (let i = 0; i < n; i++) r(); };
  const trade = () => { const buy = r() > .48; r(); r(); r(); return buy; }; // clock, then size = r()*r()
  for (let i = 0; i < 7; i++) { A.push(lv()); B.push(lv()); }
  for (let k = 0; k < 13; k++) trade();
  for (let n = 0; n < steps; n++) {
    const x = r(), side = r() > .5 ? A : B, idx = r() * 7 | 0, Lv = side[idx]; const e = { step: n + 1 };
    if (x < .36) { const buy = trade(), mine = buy ? A : B, other = buy ? B : A; e.trade = buy ? 'buy' : 'sell'; mine[0].shift(); if (!mine[0].length) { mine.shift(); mine.push(lv()); other.unshift([.3 + r()]); other.pop(); } }
    else if (x < .74) { if (Lv.length < 7) { Lv.push(.15 + r() * r() * 1.6); e.fresh = true; } }
    else if (Lv.length > 1) Lv.splice(r() * Lv.length | 0, 1);
    if (r() < .3) { e.block = true; hex(6); hex(4); }
    ev.push(e);
  }
  return ev;
}

// ---- the film (wide and tall share every time; only the pans differ) ----
function film(tall) {
  // Pan is -1 (left) to +1 (right). Wide: copy sits left, tape / panel / mirrors sit right.
  // Tall: everything is stacked on the centre line, so only the diagram's mirrors spread.
  const P = tall
    ? { text: 0, rows: 0, stampL: -0.15, stampR: 0.15, panel: 0, items: 0, nodes: [-0.3, 0.3], blk: -0.2, mirrors: 0, cdn: -0.45, chip: 0.35 }
    : { text: -0.35, rows: 0.4, stampL: 0.3, stampR: 0.5, panel: 0.4, items: -0.35, nodes: [-0.45, -0.45], blk: -0.05, mirrors: 0.45, cdn: 0.45, chip: -0.05 };
  const c = [];
  const cue = (time, sfx, gain, pan, note) => c.push({ time: r3(time), sfx, gain, pan: r3(pan), note });

  // scene 1: logo sting
  cue(0.20, 'draw', -14, 0, 'wordmark draws on: seg(t, .2, 1.35)');
  cue(1.25, 'tongue-pop', -5, 0, 'tongue pops: pop(seg(t, 1.25, 2.15))');
  cue(1.72, 'blip', -15, 0.1, 'pink dot: back(seg(t, 1.7, 2.0))');
  cue(3.10, 'whoosh', -17, 0, 'lockup lifts and fades: seg(t, 3.1, 3.55)');

  // scene 2: the problem
  for (let i = 0; i < 9; i += 2) cue(3.6 + i * 0.07, 'tick', -24 - i * 0.5, P.rows, `tape row ${i} rises in: 3.6 + i*.07`);
  // prices snap 6 times a second (Math.floor(t * 6)); sound every third change until the bars cover them
  for (let f = 26; f / 6 < 5.5; f += 3) cue(f / 6, 'tick', -27, P.rows, `tape prices snap: frame ${f} of floor(t*6)`);
  cue(5.15, 'deflate', -11, P.rows, 'grey bars wipe over the prices: 5.15 + i*.09');
  [[6.20, 'DELAYED', P.stampL], [6.75, 'PER USER · PER DEVICE', P.stampR], [7.30, 'DO NOT REDISTRIBUTE', P.stampL], [7.85, 'CALL SALES', P.stampR]]
    .forEach(([t, name, pan], i) => cue(t + 0.07, 'stamp', -10 + i * 0.5, pan, `stamp ${name}: seg(t, ${t.toFixed(2)}, ${(t + 0.22).toFixed(2)}), ease-out-quint, 86 % landed 70 ms in`));

  // tongue wipe 1
  cue(9.20, 'whoosh-down', -11, 0, 'tongue wipe drops: doWipe(t, 9.2), covers by 9.65');
  cue(9.75, 'slurp', -10, 0, 'wipe slurps back up: seg(t, 9.75, 10.22)');

  // scene 3: the bleh answer
  cue(10.12, 'blip', -17, 0.1, 'dot on the poster: back(seg(t, 10.1, 10.45))');
  cue(10.35, 'tongue-pop', -3, 0, 'THE drop. Poster tongue pops: pop(seg(t, 10.35, 11.25))');
  cue(12.45, 'whoosh', -14, 0, 'poster slides up and away: seg(t, 12.45, 13.05)');

  // scene 4: what you get
  for (let i = 0; i < 5; i++) cue(13.3 + i * 1.45, 'blip', -18, P.items, `line ${i + 1} arrives: 13.3 + ${i}*1.45`);
  // book steps: drawBook(floor((t - 13.2) * 5)), i.e. step n appears at 13.2 + n/5
  let lastChirp = -9;
  for (const e of bookEvents(Math.floor((20.6 - 13.2) * 5))) {
    const t = 13.2 + e.step / 5;
    if (t < 13.4) continue; // panel is still fading in
    if (e.trade) cue(t, 'tick', -23, P.panel + (e.trade === 'buy' ? 0.08 : -0.08), `trade prints (${e.trade}): book step ${e.step}`);
    else if (e.fresh && t - lastChirp >= 0.6) { cue(t, 'data-chirp', -25, P.panel - 0.1, `new order ringed in the book: step ${e.step}`); lastChirp = t; }
  }
  cue(20.0, 'blip', -15, P.items, 'all five lines light up: t > 20.0');

  // scene 5: how it stays up
  cue(20.6, 'whoosh', -15, 0, 'cream scene slides up: seg(t, 20.6, 21.25)');
  cue(21.12, 'blip', -19, P.nodes[0], 'node A pops: seg(t, 21.1, 21.5)');
  cue(21.24, 'blip', -19, P.nodes[1], 'node B pops: seg(t, 21.22, 21.62)');
  cue(21.57, 'blip', -17, P.blk, 'signed block pops: seg(t, 21.55, 21.95)');
  cue(22.08, 'signed', -7, P.blk, 'pink tick lands on the block: back(seg(t, 22.05, 22.4))');
  cue(22.85, 'sparkle', -9, P.mirrors, 'three mirrors pop 130 ms apart: 22.85 + i*.13 (the pings in the file use the same stagger)');
  cue(24.02, 'blip', -17, P.chip, '"timestamped on Bitcoin" chip: seg(t, 24.0, 24.35)');
  cue(24.95, 'powerdown', -9, P.cdn, 'CDN crossed out, its packets stop: seg(t, 24.95, 25.3)');
  cue(25.65, 'shrug', -8, tall ? 0.15 : P.mirrors, '"Nothing happens." The three live mirrors bounce: t > 25.65');

  // tongue wipe 2 + end card
  cue(26.20, 'whoosh-down', -11, 0, 'tongue wipe drops: doWipe(t, 26.2)');
  cue(26.75, 'slurp', -10, 0, 'wipe slurps back up: seg(t, 26.75, 27.22)');
  cue(27.02, 'blip', -17, 0.1, 'dot: back(seg(t, 27.0, 27.35))');
  cue(27.20, 'tongue-pop', -4, 0, 'end-card tongue pops: pop(seg(t, 27.2, 28.1))');
  cue(27.85, 'button', -8, 0, 'yellow URL pill arrives: seg(t, 27.8, 28.3). The button hit');
  return c.sort((a, b) => a.time - b.time);
}

function sting() {
  const c = [];
  const cue = (time, sfx, gain, pan, note) => c.push({ time, sfx, gain, pan, note });
  cue(0.20, 'draw', -12, 0, 'wordmark draws on: seg(t, .2, 1.35)');
  cue(1.25, 'tongue-pop', -4, 0, 'tongue pops: pop(seg(t, 1.25, 2.15))');
  cue(1.72, 'blip', -14, 0.1, 'pink dot: back(seg(t, 1.7, 2.0))');
  cue(2.30, 'button', -11, 0, 'tagline fades up: seg(t, 2.3, 2.8). Soft resolve');
  cue(3.30, 'lick', -7, 0, 'second small lick: sin((t - 3.3) * 10), 3.3 to 4.1');
  return c;
}

const head = (format, video, duration) => ({
  format, video, duration,
  comment: 'time = seconds from the first frame to the start of the sound. sfx = file in sfx/ without .wav. gain = dB applied to the file (files are peak-normalised, see sfx/make-sfx.sh). pan = -1 left .. 0 centre .. +1 right. note is for humans. Generated by make-cues.mjs; safe to edit by hand.',
});
const write = (name, obj) => { writeFileSync(join(here, name), JSON.stringify(obj, null, 1).replace(/\{\n\s+"time": ([^\n]+)\n\s+"sfx": ([^\n]+)\n\s+"gain": ([^\n]+)\n\s+"pan": ([^\n]+)\n\s+"note": ([^\n]+)\n\s+\}/g, '{ "time": $1 "sfx": $2 "gain": $3 "pan": $4 "note": $5 }') + '\n'); console.log(`${name}: ${obj.cues.length} cues`); };
write('cues-wide.json', { ...head('wide', 'bleh-30s-1920x1080.mp4', 30), cues: film(false) });
write('cues-tall.json', { ...head('tall', 'bleh-30s-1080x1920.mp4', 30), cues: film(true) });
write('cues-sting.json', { ...head('sting', 'bleh-sting-1080x1080.mp4', 5), cues: sting() });
