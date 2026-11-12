#!/usr/bin/env node
// bleh.xyz brand film: sound mixer. Node 22+, no npm packages; shells out to ffmpeg / ffprobe.
//
//   node brand/video/audio/mix.mjs [--music path] [--sting-music path] [options]
//
// For each of wide / tall / sting it:
//   1. takes a music file, level-matches it, trims and fades it to the exact film length;
//   2. places every sound effect from the cue sheet (time, gain, pan);
//   3. ducks the music under the effects (sidechain compressor keyed by the SFX bus);
//   4. applies a gentle master EQ and a peak limiter;
//   5. normalises to -14 LUFS integrated with two-pass loudnorm, true peak under -1 dBTP,
//      then checks the result on the EBU R128 meter and trims any residual;
//   6. writes audio/out/<film>-mix.wav and <film>-sfx.wav (SFX-only stem), and muxes
//      AAC 192 kb/s 48 kHz into ../<film>-sound.mp4 with the video stream copied untouched;
//   7. measures the finished MP4 (EBU R128) and prints integrated loudness and true peak.
//
// Music lookup
//   films:  --music, else music/theme.wav, else music/placeholder-bed.wav
//   sting:  --sting-music, else music/sting.wav, else the last bars of the film theme (from
//           26.04 s, so the theme's final hook and hit play under the logo; only when a real
//           theme is present), else music/placeholder-sting.wav
//
// Options
//   --music <file>          theme for the two 30 s films
//   --sting-music <file>    separate 5 s sting music
//   --music-offset <s>      seconds into the theme where the film's first frame sits. Positive
//                           skips into the file, negative delays the music. Default 0.
//   --sting-offset <s>      the same for the sting music. Default 0 for a separate sting file,
//                           music offset + 26.04 when the sting is cut from the theme.
//   --music-level <dB>      music trim relative to the default balance. Default 0.
//   --sfx-level <dB>        trim for every sound effect. Default 0.
//   --fade-out <s>          music fade-out at the end of the films. Default 0.25.
//   --no-duck               leave the music level alone under effects
//   --only wide,tall,sting  do some of them
//   --lufs <n> / --tp <n>   targets. Defaults -14 and -1.
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const VIDEO = join(here, '..');
const OUT = join(here, 'out');
const SR = 48000;

// ---- mix constants (the knobs worth knowing about) ----
const MUSIC_BED_LUFS = -17;   // music is first level-matched to this, so cue gains mean the same under any track
const DUCK = { threshold: 0.1, ratio: 2.5, attack: 8, release: 220, knee: 4 }; // 0.1 = -20 dBFS: ticks and whooshes pass under it, pops and stamps duck 7-11 dB
const MASTER_EQ = 'highpass=f=30,equalizer=f=250:t=q:w=1:g=-1,equalizer=f=3500:t=q:w=1.2:g=1'; // clear a little mud, a little presence for phone speakers
const STING_FROM_THEME = 26.04; // with no sting music, the sting uses the theme's ending: its end-card pop (27.29 s) lands on the sting's pop (1.25 s)
const AAC_HEADROOM = 0.5;     // loudnorm aims this far under the true-peak target, because AAC encoding overshoots a little

// ---- args ----
const argv = process.argv.slice(2), opt = {};
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a === '--no-duck') opt.noDuck = true;
  else if (a === '-h' || a === '--help') { console.log(readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n').slice(1).filter((l, i, all) => all.slice(0, i + 1).every((x) => x.startsWith('//'))).map((l) => l.replace(/^\/\/ ?/, '')).join('\n')); process.exit(0); }
  else if (a.startsWith('--') && i + 1 < argv.length) opt[a.slice(2)] = argv[++i];
  else die(`unknown argument: ${a}`);
}
const num = (k, d) => (opt[k] === undefined ? d : Number.isFinite(+opt[k]) ? +opt[k] : die(`--${k} needs a number`));
const LUFS = num('lufs', -14), TP = num('tp', -1);
const only = opt.only ? opt.only.split(',') : ['wide', 'tall', 'sting'];

function die(msg) { console.error(`mix: ${msg}`); process.exit(1); }
function run(bin, args) {
  const r = spawnSync(bin, args, { encoding: 'utf8', maxBuffer: 1 << 28 });
  if (r.error) die(`${bin}: ${r.error.message}`);
  if (r.status) die(`${bin} failed:\n${r.stderr.split('\n').slice(-15).join('\n')}`);
  return r;
}
const ff = (...args) => run('ffmpeg', ['-hide_banner', '-nostdin', '-y', ...args]).stderr;
const probe = (file, entries, stream) => JSON.parse(run('ffprobe', ['-v', 'error', ...(stream ? ['-select_streams', stream] : []), '-show_entries', entries, '-of', 'json', file]).stdout);
const rel = (p) => relative(process.cwd(), p) || p;
const db = (x) => `${x >= 0 ? '+' : ''}${x.toFixed(1)}`;

// EBU R128 of a file's audio (optionally only the first `dur` seconds from `start`)
function r128(file, start, dur) {
  const pre = start !== undefined ? ['-ss', String(start), '-t', String(dur)] : [];
  const log = ff(...pre, '-i', file, '-map', '0:a:0', '-af', 'ebur128=peak=true', '-f', 'null', '-');
  const s = log.slice(log.lastIndexOf('Summary:'));
  const g = (re) => { const m = re.exec(s); return m ? +m[1] : NaN; };
  return { I: g(/I:\s+(-?[\d.]+|-inf) LUFS/), LRA: g(/LRA:\s+(-?[\d.]+) LU/), TP: g(/Peak:\s+(-?[\d.]+|-inf) dBFS/) };
}
function loudnormJson(log) { const i = log.lastIndexOf('{'), j = log.lastIndexOf('}'); return JSON.parse(log.slice(i, j + 1)); }

function pickMusic(format) {
  const theme = opt.music
    ? resolve(opt.music)
    : ['music/theme.wav', 'music/theme.flac'].map((p) => join(here, p)).find((p) => existsSync(p)) ?? join(here, 'music/theme.wav');
  const haveTheme = existsSync(theme);
  if (opt.music && !haveTheme) die(`music file not found: ${opt.music}`);
  if (format !== 'sting') {
    if (haveTheme) return { file: theme, offset: num('music-offset', 0), fade: num('fade-out', 0.25), kind: 'theme' };
    return { file: join(here, 'music/placeholder-bed.wav'), offset: 0, fade: num('fade-out', 0.25), kind: 'PLACEHOLDER bed' };
  }
  const own = opt['sting-music'] ? resolve(opt['sting-music']) : join(here, 'music/sting.wav');
  if (opt['sting-music'] && !existsSync(own)) die(`sting music not found: ${opt['sting-music']}`);
  if (existsSync(own)) return { file: own, offset: num('sting-offset', 0), fade: 0.25, kind: 'sting music' };
  if (haveTheme) return { file: theme, offset: num('sting-offset', num('music-offset', 0) + STING_FROM_THEME), fade: 0.5, kind: `cut from the theme at ${(num('sting-offset', num('music-offset', 0) + STING_FROM_THEME)).toFixed(2)} s` };
  return { file: join(here, 'music/placeholder-sting.wav'), offset: 0, fade: 0.25, kind: 'PLACEHOLDER sting' };
}

function mix(format) {
  const sheet = JSON.parse(readFileSync(join(here, `cues-${format}.json`), 'utf8'));
  const D = sheet.duration, video = join(VIDEO, sheet.video), base = sheet.video.replace(/\.mp4$/, '');
  if (!existsSync(video)) die(`video not found: ${rel(video)}. Render it first (brand/video/render/render.mjs).`);
  const vdur = +probe(video, 'stream=duration', 'v:0').streams[0].duration;
  if (Math.abs(vdur - D) > 0.02) die(`${sheet.video} is ${vdur} s but cues-${format}.json says ${D} s`);
  const music = pickMusic(format);
  if (!existsSync(music.file)) die(`no music: ${rel(music.file)} is missing. Run music/make-placeholder.mjs or pass --music.`);
  const cues = sheet.cues.map((c, i) => {
    const file = join(here, 'sfx', `${c.sfx}.wav`);
    if (!existsSync(file)) die(`cues-${format}.json cue ${i}: sfx/${c.sfx}.wav is missing. Run sfx/make-sfx.sh.`);
    if (!(c.time >= 0 && c.time < D)) die(`cues-${format}.json cue ${i}: time ${c.time} is outside 0..${D}`);
    return { ...c, file, gain: (c.gain ?? 0) + num('sfx-level', 0), pan: Math.max(-1, Math.min(1, c.pan ?? 0)) };
  });
  console.log(`\n${format}: ${sheet.video}  ${D} s  ${cues.length} cues  music: ${rel(music.file)} (${music.kind})`);

  // 1. level-match the music: measure the stretch that will be used, gain it to MUSIC_BED_LUFS
  const skip = Math.max(0, music.offset), delay = Math.max(0, -music.offset);
  const m = r128(music.file, skip, D);
  if (!Number.isFinite(m.I)) die(`could not measure ${rel(music.file)} (silent, or shorter than the offset?)`);
  const mgain = MUSIC_BED_LUFS - m.I + num('music-level', 0);
  console.log(`  music in: ${m.I.toFixed(1)} LUFS, gain ${db(mgain)} dB to sit at ${MUSIC_BED_LUFS + num('music-level', 0)} LUFS under the effects`);

  // 2-4. one filtergraph: music bed, panned SFX, duck, master EQ
  const g = [];
  g.push(`[0:a]aresample=${SR},aformat=sample_fmts=fltp:channel_layouts=stereo,atrim=start=${skip},asetpts=PTS-STARTPTS`
    + (delay ? `,adelay=${Math.round(delay * SR)}S:all=1` : '')
    + `,apad=whole_dur=${D},atrim=end=${D},afade=t=in:d=0.01,afade=t=out:st=${(D - music.fade).toFixed(3)}:d=${music.fade},volume=${mgain.toFixed(2)}dB[mus]`);
  cues.forEach((c, i) => {
    const th = (c.pan + 1) * Math.PI / 4, L = Math.SQRT2 * Math.cos(th), R = Math.SQRT2 * Math.sin(th); // constant power, unity at centre
    g.push(`[${i + 1}:a]aresample=${SR},aformat=sample_fmts=fltp:channel_layouts=mono,volume=${c.gain.toFixed(2)}dB,pan=stereo|c0=${L.toFixed(4)}*c0|c1=${R.toFixed(4)}*c0,adelay=${Math.round(c.time * SR)}S:all=1[s${i}]`);
  });
  // a silent stereo base of exactly D seconds fixes the SFX bus length (do not rely on padding the last cue's tail)
  g.push(`anullsrc=r=${SR}:cl=stereo,atrim=end_sample=${Math.round(D * SR)}[base]`);
  g.push(`[base]${cues.map((_, i) => `[s${i}]`).join('')}amix=inputs=${cues.length + 1}:normalize=0:duration=first,asplit=3[sfx][key][stem]`);
  g.push(opt.noDuck ? '[key]anullsink;[mus]anull[bed]'
    : `[mus][key]sidechaincompress=threshold=${DUCK.threshold}:ratio=${DUCK.ratio}:attack=${DUCK.attack}:release=${DUCK.release}:knee=${DUCK.knee}:makeup=1[bed]`);
  g.push(`[bed][sfx]amix=inputs=2:normalize=0,${MASTER_EQ}[pre]`);
  mkdirSync(OUT, { recursive: true });
  const pre = join(OUT, `.${base}-pre.wav`), wav = join(OUT, `${base}-mix.wav`), stem = join(OUT, `${base}-sfx.wav`), mp4 = join(VIDEO, `${base}-sound.mp4`);
  ff('-v', 'error', '-i', music.file, ...cues.flatMap((c) => ['-i', c.file]), '-filter_complex', g.join(';'),
    '-map', '[pre]', '-t', String(D), '-c:a', 'pcm_f32le', pre,
    '-map', '[stem]', '-t', String(D), '-c:a', 'pcm_s24le', stem);

  // 5. loudness. A film mix is sparse (loud pops over a quiet bed), so one plain gain to -14 LUFS
  //    would push the pops over the ceiling and loudnorm would fall back to its dynamic mode, which
  //    pumps. So: gain to target, catch the few peaks with a limiter set 0.8 dB under the ceiling,
  //    then two-pass loudnorm (pass 1 measures, pass 2 applies one linear gain) to land exactly.
  const tpAim = TP - AAC_HEADROOM;
  const i0 = r128(pre);
  const fitted = join(OUT, `.${base}-fit.wav`);
  let gain = LUFS - i0.I, p1;
  for (let k = 0; k < 4; k++) { // the limiter costs a little loudness; make it up here, so loudnorm only has to trim
    ff('-v', 'error', '-i', pre, '-af', `volume=${gain.toFixed(2)}dB,alimiter=limit=${(10 ** ((tpAim - 0.8) / 20)).toFixed(4)}:attack=5:release=60:level=false`, '-c:a', 'pcm_f32le', fitted);
    p1 = loudnormJson(ff('-i', fitted, '-af', `loudnorm=I=${LUFS}:TP=${tpAim}:LRA=11:print_format=json`, '-f', 'null', '-'));
    const short = LUFS - p1.input_i;
    if (short < 0.4) break;
    gain += short;
  }
  const ln = `loudnorm=I=${LUFS}:TP=${tpAim}:LRA=11:linear=true:measured_I=${p1.input_i}:measured_TP=${p1.input_tp}:measured_LRA=${p1.input_lra}:measured_thresh=${p1.input_thresh}:offset=${p1.target_offset}:print_format=json`;
  const tail = `aresample=${SR},atrim=end_sample=${Math.round(D * SR)}`, enc = ['-t', String(D), '-ar', String(SR), '-c:a', 'pcm_s24le', wav];
  const p2 = loudnormJson(ff('-i', fitted, '-af', `${ln},${tail}`, ...enc));
  // loudnorm's two passes do not meter a short clip identically (its analysis pass reads a 5 s file
  // up to 0.5 LU low), so check the result on the EBU R128 meter and land it with one plain gain.
  const chk = r128(wav), nudge = Math.min(LUFS - chk.I, tpAim - chk.TP);
  if (Math.abs(nudge) >= 0.05) ff('-v', 'error', '-i', fitted, '-af', `${ln},volume=${nudge.toFixed(2)}dB,${tail}`, ...enc);
  if (!process.env.BLEH_KEEP_PRE) { rmSync(pre, { force: true }); rmSync(fitted, { force: true }); }
  else console.log('  debug', JSON.stringify({ gain, nudge, p1, p2 }));
  console.log(`  pre-master ${i0.I.toFixed(1)} LUFS / ${i0.TP.toFixed(1)} dBTP, limited ${Math.max(0, i0.TP + gain - (tpAim - 0.8)).toFixed(1)} dB on the loudest peak, loudnorm pass 2: ${p2.normalization_type}, meter trim ${db(Math.abs(nudge) >= 0.05 ? nudge : 0)} dB`);
  if (p2.normalization_type !== 'linear') console.log('  note: loudnorm used its dynamic mode; check the mix for pumping');

  for (const f of [stem, wav]) { // both must be exactly D seconds, to the sample
    const n = Math.round(+probe(f, 'stream=duration', 'a:0').streams[0].duration * SR);
    if (n !== Math.round(D * SR)) die(`${rel(f)} is ${n} samples, expected ${Math.round(D * SR)}. Run again; if it repeats, report it.`);
  }

  // 6. mux: video copied bit for bit, AAC-LC 192 kb/s 48 kHz stereo. If the encode lands over the
  //    true-peak ceiling, pull the whole mix down by the excess and encode again.
  let trim = 0, r;
  for (let pass = 0; pass < 3; pass++) {
    ff('-v', 'error', '-i', video, '-i', wav, '-map', '0:v:0', '-map', '1:a:0', '-c:v', 'copy',
      ...(trim ? ['-af', `volume=${trim.toFixed(2)}dB`] : []), '-c:a', 'aac', '-b:a', '192k', '-ar', String(SR), '-ac', '2', '-t', String(D), '-movflags', '+faststart', mp4);
    r = r128(mp4);
    if (r.TP <= TP) break;
    trim -= r.TP - TP + 0.1;
  }

  // 7. report
  const w = r128(wav), size = (statSync(mp4).size / 1e6).toFixed(2);
  console.log(`  ${rel(wav)}: ${w.I.toFixed(1)} LUFS, ${w.TP.toFixed(1)} dBTP`);
  console.log(`  ${rel(mp4)}: ${r.I.toFixed(1)} LUFS integrated, ${r.TP.toFixed(1)} dBTP true peak, LRA ${r.LRA.toFixed(1)} LU, ${size} MB${trim ? ` (trimmed ${trim.toFixed(1)} dB to hold the true-peak ceiling)` : ''}`);
  if (Math.abs(r.I - LUFS) > 0.7) console.log(`  note: integrated loudness is ${db(r.I - LUFS)} LU from the ${LUFS} LUFS target`);
  if (r.TP > TP) console.log(`  WARNING: true peak ${r.TP} dBTP is over ${TP}`);
  return { format, mp4, ...r };
}

for (const f of only) { if (!['wide', 'tall', 'sting'].includes(f)) die(`--only takes wide, tall, sting (got ${f})`); mix(f); }
