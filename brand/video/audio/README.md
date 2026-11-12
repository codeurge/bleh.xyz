# bleh.xyz brand film: sound

Sound for the three videos in `brand/video/`. The films were made silent and still work
silent; this adds a music bed and a small set of sound effects that land on the
animation's own beats.

**Status: the music is a placeholder.** `music/placeholder-bed.wav` is a plain
synthesised bed so the effect timing can be heard. The real theme comes from Suno
(prompts in [suno-prompts.md](suno-prompts.md)). The sound effects are final-quality
candidates, all synthesised here with ffmpeg; nothing was downloaded or sampled.

## Workflow

```
Suno → download WAV → save as audio/music/theme.wav → node brand/video/audio/mix.mjs
```

1. Generate the theme with one of the prompts in [suno-prompts.md](suno-prompts.md).
2. Save it as `brand/video/audio/music/theme.wav` (optionally `music/sting.wav`).
3. Run `node brand/video/audio/mix.mjs` from the repository root. If the theme was not
   cropped to the film in Suno, pass `--music-offset` (worked example in the prompts file).
4. Watch the three `-sound.mp4` files next to the silent masters.

Requires Node 22 or newer and ffmpeg. No npm packages, no network.

## What is here

| Path | What |
|---|---|
| `mix.mjs` | The mixer. `node brand/video/audio/mix.mjs --help` lists every option |
| `cues-wide.json`, `cues-tall.json`, `cues-sting.json` | Cue sheets: when each effect plays, how loud, where in the stereo field |
| `make-cues.mjs` | Regenerates the cue sheets from the animation's timings |
| `sfx/*.wav` | 16 sound effects, 48 kHz 16-bit mono |
| `sfx/make-sfx.sh` | Regenerates every effect; each recipe is documented in the script |
| `music/placeholder-bed.wav`, `music/placeholder-sting.wav` | Placeholder music (30 s and 5 s), 106 BPM, E minor |
| `music/make-placeholder.mjs` | Regenerates the placeholders |
| `music/theme.wav` or `music/theme.flac`, `music/sting.wav` | **You add these.** The mixer picks them up automatically; the current theme is "Deadpan Fuzz" (Suno, exactly 30 s, drop at about 10.2 s), stored as FLAC; "Falling Figure" is kept as an alternate |
| `suno-prompts.md` | Prompts and handoff steps for the real theme |
| `out/*-mix.wav`, `out/*-sfx.wav` | Written by the mixer: the mastered stereo mix and the effects-only stem, 48 kHz 24-bit. Ignored by git |

Outputs, next to the silent masters in `brand/video/`:
`bleh-30s-1920x1080-sound.mp4`, `bleh-30s-1080x1920-sound.mp4`,
`bleh-sting-1080x1080-sound.mp4`. The video stream is copied bit for bit; only an AAC
track (192 kb/s, 48 kHz, stereo) is added. The silent masters are never touched.

## The effects

| File | Used for |
|---|---|
| `tongue-pop` | The signature. Wet pop, droplet, and a boing whose pitch follows the same damped spring as the tongue. Three times in the film (1.25, 10.35, 27.2), once in the sting |
| `lick` | The sting's small second lick at 3.3 s |
| `draw` | The wordmark drawing on |
| `blip` | Small arrivals: the pink dot, list lines, diagram boxes |
| `tick` | Tape rows and trade prints |
| `data-chirp` | A new order ringed in the book |
| `deflate` | The grey bars wiping over the prices |
| `stamp` | The four stamps |
| `whoosh`, `whoosh-down`, `slurp` | Scene slides; the tongue wipe dropping; the wipe slurping back |
| `signed` | The pink tick landing on the signed block |
| `sparkle` | The four mirrors popping in |
| `powerdown` | The CDN mirror being crossed out |
| `shrug` | "Nothing happens." |
| `button` | The yellow URL pill on the end card |

To change a sound, edit its recipe in `sfx/make-sfx.sh` and run it. `CHECK=1
sfx/make-sfx.sh` also writes a waveform and spectrogram PNG per effect to `sfx/check/`.

## Adjusting cues

A cue is one line:

```json
{ "time": 10.35, "sfx": "tongue-pop", "gain": -3, "pan": 0, "note": "THE drop. Poster tongue pops: pop(seg(t, 10.35, 11.25))" }
```

- `time`: seconds from the first frame to the start of the sound.
- `sfx`: a file in `sfx/`, without `.wav`.
- `gain`: dB applied to the file. Files are peak-normalised, so the same number means
  roughly the same loudness for every effect. Signature moments sit around −3 to −5,
  supporting sounds −7 to −15, texture (ticks, chirps) −18 to −28.
- `pan`: −1 left, 0 centre, +1 right. The wide film pans toward where things are on
  screen; the vertical film keeps nearly everything centred, because phones are
  close to mono.
- `note`: for people. It quotes the expression in `source/index.html` the time came from.

For a one-off nudge, edit the JSON and run the mixer. If the animation's timing changes,
edit the tables in `make-cues.mjs` and run `node brand/video/audio/make-cues.mjs`; that
overwrites all three sheets, so move any hand edits into the script first. Scene 4's
ticks are not hand-placed: the script ports the film's seeded book simulation and puts a
tick on each frame where a trade prints.

Mix-wide knobs without touching cues: `--sfx-level -3` (all effects down 3 dB),
`--music-level 2` (music up 2 dB), `--no-duck`, `--only sting`.

## Loudness

| | Target | Why |
|---|---|---|
| Integrated loudness | **−14 LUFS** | The level the large platforms normalise to or near. Louder files get turned down; quieter ones often do not get turned up |
| True peak | **under −1 dBTP** | Headroom for each platform's own re-encode |
| Audio | AAC-LC, 192 kb/s, 48 kHz, stereo | |

How the mixer gets there: the music is first level-matched to −17 LUFS so cue gains mean
the same under any track; a sidechain compressor keyed by the effects ducks the music
7 to 11 dB under pops and stamps and leaves it alone under ticks; a light master EQ; a
limiter catches the few peaks; two-pass `loudnorm` sets the level; the result is checked
on the EBU R128 meter and trimmed; and the finished MP4 is measured again after AAC
encoding. The mixer prints those final numbers. To measure a file yourself:

```sh
ffmpeg -hide_banner -nostats -i brand/video/bleh-30s-1920x1080-sound.mp4 -af ebur128=peak=true -f null - 2>&1 | tail -12
```

## Per-platform notes

Every feed below starts video **muted**. The first second has to read with the sound
off, and it does: the wordmark is drawing on from 0.2 s and the tongue pops at 1.25 s.
Sound is a reward for people who turn it on, never the carrier of meaning. All the words
are on screen.

| Platform | File | Notes |
|---|---|---|
| X | wide for the timeline, sting for replies and quote posts | Autoplays muted; shows no sound-on prompt worth relying on. Upload the `-sound` file anyway. X re-encodes audio at a low bitrate, which is why the effects avoid depending on deep bass or very high sparkle |
| LinkedIn | wide (or tall, which takes more of a phone feed) | Autoplays muted, and much of the audience is at a desk with sound off. Consider adding the first line of copy to the post text. Avoid posting the sound version where a sudden pop would be unwelcome: the film is quiet for 1.25 s and then pops |
| YouTube Shorts | tall | Sound is usually **on** here. This is where the mix matters most. YouTube normalises to about −14 LUFS, so the file plays as mastered |
| TikTok | tall | Sound on by default in the For You feed, muted in some embeds. A track you upload is treated as an "original sound"; do not add a library track on top of it in the app |
| Instagram Reels | tall | Same as TikTok for practical purposes. Square sting works for the grid |
| Site hero, README, docs | `bleh-sting-loop.webm` / `.gif` (silent) | Keep these silent. Browsers block autoplay with sound, and a README should not pop at anyone |

Phone speakers reproduce almost nothing under about 200 Hz. The pop, the stamps and the
button all have their identifying energy well above that; the low thud is a bonus on
headphones. Check the mix once on a phone speaker and once on headphones before posting.

## Regenerating everything

```sh
brand/video/audio/sfx/make-sfx.sh                     # effects
node brand/video/audio/music/make-placeholder.mjs     # placeholder music
node brand/video/audio/make-cues.mjs                  # cue sheets
node brand/video/audio/mix.mjs                        # mixes and the three -sound.mp4 files
```

The effects and the placeholder music are deterministic (same ffmpeg build, same bytes).
The mix is repeatable to the meter reading but not always to the byte.

## Not done

- Nobody has listened to any of this. It was built and checked by measurement and by
  looking at waveforms and spectrograms. Timing, levels and sync are verified; whether
  the pop sounds wet and funny rather than odd is a judgement for ears.
- The real theme does not exist yet. Balance against a dense, mastered Suno track will
  differ from the sparse placeholder; expect to use `--music-level` or `--sfx-level`.
- The storyboard still describes the films as silent.
