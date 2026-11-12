# bleh.xyz brand guidelines

The brand is one small joke in a calm wordmark, surrounded by work that is
precise. The tongue is the only thing allowed to misbehave. Everything else
(type, layout, numbers, claims) is exact, so the data and docs stay serious.

Think top-tier developer tool with a punk streak: cheeky about legacy
market-data giants in general, never about anyone in particular.

Contents: [Logo](#1-logo) · [Colour](#2-colour) · [Type](#3-type) ·
[Icons and illustration](#4-icons-and-illustration) · [Motion](#5-motion) ·
[Voice](#6-voice-and-tone) · [Asset index](#7-asset-index)

## 1. Logo

The logo is the name: a lowercase **bleh** whose **e** sticks its tongue out,
followed by a pink dot and a quieter `.xyz`. It is drawn geometry (round
12-unit strokes on a 10/40/80 grid), not a font.

| File | Use |
|---|---|
| [logo.svg](logo.svg) | Wordmark, ink. Cream, yellow and other light backgrounds |
| [logo-light.svg](logo-light.svg) | Wordmark, cream. Ink and other dark backgrounds |
| [logo-mono.svg](logo-mono.svg) | One colour, tongue as an outline. Engraving, single-ink print, fax-grade contexts |
| [mark.svg](mark.svg) | Square mark, the e alone. Favicons, avatars, app icons |

Two derived lockups are used in the site, film and cards. They reuse the same
paths and are not separate logos:

- **Short wordmark, `bleh.`**: the wordmark cropped after the pink dot
  (viewBox `0 0 215 100`). For posters, the film's answer scene and anywhere
  the URL is given separately.
- **Open glyph**: the e from the mark with no rounded square, placed directly
  on an ink field. For avatars that get circle-cropped and for large
  decorative use.

### Clear space and size

- **Clear space:** one stroke-and-a-half on every side, measured as the
  height of the letter l's lower half, or simply 20% of the wordmark's
  height. Nothing else enters that space. The tongue hangs below the
  baseline; measure from the bottom of the tongue, not the baseline.
- **Minimum size, wordmark:** 96 px wide on screen, 25 mm in print. Below
  that the tongue's centre line fills in; switch to the mark.
- **Minimum size, mark:** 16 px. It was drawn to survive there.
- **Minimum size, mono wordmark:** 120 px wide, since the outlined tongue is
  finer.

### Backgrounds

| Background | Use |
|---|---|
| Cream, white, tape yellow | `logo.svg` (ink) |
| Ink, photos darkened to at least 70% | `logo-light.svg` (cream) |
| Tongue pink | Ink wordmark with the tongue recoloured cream and the groove ink, as on card 4. The only permitted recolour |

### Don't

- Don't redraw, re-space, outline or set the name in a font. Use the files.
- Don't recolour the tongue. It is tongue pink (or cream on a pink field).
- Don't remove the tongue, add a second one, or add eyes, faces or drips.
- Don't rotate, skew, stretch, or add shadows, glows or gradients.
- Don't put the ink wordmark on mid-tone or busy backgrounds.
- Don't animate the letters once drawn. Only the tongue moves.
- Don't place it next to another organisation's logo in a way that implies
  partnership or endorsement, and never combine it with a third-party mark.
- Don't let the tongue touch or point at another company's name or logo.
  The joke is aimed at a category, never at a target.

## 2. Colour

The adopted five colours are unchanged. This system adds one secondary
accent (tape yellow, already named in the brand README for posters) and a
set of neutrals.

### Core

| Name | Hex | Role |
|---|---|---|
| Ink | `#16141c` | Text, wordmark on light, dark backgrounds |
| Cream | `#fff6e9` | Light backgrounds, wordmark and text on dark |
| Tongue pink | `#ff4f8b` | The tongue, the dot, primary buttons, one accent per view |
| Groove | `#c9245f` | Tongue centre line; small pink text and links on light backgrounds |
| Pink on light | `#e0336f` | Large pink display text on light backgrounds (24 px and up) |

### Secondary accent

| Name | Hex | Role |
|---|---|---|
| Tape yellow | `#ffd23f` | Poster fields, the "illustrative" and status badges, highlights in code. Always with ink text |
| Tape on light | `#7a5600` | Yellow's text-safe partner, for bid numbers on light backgrounds only |

### Neutrals

| Name | Hex | Role |
|---|---|---|
| Night | `#0f0d14` | Code blocks and data panels, in both themes |
| Ink 2 | `#1e1b26` | Cards on dark |
| Ink 3 | `#322d3d` | Borders and dividers on dark |
| Fog | `#aba2b4` | Secondary text on dark |
| Stone | `#5d5566` | Secondary text on light |
| Paper | `#fffcf6` | Cards on light |
| Sand | `#f8ead6` | Alternating section background on light |
| Dune | `#eadcc6` | Borders and dividers on light |

### Data colours

We do not use red and green. **Asks and sells are tongue pink; bids and buys
are tape yellow.** It is ours, it survives the common forms of colour
blindness better than red and green, and it avoids implying good and bad.
Always pair the colour with a second signal: an arrow, a side label or
position in the ladder.

### Contrast (WCAG 2.1 ratios)

| Pair | Ratio | Verdict |
|---|---|---|
| Ink on cream | 17.0 | AAA |
| Stone on cream | 6.6 | AA body text |
| Groove on cream | 5.0 | AA body text |
| Pink on light on cream | 4.0 | Large text and UI only |
| Tongue pink on cream | under 3 | Never for text |
| Tape on light on cream | 6.2 | AA body text |
| Cream on ink | 17.0 | AAA |
| Fog on ink | 7.4 | AAA |
| Tongue pink on ink | 5.9 | AA body text |
| Tape yellow on ink | 12.6 | AAA |
| Ink on tongue pink | 5.9 | AA; use ink, not cream or white, on pink |
| Ink on tape yellow | 12.6 | AAA |

### Rules

- One accent per view. If pink is doing a job, yellow is at most a badge.
- Full-bleed pink and full-bleed yellow are for posters, cards, the film and
  one band per page. They are not page backgrounds.
- Light theme: cream background, ink text. Dark theme: ink background, cream
  text. Pure white and pure black are not brand colours (the sticker sheet's
  white is paper, not a colour).

## 3. Type

**DM Sans** for text and headings, **DM Mono** for code, data, labels and
anything a machine might have printed. Both are open-source and served by
Google Fonts.

| Style | Font | Size (fluid) | Weight | Tracking | Line height |
|---|---|---|---|---|---|
| Display / H1 | DM Sans | 43–85 px | 800 | −0.045em | 0.98 |
| H2 | DM Sans | 32–54 px | 800 | −0.035em | 1.05 |
| H3 | DM Sans | 18.5 px | 700 | −0.015em | 1.25 |
| Lede | DM Sans | 17–20 px | 400 | 0 | 1.6 |
| Body | DM Sans | 17 px | 400 | 0 | 1.6 |
| Small | DM Sans | 13.5–15.5 px | 400 | 0 | 1.5 |
| Eyebrow / label | DM Mono | 12.5 px, uppercase | 500 | +0.12em | 1 |
| Code | DM Mono | 13.5 px | 400 | 0 | 1.7 |
| Data | DM Mono | 12.5 px | 400 | 0 | 1.5 |

- Headlines are short, sentence case, and end with a full stop.
- Tight negative tracking is for 800-weight display sizes only. Do not
  tighten body text.
- **Set `CC0` in DM Mono whenever it appears large.** In DM Sans the zero
  reads as the letter O. DM Mono's zero is slashed.
- Numbers in tables and panels are always DM Mono so columns align.
- Poster style from the launch brief (very large type on tape yellow) uses
  DM Sans 800; no third typeface is needed.

## 4. Icons and illustration

### Icons

- 24 px grid, 1.75 px stroke, round caps and joins, `currentColor`.
- Geometric and plain: lines, rectangles, circles. No fills except one.
- **At most one pink element per icon** (a filled dot, bar or tick). That
  pink element is the icon's "tongue": the part that matters.
- Sit icons in a 40 px rounded square (11 px radius) on the surface colour.
- No third-party logos, ever. Link to GitHub, BitTorrent, IPFS and so on by
  name in DM Mono, not by their marks.

### Illustration

Illustrations are diagrams and data, not characters.

- **The order-by-order ladder** is the house illustration: rows of small
  rounded bars, each bar one order, asks pink above a dashed spread line,
  bids yellow below. A newly added order gets a cream ring.
- **The tape:** time, price, size and an aggressor arrow, in DM Mono.
- **Pipeline diagrams:** boxes with a 1.5–3.5 px ink outline and generous
  radius, dashed connectors, pink dots as packets. A signed thing gets a
  pink circle with an ink tick. A removed thing gets a dashed outline, a
  struck-through label and a pink cross.
- **The fine-print form:** a generic paper form with crossed boxes, for the
  problem. It never resembles a real company's document.
- **Any number shown in an illustration is labelled "illustrative"** with
  the yellow badge, and a caption says it is not a live feed.
- The order-book face from concept 3 remains available as an in-product
  status glyph. It is not used in marketing.

## 5. Motion

One signature, everything else quiet.

### The tongue pop

The tongue scales out of the e's crossbar from its top edge, overshoots and
settles. It is the only expressive motion in the system.

| Property | Value |
|---|---|
| Transform origin | Top centre of the tongue (the crossbar) |
| Path | scaleY 0 → 1.22 → 0.92 → 1, with scaleX 1 → 0.90 → 1.06 → 1 (it thins as it stretches) |
| Duration | 520–600 ms on the web; 900 ms in film, where it is a damped spring |
| Easing (web) | Keyframes at 0 / 55 / 75 / 100% on `cubic-bezier(.16, 1, .3, 1)` |
| Spring (film) | `scaleY = 1 − e^(−6.5p) · cos(11p)`, `scaleX = 1 + 0.16 · e^(−6.5p) · sin(11p)`, p from 0 to 1 |
| Delay after the logo appears | 250–350 ms. Let the word land first |

Use it once per view: when the logo first appears, when a section's big
wordmark scrolls into view, or as the answer beat in a film. On hover the
tongue may stretch to 1.25 and return. Never loop it on a page.

**The tongue wipe** is the film transition: a full-width pink tongue drops
over the frame in 450 ms (`ease-out-quint`), holds for 100 ms, and slurps
back up in 470 ms (`ease-in-out-cubic`). Scene content changes while it
covers the frame. Film only.

### Everything else

| Motion | Timing | Easing |
|---|---|---|
| Reveal on scroll | 700 ms, 18 px rise plus fade, 60–160 ms stagger | `cubic-bezier(.16, 1, .3, 1)` |
| Buttons, chevrons | 250 ms | `cubic-bezier(.34, 1.56, .64, 1)` (small overshoot) |
| Data ticks (book, tape) | one change every 420 ms | none; data snaps |
| Stamps in film | 220 ms, scale 2.1 → 1 | `ease-out-quint` |
| Diagram boxes in film | 400 ms, scale 0.6 → 1 | back-out |

Principles:

1. **Data snaps, interface eases, only the tongue bounces.**
2. Motion explains order: things appear in the order the system works.
3. Nothing moves forever except illustrative data, and that stops when it
   is off screen.
4. **Respect `prefers-reduced-motion`:** no reveals, no pop, no ticking
   data. The page must be complete and readable with motion off and with
   JavaScript off.
5. Film animation is a pure function of time so any frame can be rendered
   again exactly.

## 6. Voice and tone

Cheeky about the category, precise about the product, honest about the
status. A shrug, not a rant.

### Principles

1. **Say the true thing plainly.** Short sentences. Specific nouns.
2. **One joke per screen**, and the logo usually already made it.
3. **Punch at practices, never at names.** "Legacy market-data giants",
   "the order form", "the embargo". No real exchange, vendor or product is
   named, drawn or parodied. Hyperliquid may appear only as a factual
   source venue.
4. **Never invent a number.** No statistics, customers, testimonials, user
   logos, latency or uptime figures. If a number is illustrative, label it.
5. **Call a proxy a proxy.** These are prices for the same exposure, not
   exchange prices or settlements. Thin markets are tapes, not benchmarks.
6. **Say what is not built.** While the project is an early preview, every
   page, post and video says so, and planned things are described as
   planned.

### Do and don't

| Do | Don't |
|---|---|
| "Market data without the permission slip." | "The exchange-killer is here." |
| "The legacy data giants can keep their order forms." | Naming or hinting at a specific company, product or price list |
| "Order-level books, no embargo." | "The fastest, most complete feed in the world." |
| "Early preview. Nothing is live yet." | "Join thousands of traders" or any invented count |
| "Prices for the same economic exposure. Not exchange prices." | "Real-time futures prices, free." |
| "Tape: real trades, thin market. A tape, not a benchmark." | Presenting every market as equally deep |
| "You check the maths." | "Trust us" / "bank-grade security" |
| "There is nobody to send the letter to." | Threats, legal bravado, or promises that nothing can ever go wrong |
| "No token." | Anything that reads like a token launch: points, airdrops, "wen" |
| "We would rather tell you now." (about gaps) | Hiding gaps until someone finds them |

### Tone by context

| Context | Cheek | Example |
|---|---|---|
| Launch posts, cards, stickers | High | "Market data? bleh." |
| Site headlines | Medium | "The whole feed. No tiers." |
| Site body, FAQ | Low | "Do not use these as official settlements." |
| Docs, changelogs, errors | None | "Gap from block N to N+12: reader restart." |

### Words

- **Use:** open, signed, mirrored, public domain, tape, proxy, exposure,
  lineage, verify, recompute, early preview, planned.
- **Avoid:** revolutionary, disrupt, democratise, decentralised (say what is
  actually distributed), real-time futures prices, exchange data, free
  forever, guaranteed, unstoppable.
- Write `bleh.xyz` in lower case, including at the start of a sentence where
  it cannot be avoided. Write `CC0` and `Apache-2.0` exactly.

## 7. Asset index

### Logos

| Asset | File |
|---|---|
| Wordmark, ink | [logo.svg](logo.svg) |
| Wordmark, cream | [logo-light.svg](logo-light.svg) |
| Wordmark, one colour | [logo-mono.svg](logo-mono.svg) |
| Mark | [mark.svg](mark.svg) |

### Site

| Asset | File |
|---|---|
| Home page | [site/index.html](site/index.html) |
| Manifesto | [site/manifesto.html](site/manifesto.html) |
| Styles (design tokens at the top) | [site/assets/site.css](site/assets/site.css) |
| Script (theme, tabs, illustrative book) | [site/assets/site.js](site/assets/site.js) |
| Link preview images | [site/assets/og.png](site/assets/og.png), [site/assets/og-manifesto.png](site/assets/og-manifesto.png) |
| Favicons and manifest | [site/assets/](site/assets/) |

### Video

| Asset | File |
|---|---|
| Brand film, 30 s, 1920x1080 | [video/bleh-30s-1920x1080.mp4](video/bleh-30s-1920x1080.mp4) |
| Brand film, 30 s, 1080x1920 | [video/bleh-30s-1080x1920.mp4](video/bleh-30s-1080x1920.mp4) |
| Logo sting, 5 s, 1080x1080 | [video/bleh-sting-1080x1080.mp4](video/bleh-sting-1080x1080.mp4) |
| Sting loop | [video/bleh-sting-loop.gif](video/bleh-sting-loop.gif), [video/bleh-sting-loop.webm](video/bleh-sting-loop.webm) |
| Storyboard and script | [video/storyboard.md](video/storyboard.md) |
| Seekable source | [video/source/index.html](video/source/index.html) |
| Render scripts | [video/render/render.mjs](video/render/render.mjs), [video/render/cdp.mjs](video/render/cdp.mjs) |

### Social

PNG in `social/`, SVG source of the same name in `social/src/`.

| Asset | Size | File |
|---|---|---|
| Open Graph image | 1200x630 | [social/og-1200x630.png](social/og-1200x630.png) |
| Open Graph image, manifesto | 1200x630 | [social/og-manifesto-1200x630.png](social/og-manifesto-1200x630.png) |
| GitHub social preview | 1280x640 | [social/github-social-1280x640.png](social/github-social-1280x640.png) |
| X header | 1500x500 | [social/x-header-1500x500.png](social/x-header-1500x500.png) |
| LinkedIn banner | 1584x396 | [social/linkedin-banner-1584x396.png](social/linkedin-banner-1584x396.png) |
| Avatar | 400x400 | [social/avatar-400x400.png](social/avatar-400x400.png) |
| Favicons | 16, 32, 48, .ico | [social/favicon-16.png](social/favicon-16.png), [social/favicon-32.png](social/favicon-32.png), [social/favicon-48.png](social/favicon-48.png), [social/favicon.ico](social/favicon.ico) |
| Apple touch icon | 180x180 | [social/apple-touch-icon-180.png](social/apple-touch-icon-180.png) |
| Maskable icon | 512x512 | [social/icon-512-maskable.png](social/icon-512-maskable.png) |
| Card: market data? bleh. | 1080x1080 | [social/card-1-market-data-bleh.png](social/card-1-market-data-bleh.png) |
| Card: order books, no embargo | 1080x1080 | [social/card-2-no-embargo.png](social/card-2-no-embargo.png) |
| Card: signed. mirrored. CC0. | 1080x1080 | [social/card-3-signed-mirrored-cc0.png](social/card-3-signed-mirrored-cc0.png) |
| Card: we don't do takedowns | 1080x1080 | [social/card-4-no-takedowns.png](social/card-4-no-takedowns.png) |
| Sticker sheet (art and cut layers) | 1400x900 | [social/src/sticker-sheet.svg](social/src/sticker-sheet.svg), preview [social/sticker-sheet.png](social/sticker-sheet.png) |
| Launch copy | – | [social/copy.md](social/copy.md) |
| Generator | – | [social/build/build.mjs](social/build/build.mjs) |

Notes on the social sources:

- The SVG sources set their text in DM Sans and DM Mono through a Google
  Fonts import. Open them in a browser, or run the generator, to see them as
  intended; an editor without those fonts will substitute. Convert text to
  outlines before sending the sticker sheet to a printer.
- On the sticker sheet, artwork is in the `art` group and die lines are in
  the `cut` group (magenta hairline, about 3 mm outside the art). Keep the
  cut layer as a spot colour named to your printer's convention.
- To rebuild everything: `node brand/social/build/build.mjs`, which also
  refreshes the images in `site/assets/`.

### Concepts

The first design round is kept in [concepts/](concepts/) for reference only.
