# bleh.xyz brand concepts: round 1

Status: concepts for review, not final. Open `index.html` for the gallery.

All wordmarks are drawn geometry (strokes on a 10/40/80 grid, or pixel
rects), not fonts, so the SVGs render the same everywhere with no font
dependency. Ink parts use `currentColor` with a default set on the root
element, so the same file can be inlined and recoloured by CSS.

Each direction has three files: `logo-<n>-<slug>.svg` (lockup),
`-mark.svg` (64 px square, tested at 16 px) and `-mono.svg` (one colour).

## Directions

### 1. Shrugging candlestick (`logo-1-shrug-candle`)

A candle body with a wick, two shrugging arms and a smirk.

- Palette: ink `#16141c`, acid lime `#c8ff2e`.
- Wordmark: round monoline, weight 10.
- Suggested web type: Space Grotesk with JetBrains Mono.
- For: says "market data" and "shrug" at once; the most literal reading of
  the name.
- Against: the weakest at 16 px. The arms and face blur and it reads as a
  generic trading glyph. Candlesticks are also the most common motif in the
  category.

### 2. Tongue-out e (`logo-2-tongue-e`), recommended

The wordmark is the logo. The lower right of the e is opened up and a pink
tongue hangs out of it. The e alone is the favicon.

- Palette: ink `#16141c`, cream `#fff6e9`, tongue pink `#ff4f8b`
  (groove `#c9245f`). For pink text on light backgrounds use `#e0336f`.
- Wordmark: round monoline, weight 12; `.xyz` at 55% with a pink dot.
- Web type used in mock C: DM Sans with DM Mono.
- For: one idea, one accent colour, no separate symbol to learn. Clearest
  favicon of the five. Carries the joke without any finance imagery, so it
  will not date and does not look like anyone else in the category.
- Against: says nothing about markets by itself; the surrounding page has to.
  In one colour the tongue becomes an outline, which is slightly less
  immediate.

### 3. Order-book :P (`logo-3-book-face`)

A tongue-out face stacked from horizontal order-book levels: bid-green eyes
and mouth, ask-red tongue. Pixel wordmark on a 5 by 7 grid.

- Palette: terminal black `#0a0f0c`, bid green `#2dff8f`, ask red `#ff5a3d`.
  On light backgrounds use green `#087f45` and red `#c4381c` for text.
- Web type used in mock A: JetBrains Mono.
- For: strongest product tie (order-level books are the headline feature);
  crisp at 16 px because it is already pixels.
- Against: retro-terminal styling narrows the audience, and the pixel
  wordmark is hard to read at small sizes.

### 4. Meh line (`logo-4-meh-line`)

Two dots and a price line that wobbles then goes flat, making a deadpan
face.

- Palette: violet `#4a3aff`, white, ink `#16141c`.
- Wordmark: thin round monoline, weight 7.5; `.xyz` at 50%.
- Suggested web type: Inter or Geist with a matching mono.
- For: tidy, friendly, safe for a developer tool.
- Against: the least cheeky. "Flat price line" could also read as "feed is
  down", which is the wrong joke for a data service.

### 5. Torn ticker tape (`logo-5-torn-tape`)

The name printed on a strip of tape torn off at both ends, set at a slight
angle. The mark is a torn tab with a bold b.

- Palette: tape yellow `#ffd60a`, ink `#111111`, red `#e5261a` for accents.
- Wordmark: heavy round monoline, weight 13.
- Web type used in mock B: Anton for display, Space Mono for text.
- For: the most attitude, great on stickers, posters and social cards, and
  works on any background because it brings its own.
- Against: ticker tape is a legacy-finance symbol, which fights the "not
  them" story. The mark is a plain letter on a tile and less ownable.

## Landing mocks

| File | Style | Logo |
|---|---|---|
| `landing-a-terminal.html` | Terminal: monospace, green on black, animated order-level book | 3 |
| `landing-b-poster.html` | Punk poster: huge condensed type, tape yellow, hard shadows, running tape | 5 |
| `landing-c-devtool.html` | Clean dev tool: warm cream, one pink accent, candle chart, code tabs | 2 |

All three follow `prefers-color-scheme`, were checked at 375 px with no
horizontal scroll, and honour `prefers-reduced-motion`. The only external
request is the Google Fonts stylesheet. Charts, books and tapes are random
or fixed made-up values and are labelled as illustrative. The code snippets
are sketches: the gateway protocol is not designed yet.

## Recommendation

Go with direction 2, the tongue-out e.

- It is the name. "bleh" is a tongue-out noise, and the logo is the word
  doing exactly that. Nothing needs explaining.
- It scales best. The e mark is the most legible of the five at 16 px, and
  the lockup needs no separate symbol.
- It is the most ownable. No candles, charts or tape, so it cannot be
  confused with any other market-data brand.
- It leaves room for the serious side. One small joke in an otherwise calm
  wordmark lets the docs and data pages stay sober.

Suggested system: logo 2 as the primary, with the poster voice from mock B
(Anton headlines, tape yellow as a second accent) kept for launch posts,
stickers and social cards. The order-book face from direction 3 would make
a good status or loading glyph inside the product.

## Not yet done

- Checked in Chromium and librsvg only; not in Safari or Firefox.
- No colour-contrast audit beyond choosing darker text variants for light
  backgrounds.
- No trademark or similarity search on any mark.
- Mono lockups for 1, 3 and 4 use the bare mark without its tile, so the
  mark looks smaller than in the colour lockup.
