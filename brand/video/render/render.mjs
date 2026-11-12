#!/usr/bin/env node
// Renders ../source/index.html to video by stepping window.renderAt(t) frame by frame in headless Chrome
// and piping PNG frames into ffmpeg. No npm dependencies. Usage:
//   node render.mjs                # everything
//   node render.mjs wide tall      # only some of: wide tall sting loop
//   node render.mjs --still wide 10.6 out.png   # one frame
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { launch } from './cdp.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const OUT = join(here, '..');
const src = (format) => pathToFileURL(join(here, '../source/index.html')).href + `?render=1&format=${format}`;
const X264 = ['-c:v', 'libx264', '-preset', 'slow', '-crf', '17', '-pix_fmt', 'yuv420p', '-profile:v', 'high', '-movflags', '+faststart', '-an'];
const JOBS = {
  wide: [['bleh-30s-1920x1080.mp4', X264]],
  tall: [['bleh-30s-1080x1920.mp4', X264]],
  sting: [['bleh-sting-1080x1080.mp4', X264]],
  loop: [
    ['bleh-sting-loop.webm', ['-vf', 'scale=720:720:flags=lanczos', '-c:v', 'libvpx-vp9', '-crf', '30', '-b:v', '0', '-pix_fmt', 'yuv420p', '-an']],
    ['bleh-sting-loop.gif', ['-filter_complex', '[0:v]fps=25,scale=480:480:flags=lanczos,split[a][b];[a]palettegen=max_colors=32:stats_mode=diff[p];[b][p]paletteuse=dither=none', '-loop', '0']],
  ],
};

async function open(page, format) {
  await page.goto(src(format));
  const film = JSON.parse(await page.eval('JSON.stringify(window.FILM)'));
  await page.viewport(film.width, film.height, 1);
  return film;
}

async function render(page, format) {
  const film = await open(page, format);
  const frames = Math.round(film.duration * film.fps);
  const encoders = JOBS[format].map(([name, args]) => {
    const ff = spawn('ffmpeg', ['-y', '-loglevel', 'error', '-f', 'image2pipe', '-framerate', String(film.fps), '-c:v', 'png', '-i', '-', ...args, join(OUT, name)], { stdio: ['pipe', 'inherit', 'inherit'] });
    return { name, ff, done: once(ff, 'close') };
  });
  const t0 = Date.now();
  for (let i = 0; i < frames; i++) {
    await page.eval(`window.renderAt(${i / film.fps})`);
    const png = await page.png();
    for (const e of encoders) if (!e.ff.stdin.write(png)) await once(e.ff.stdin, 'drain');
    if (i % 150 === 0) console.log(`${format}: frame ${i}/${frames}`);
  }
  for (const e of encoders) { e.ff.stdin.end(); const [code] = await e.done; if (code) throw new Error(`ffmpeg failed for ${e.name}`); console.log(`wrote ${e.name}`); }
  console.log(`${format}: ${frames} frames in ${((Date.now() - t0) / 1000).toFixed(0)} s`);
}

const args = process.argv.slice(2);
const page = await launch();
try {
  if (args[0] === '--still') {
    await open(page, args[1]);
    await page.eval(`window.renderAt(${+args[2]})`);
    writeFileSync(args[3], await page.png());
  } else {
    for (const f of (args.length ? args : Object.keys(JOBS))) await render(page, f);
  }
} finally { await page.close(); }
