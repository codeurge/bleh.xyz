#!/usr/bin/env node
// Synthesises the PLACEHOLDER music beds with ffmpeg alone (aevalsrc, no samples, no downloads):
//
//   music/placeholder-bed.wav     30.0 s, for the two films
//   music/placeholder-sting.wav    5.0 s, for the sting
//
//   node brand/video/audio/music/make-placeholder.mjs
//
// These exist so the SFX timing can be heard before the real theme arrives from Suno. They are
// deliberately plain: sine bass, noise hats, a two-note falling "bleh" motif (G4 to E4). Replace
// them by dropping music/theme.wav (and optionally music/sting.wav) in this folder.
//
// Grid: 106 BPM, E minor, two-beat pickup. Bar 1's downbeat is the first tongue pop at 1.25 s, so
// V below is "beats since 1.25 s" and bar k covers V in [4(k-1), 4k):
//
//   bar  1   1.25 s  tongue pop            bar  9  19.36 s  (scene 5 enters on beat 3, 20.50)
//   bar  2   3.51 s  the problem           bar 10  21.63 s
//   bar  4   8.04 s  beat 3 = 9.17 wipe    bar 12  26.16 s  wipe; beat 3 = 27.29 end-card pop
//   bar  5  10.31 s  the bleh drop         bar 13  28.42 s  final hit, rings out, silent by 30.0
//   bar  6  12.57 s  what you get
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const SR = 48000, BPM = 106, B = 60 / BPM, T0 = 1.25;
const V = `((t-${T0})/${B})`;
const g = (a, b) => `(gte(${V},${a})*lt(${V},${b}))`;          // gate on beats
const since = (v0) => `max((${V}-${v0})*${B},0)`;               // seconds since beat v0
const every = (n) => `(mod(max(${V},0),${n})*${B})`;            // seconds since the last n-beat tick
const N = '(random(0)*2-1)';
const P2 = '2*PI';

const kick = (x) => `(exp(-${x}/0.11)*sin(${P2}*(48*${x}+70*0.018*(1-exp(-${x}/0.018)))))`;
const bassTone = (f, x, k = 7) => `((sin(${P2}*${f}*${x})+0.45*sin(2*${P2}*${f}*${x})+0.2*sin(3*${P2}*${f}*${x}))*exp(-${x}*${k})*(1-exp(-${x}*500)))`;
const lead = (f, x, dec) => `((sin(${P2}*${f}*${x})+0.33*sin(3*${P2}*${f}*${x})+0.2*sin(5*${P2}*${f}*${x}))*exp(-${x}/${dec})*(1-exp(-${x}*400)))`;
// the "bleh" motif: G4 for one beat, E4 for two
const bleh = (v0, lvl) => `${lvl}*(${g(v0, v0 + 1)}*${lead(392, since(v0), 0.22)}+${g(v0 + 1, v0 + 3)}*${lead(329.63, since(v0 + 1), 0.4)})`;
// tape-stop: a bass note whose pitch dies, f = 82.41 e^(-3x)
const tapeStop = (v0) => `${g(v0, v0 + 1.3)}*(sin(${P2}*82.41*(1-exp(-3*${since(v0)}))/3)+0.45*sin(2*${P2}*82.41*(1-exp(-3*${since(v0)}))/3))*max(0,1-${since(v0)}/0.7)`;
const stab = (x) => `(exp(-${x}/0.45)*(1-exp(-${x}*800))*(0.5*sin(${P2}*164.81*${x})+0.4*sin(${P2}*246.94*${x})+0.4*sin(${P2}*329.63*${x})+0.2*sin(${P2}*493.88*${x})))`;

// bass line, one pitch per beat over a two-bar loop: E E G A | E E D B
const I = `floor(mod(max(${V},0),8))`;
const F = `(82.41+15.59*eq(${I},2)+27.59*eq(${I},3)-8.99*eq(${I},6)+41.06*eq(${I},7))`;

function layers(kind) {
  if (kind === 'sting') {
    const vs = (3.3 - T0) / B, end = 4.9;   // final stab on the second lick at 3.3 s
    return {
      dur: 5,
      drone: `0.5*min(1,t/1.0)*max(0,1-max(t-1.25,0)/1.5)*(0.5*sin(${P2}*82.41*t)+0.3*sin(${P2}*123.47*t)+0.2*sin(${P2}*164.81*t))`,
      drums: `${g(0, 2)}*${kick(since(0))}+${g(vs, vs + 2)}*0.8*${kick(since(vs))}`,
      hats: `0.6*${g(0, 3.4)}*${N}*exp(-${every(0.5)}/0.02)*(0.5+0.5*gte(mod(max(${V},0),1),0.5))`,
      bass: `${g(0, 2)}*${bassTone(82.41, since(0), 3)}`,
      lead: `${bleh(1, 0.9)}`,
      end: `gte(${V},${vs})*${stab(since(vs))}*clip((${end}-t)/0.5,0,1)`,
    };
  }
  const end = 29.9;
  return {
    dur: 30,
    // a low E2 + B2 drone under the sting and the problem scene, gone by the first wipe
    drone: `min(1,t/1.0)*(0.5+0.5*lt(t,3.5))*lt(${V},14)*(1+0.15*sin(${P2}*0.7*t))*0.5*(0.5*sin(${P2}*82.41*t)+0.3*sin(${P2}*123.47*t)+0.2*sin(${P2}*164.81*t))`,
    // kick: bar 1 hit; beats 1+3 through the problem; drop hit; four on the floor in bars 6-11; final hit
    drums: `${g(0, 2)}*${kick(since(0))}+${g(4, 14)}*0.7*${kick(every(2))}+${g(16, 18)}*${kick(since(16))}+${g(20, 44)}*${kick(every(1))}+${g(48, 53)}*${kick(since(48))}`,
    // snare-ish noise on beats 2 and 4 of the groove
    snare: `${g(20, 44)}*gte(mod(${V},2),1)*${N}*exp(-(mod(max(${V}-1,0),2)*${B})/0.05)`,
    // hats: eighths, offbeats accented; half level in the problem scene
    hats: `(0.5*${g(4, 14)}+${g(20, 44)})*${N}*exp(-${every(0.5)}/0.02)*(0.5+0.5*gte(mod(max(${V},0),1),0.5))`,
    // bass: eighth-note pulse, plus held E on the bar 1 and bar 5 hits, plus tape-stops into both wipes
    bass: `(0.7*${g(4, 14)}+${g(20, 44)})*${bassTone(F, every(0.5))}+${g(0, 3)}*${bassTone(82.41, since(0), 2.5)}+${g(16, 19)}*${bassTone(82.41, since(16), 2.5)}+${tapeStop(14)}+${tapeStop(44)}`,
    // lead: the motif alone on the drop, echoed quietly on beat 3 of bars 7, 9 and 11, then on the end-card pop
    lead: `${bleh(16, 1)}+${bleh(26, 0.4)}+${bleh(34, 0.4)}+${bleh(42, 0.4)}+${bleh(46, 0.9)}`,
    // arp in bars 10-11 ("mirrored four ways"): E5 G5 B5 D6 in eighths
    arp: `0.22*${g(36, 44)}*exp(-${every(0.5)}/0.08)*sin(${P2}*(659.26+124.73*eq(floor(mod(${V}*2,4)),1)+328.51*eq(floor(mod(${V}*2,4)),2)+515.4*eq(floor(mod(${V}*2,4)),3))*${every(0.5)})`,
    // button ending on bar 13, forced to silence before 30.0
    end: `gte(${V},48)*${stab(since(48))}*clip((${end}-t)/0.6,0,1)`,
  };
}

const POST = { drone: 'lowpass=f=400', drums: 'lowpass=f=5000', snare: 'bandpass=f=1900:t=q:w=0.8', hats: 'highpass=f=7000', bass: 'lowpass=f=1400', lead: 'lowpass=f=3800,aecho=0.9:0.6:140:0.22', arp: 'aecho=0.9:0.6:212:0.3', end: 'aecho=0.9:0.5:95:0.2' };
const LEVEL = { drone: 0.5, drums: 0.6, snare: 0.25, hats: 0.14, bass: 0.5, lead: 0.42, arp: 1, end: 1.3 };

function ff(args) { const r = spawnSync('ffmpeg', args, { encoding: 'utf8', maxBuffer: 1 << 26 }); if (r.status) { console.error(r.stderr); process.exit(1); } return r.stderr; }

function build(kind, name) {
  const L = layers(kind), dur = L.dur; delete L.dur;
  const keys = Object.keys(L);
  const graph = keys.map((k) => `aevalsrc='${L[k]}':s=${SR}:d=${dur},${POST[k]},volume=${LEVEL[k]}[${k}]`).join(';')
    + `;${keys.map((k) => `[${k}]`).join('')}amix=inputs=${keys.length}:normalize=0,highpass=f=35,lowpass=f=12000[o]`;
  const tmp = join(here, `.${name}.tmp.wav`);
  ff(['-y', '-v', 'error', '-filter_complex', graph, '-map', '[o]', '-t', String(dur), '-c:a', 'pcm_f32le', tmp]);
  const max = +/max_volume: (-?[\d.]+) dB/.exec(ff(['-v', 'info', '-i', tmp, '-af', 'volumedetect', '-f', 'null', '-']))[1];
  ff(['-y', '-v', 'error', '-i', tmp, '-af', `volume=${(-3 - max).toFixed(2)}dB,afade=t=out:st=${dur - 0.05}:d=0.05`, '-ac', '2', '-ar', String(SR), '-c:a', 'pcm_s16le',
    '-fflags', '+bitexact', '-flags:a', '+bitexact', '-map_metadata', '-1', join(here, name)]);
  spawnSync('rm', ['-f', tmp]);
  console.log(`${name}: ${dur} s, ${BPM} BPM, peak -3 dBFS`);
}
build('film', 'placeholder-bed.wav');
build('sting', 'placeholder-sting.wav');
