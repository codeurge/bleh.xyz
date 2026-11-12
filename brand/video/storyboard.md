# bleh.xyz brand film: storyboard and script

30 seconds, silent, on-screen text only. 30 fps, 900 frames. Two compositions
from one source: 1920x1080 (`format=wide`) and 1080x1920 (`format=tall`). The
vertical cut is laid out again, not cropped: text stacks above the visual and
the pipeline diagram runs top to bottom.

Nothing in the film claims the service is live. Scene 4 is titled "What we're
building", the panel is badged "illustrative", and scenes 4 to 6 carry an
"early preview" line. No real company is named or drawn; the problem scene
uses generic row labels and generic stamps.

| # | Time | Scene | On-screen text | Picture and motion |
|---|---|---|---|---|
| 1 | 0.0–3.5 | Logo sting | `bleh.xyz` · OPEN MARKET DATA | Ink background. The wordmark draws on letter by letter (0.2–1.35 s). At 1.25 s the tongue pops out of the e with a spring overshoot. The pink dot pops at 1.7 s, `.xyz` draws at 1.85 s, tagline fades up at 2.3 s. The lockup lifts and fades from 3.1 s. |
| 2 | 3.5–9.2 | The problem | "Prices are made in public." then "Then somebody **fences them off.**" | A generic tape (INDEX, CRUDE, GOLD, RATES, FX, GRAINS, METALS, ENERGY) ticks with illustrative numbers. At 5.1 s grey bars wipe across each row. From 6.2 s four stamps slam down: DELAYED · PER USER · PER DEVICE · DO NOT REDISTRIBUTE · CALL SALES. |
| – | 9.2–10.2 | Tongue wipe | – | A giant pink tongue drops from the top, covers the frame, and slurps back up to reveal the next scene. |
| 3 | 9.7–12.5 | The bleh answer | OUR CONSIDERED RESPONSE · `bleh.` · "Open market data. No permission slip." | Tape-yellow poster. The big wordmark sits still; its tongue pops at 10.35 s. The whole poster slides up and away at 12.45 s. |
| 4 | 12.5–20.6 | What you get | WHAT WE'RE BUILDING · Trades with aggressor side · Order-level books. No embargo. · Bars and open order-flow analytics · History back to day one · Python · TypeScript · Rust · MCP | An illustrative order-by-order book and trade tape runs (asks pink, bids yellow, new orders ringed, signed block ticker underneath). One line arrives every 1.45 s; the current line is bright, earlier lines dim, all five light up at 20 s. |
| 5 | 20.6–26.2 | How it stays up | "Signed per block." → "Mirrored four ways." → "Anchored. Public domain." → "Take one down." → "Nothing happens." | Cream background slides up. Two nodes feed a signed block (pink tick lands at 22.05 s). Lines fan out to CDN, BitTorrent, IPFS and Arweave; a dashed box notes the Bitcoin timestamp. At 24.95 s the CDN mirror is crossed out and its packets stop. The other three give a small shrug-bounce and keep flowing. |
| – | 26.2–27.2 | Tongue wipe | – | Same wipe as before. |
| 6 | 26.7–30.0 | End card | `bleh.xyz` · "Open market data. No permission slip." · github.com/codeurge/bleh.xyz · Early preview · in development · Signed per block · Code Apache-2.0 · Data CC0 · No token | Ink background, tongue pops once more at 27.2 s, lines rise in. Holds for 1.4 s. |

## Sting and loop

- `format=sting` (1080x1080, 5 s): scene 1 on its own with a second small
  lick at 3.3 s, then a hold on the lockup.
- `format=loop` (1080x1080, 3 s): the finished lockup with the tongue in;
  it pops out, wiggles, and slides back in, so the last frame matches the
  first. Used for the GIF and WebM.

## Files

| File | What |
|---|---|
| `bleh-30s-1920x1080.mp4` | Brand film, landscape. H.264, yuv420p, 30 fps, silent |
| `bleh-30s-1080x1920.mp4` | Brand film, vertical recomposition |
| `bleh-sting-1080x1080.mp4` | 5 second logo sting |
| `bleh-sting-loop.gif` / `.webm` | 3 second seamless loop (480 px GIF, 720 px VP9) |
| `source/index.html` | The animation. Seekable: `window.renderAt(seconds)` |
| `render/render.mjs`, `render/cdp.mjs` | Frame renderer. No npm dependencies |

## Rendering

Requires Node 22 or newer, Chrome or Chromium, ffmpeg, and network access to
Google Fonts for DM Sans and DM Mono.

```sh
node brand/video/render/render.mjs              # all outputs
node brand/video/render/render.mjs wide         # one of: wide tall sting loop
node brand/video/render/render.mjs --still wide 10.6 frame.png
```

The script starts headless Chrome, opens `source/index.html?render=1&format=…`,
calls `renderAt(frame / 30)` for each frame, captures a PNG over the DevTools
protocol and pipes it to ffmpeg (`libx264 -crf 17 -preset slow -pix_fmt yuv420p`).
Every frame is a pure function of time: no CSS animations, timers or
`Math.random`, so renders are repeatable.

To preview in a browser, open `source/index.html?format=wide&play=1`, or add
`&t=12.5` to look at one moment.
