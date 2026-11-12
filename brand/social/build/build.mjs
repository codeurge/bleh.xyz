#!/usr/bin/env node
// Generates the social SVG sources in ../src and rasterizes them to PNG in .. with headless Chrome.
// Also writes the favicon set and copies what the site needs into ../../site/assets.
// No npm dependencies. Usage: node build.mjs
import { writeFileSync, readFileSync, copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { launch } from '../../video/render/cdp.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = join(here, '../src'), OUT = join(here, '..'), SITE = join(here, '../../site/assets'), MARK = join(here, '../../mark.svg');
mkdirSync(SRC, { recursive: true });

const INK = '#16141c', CREAM = '#fff6e9', PINK = '#ff4f8b', GROOVE = '#c9245f', TAPE = '#ffd23f', MUTE = '#aba2b4', STONE = '#5d5566';
const FONT = `<style>@import url('https://fonts.googleapis.com/css2?family=DM+Mono:wght@400;500&amp;family=DM+Sans:opsz,wght@9..40,500;9..40,700;9..40,800&amp;display=swap');.s{font-family:'DM Sans',system-ui,sans-serif;font-weight:800;letter-spacing:-.045em}.b{font-family:'DM Sans',system-ui,sans-serif;font-weight:500;letter-spacing:-.01em}.m{font-family:'DM Mono',ui-monospace,monospace;font-weight:500}</style>`;

const svg = (w, h, body, title) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" role="img" aria-label="${title}">\n<title>${title}</title>\n${FONT}\n${body}\n</svg>\n`;
const rect = (x, y, w, h, fill, rx = 0, extra = '') => `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${rx}" fill="${fill}" ${extra}/>`;
const t = (x, y, size, str, fill, cls = 's', extra = '') => `<text x="${x}" y="${y}" font-size="${size}" fill="${fill}" class="${cls}" ${extra}>${str}</text>`;

// Wordmark geometry, identical to brand/logo.svg. w is the drawn width in px.
function word(x, y, w, ink, { xyz = true } = {}) {
  const k = w / (xyz ? 377.9 : 215);
  return `<g transform="translate(${x} ${y}) scale(${k}) translate(8 0)" style="color:${ink}"><g fill="none" stroke="currentColor" stroke-width="12" stroke-linecap="round" stroke-linejoin="round"><path d="M0 10V80M0 60a20 20 0 1 0 40 0a20 20 0 1 0-40 0M61 10V80M122 60A20 20 0 1 0 102 80M143 10V80M143 57a17 17 0 0 1 34 0V80"/>${xyz ? '<path d="M217.9 40L251.9 80M251.9 40L217.9 80M272.9 40L290.9 78M308.9 40L280.5 100M329.9 40H361.9L329.9 80H361.9" opacity=".55"/>' : ''}</g><path d="M103 60h19v25a9.5 9.5 0 0 1-19 0z" fill="${PINK}"/><path d="M112.5 73v12" stroke="${GROOVE}" stroke-width="2.6" stroke-linecap="round"/><path d="M82 60H122" stroke="currentColor" stroke-width="12" stroke-linecap="round"/><circle cx="197.4" cy="78.6" r="7.4" fill="${PINK}"/></g>`;
}
// The "e" glyph from brand/mark.svg, centred on (cx, cy); s is the scale of the 64-unit mark grid.
function glyph(cx, cy, s, stroke = CREAM) {
  return `<g transform="translate(${cx} ${cy}) scale(${s}) translate(-30 -31.5)"><path d="M47 29A17 17 0 1 0 30 46" fill="none" stroke="${stroke}" stroke-width="9" stroke-linecap="round"/><path d="M31 29h15v19a7.5 7.5 0 0 1-15 0z" fill="${PINK}"/><path d="M38.5 38v10" stroke="${GROOVE}" stroke-width="2.4" stroke-linecap="round"/><path d="M13 29H47" stroke="${stroke}" stroke-width="9" stroke-linecap="round"/></g>`;
}
const chip = (x, y, str, bg = TAPE, fg = INK, size = 20) => { const w = str.length * size * 0.68 + size * 1.5; return rect(x, y, w, size * 2.1, bg, size * 0.5) + t(x + size * 0.8, y + size * 1.4, size, str, fg, 'm', 'letter-spacing=".08em"'); };
function rng(seed) { let s = seed >>> 0; return () => { s = (s * 1664525 + 1013904223) >>> 0; return s / 4294967296; }; }
// Illustrative order-book ladder: order-by-order segments, asks pink, bids yellow.
function ladder(x, y, w, rowH, rows, seed = 5) {
  const r = rng(seed); let h = '';
  for (let i = 0; i < rows * 2; i++) {
    const ask = i < rows, yy = y + i * rowH + (ask ? 0 : rowH * 0.6); let xx = x; const n = 2 + (r() * 5 | 0);
    for (let j = 0; j < n; j++) { const ww = Math.min(w - (xx - x), (0.06 + r() * r() * 0.3) * w); if (ww < 6) break; h += rect(xx.toFixed(1), yy, ww.toFixed(1), rowH * 0.56, ask ? PINK : TAPE, rowH * 0.12); xx += ww + rowH * 0.12; }
  }
  return h + `<path d="M${x} ${y + rows * rowH + rowH * 0.08}H${x + w}" stroke="${MUTE}" stroke-width="2" stroke-dasharray="6 8" opacity=".6"/>`;
}

const A = {}; // name -> [w, h, svg]
// --- link previews ---
const preview = (w, h, lines, foot, title) => {
  const k = h / 630;
  return svg(w, h, [
    rect(0, 0, w, h, INK),
    glyph(w - 215 * k, h / 2 - 4 * k, 9.6 * k),
    word(72 * k, 64 * k, 230 * k, CREAM),
    ...lines.map((l, i) => t(70 * k, (268 + i * 92) * k, 92 * k, l[0], l[1])),
    t(72 * k, h - 74 * k, 23 * k, foot, MUTE, 'm', 'letter-spacing=".06em"'),
    chip(330 * k, 72 * k, 'EARLY PREVIEW', TAPE, INK, 16 * k),
  ].join('\n'), title);
};
A['og-1200x630'] = [1200, 630, preview(1200, 630, [['Market data', CREAM], ['without the', CREAM], ['permission slip.', PINK]], 'open · signed per block · mirrored · CC0', 'bleh.xyz: market data without the permission slip')];
A['og-manifesto-1200x630'] = [1200, 630, preview(1200, 630, [['A price is a fact.', CREAM], ['Facts are free.', PINK]], 'the bleh manifesto · bleh.xyz', 'The bleh manifesto: a price is a fact, facts are free')];
A['github-social-1280x640'] = [1280, 640, preview(1280, 640, [['Open market', CREAM], ['data. No', CREAM], ['permission slip.', PINK]], 'github.com/codeurge/bleh.xyz · Apache-2.0 code · CC0 data', 'bleh.xyz on GitHub: open market data, no permission slip')];

// --- profile banners (keep the bottom-left clear for the avatar overlap) ---
A['x-header-1500x500'] = [1500, 500, svg(1500, 500, [
  rect(0, 0, 1500, 500, TAPE),
  word(430, 110, 500, INK, { xyz: false }),
  t(990, 226, 42, 'Open market data.', INK), t(990, 278, 42, 'No permission slip.', INK),
  t(992, 330, 18, 'SIGNED · MIRRORED · CC0', INK, 'm', 'letter-spacing=".12em"'),
].join('\n'), 'bleh. Open market data. No permission slip.')];
A['linkedin-banner-1584x396'] = [1584, 396, svg(1584, 396, [
  rect(0, 0, 1584, 396, INK),
  word(520, 92, 470, CREAM),
  t(1060, 176, 40, 'Open market data.', CREAM), t(1060, 226, 40, 'No permission slip.', PINK),
  t(1060, 280, 17, 'OPEN SOURCE · SIGNED PER BLOCK · CC0', MUTE, 'm', 'letter-spacing=".12em"'),
  t(1060, 310, 17, 'EARLY PREVIEW', TAPE, 'm', 'letter-spacing=".12em"'),
].join('\n'), 'bleh.xyz. Open market data. No permission slip.')];

// --- avatar and icons ---
A['avatar-400x400'] = [400, 400, svg(400, 400, rect(0, 0, 400, 400, INK) + glyph(200, 200, 4.6), 'bleh.xyz mark')];
A['apple-touch-icon-180'] = [180, 180, svg(180, 180, rect(0, 0, 180, 180, INK) + glyph(90, 90, 2.5), 'bleh.xyz mark')];
A['icon-512-maskable'] = [512, 512, svg(512, 512, rect(0, 0, 512, 512, INK) + glyph(256, 256, 5.4), 'bleh.xyz mark, maskable')];

// --- announcement cards ---
const cardFoot = (fg, chipBg = TAPE, chipFg = INK) => word(72, 930, 200, fg) + chip(1080 - 72 - 186, 938, 'EARLY PREVIEW', chipBg, chipFg, 18);
A['card-1-market-data-bleh'] = [1080, 1080, svg(1080, 1080, [
  rect(0, 0, 1080, 1080, TAPE),
  t(72, 250, 168, 'market', INK), t(72, 410, 168, 'data?', INK),
  word(64, 470, 900, INK, { xyz: false }),
  t(76, 990, 24, 'OPEN MARKET DATA · EARLY PREVIEW · BLEH.XYZ', INK, 'm', 'letter-spacing=".1em"'),
].join('\n'), 'market data? bleh.')];
A['card-2-no-embargo'] = [1080, 1080, svg(1080, 1080, [
  rect(0, 0, 1080, 1080, INK),
  t(72, 96, 22, 'ILLUSTRATIVE', MUTE, 'm', 'letter-spacing=".14em"'),
  ladder(72, 130, 936, 44, 5),
  t(68, 720, 132, 'Order books,', CREAM), t(68, 850, 132, 'no embargo.', PINK),
  cardFoot(CREAM),
].join('\n'), 'Order books, no embargo.')];
A['card-3-signed-mirrored-cc0'] = [1080, 1080, svg(1080, 1080, [
  rect(0, 0, 1080, 1080, CREAM),
  t(68, 270, 186, 'Signed.', INK), t(68, 480, 186, 'Mirrored.', INK),
  `<text x="72" y="690" font-size="186" fill="${GROOVE}" class="m" letter-spacing="-.06em">CC0<tspan class="s" fill="${INK}" dx="6">.</tspan></text>`,
  t(76, 790, 34, 'Every block signed. Four independent mirrors.', STONE, 'b'), t(76, 836, 34, 'Data in the public domain.', STONE, 'b'),
  cardFoot(INK, INK, CREAM),
].join('\n'), 'Signed. Mirrored. CC0.')];
A['card-4-no-takedowns'] = [1080, 1080, svg(1080, 1080, [
  rect(0, 0, 1080, 1080, PINK),
  `<path d="M700 -20h260v300a130 130 0 0 1-260 0z" fill="${GROOVE}" opacity=".35"/><path d="M830 150v170" stroke="${GROOVE}" stroke-width="22" stroke-linecap="round"/>`,
  t(68, 500, 164, 'We don’t', INK), t(68, 670, 164, 'do', INK), t(68, 840, 164, 'takedowns.', INK),
  t(76, 150, 26, 'NOTHING LICENSED UPSTREAM.', INK, 'm', 'letter-spacing=".1em"'), t(76, 190, 26, 'EVERY COPY IS THE ORIGINAL.', INK, 'm', 'letter-spacing=".1em"'),
  word(72, 930, 200, INK).replaceAll(PINK, CREAM).replaceAll(GROOVE, INK) + chip(1080 - 72 - 186, 938, 'EARLY PREVIEW', INK, CREAM, 18),
].join('\n'), 'We don’t do takedowns.')];

// --- sticker sheet: art on the "art" layer, die lines on the "cut" layer (0.25 pt magenta, 3 mm outside the art) ---
const CUT = 'fill="none" stroke="#ff00ff" stroke-width="1"';
const sticker = [
  // wordmark pill
  [`<rect x="60" y="60" width="520" height="210" rx="105" fill="${CREAM}"/>${word(110, 100, 420, INK)}`, `<rect x="48" y="48" width="544" height="234" rx="117" ${CUT}/>`],
  // round mark
  [`<circle cx="760" cy="165" r="105" fill="${INK}"/>${glyph(760, 165, 2.7)}`, `<circle cx="760" cy="165" r="117" ${CUT}/>`],
  // market data? bleh.
  [`<rect x="930" y="60" width="410" height="410" rx="36" fill="${TAPE}"/>${t(962, 150, 70, 'market', INK)}${t(962, 222, 70, 'data?', INK)}${word(956, 250, 350, INK, { xyz: false })}`, `<rect x="918" y="48" width="434" height="434" rx="48" ${CUT}/>`],
  // no embargo
  [`<rect x="60" y="330" width="400" height="300" rx="36" fill="${INK}"/>${ladder(92, 366, 336, 22, 3, 9)}${t(90, 566, 50, 'no embargo.', CREAM)}${t(92, 602, 15, 'ORDER BOOKS · ORDER BY ORDER', MUTE, 'm', 'letter-spacing=".1em"')}`, `<rect x="48" y="318" width="424" height="324" rx="48" ${CUT}/>`],
  // CC0 badge
  [`<circle cx="690" cy="480" r="150" fill="${CREAM}" stroke="${INK}" stroke-width="10"/><text x="690" y="512" font-size="104" text-anchor="middle" class="m" fill="${INK}" letter-spacing="-.06em">CC0</text>${t(690, 410, 19, 'DATA IN THE', INK, 'm', 'text-anchor="middle" letter-spacing=".14em"')}${t(690, 562, 19, 'PUBLIC DOMAIN', GROOVE, 'm', 'text-anchor="middle" letter-spacing=".14em"')}`, `<circle cx="690" cy="480" r="167" ${CUT}/>`],
  // tongue
  [`<path d="M960 540h150v190a75 75 0 0 1-150 0z" fill="${PINK}"/><path d="M1035 640v96" stroke="${GROOVE}" stroke-width="18" stroke-linecap="round"/><path d="M930 540H1140" stroke="${INK}" stroke-width="44" stroke-linecap="round"/>`, `<path d="M930 506h210a34 34 0 0 1 34 34v0a34 34 0 0 1-34 34h-18v156a87 87 0 0 1-174 0V574h-18a34 34 0 0 1-34-34v0a34 34 0 0 1 34-34z" ${CUT}/>`],
  // signed. mirrored. CC0. strip
  [`<rect x="60" y="700" width="700" height="130" rx="65" fill="${PINK}"/><text x="410" y="786" font-size="58" text-anchor="middle" class="s" fill="${INK}">signed. mirrored. <tspan class="m" letter-spacing="-.06em">CC0</tspan>.</text>`, `<rect x="48" y="688" width="724" height="154" rx="77" ${CUT}/>`],
  // no permission slip
  [`<rect x="1200" y="540" width="140" height="290" rx="28" fill="${INK}"/><text transform="translate(1281 685) rotate(-90)" font-size="31" text-anchor="middle" class="s" fill="${CREAM}">no permission slip</text>`, `<rect x="1188" y="528" width="164" height="314" rx="40" ${CUT}/>`],
];
A['sticker-sheet'] = [1400, 900, svg(1400, 900, `${rect(0, 0, 1400, 900, '#ffffff')}\n<g id="art">\n${sticker.map(s => s[0]).join('\n')}\n</g>\n<g id="cut">\n${sticker.map(s => s[1]).join('\n')}\n</g>`, 'bleh.xyz sticker sheet with die lines')];

// ---------- write SVG, rasterize ----------
const page = await launch();
async function shot(file, w, h, out, transparent = false) {
  await page.viewport(w, h, 1);
  await page.send('Emulation.setDefaultBackgroundColorOverride', transparent ? { color: { r: 0, g: 0, b: 0, a: 0 } } : {});
  await page.goto(pathToFileURL(file).href);
  await page.eval(`Promise.all(['800 40px "DM Sans"','500 40px "DM Sans"','500 40px "DM Mono"'].map(f=>document.fonts.load(f))).then(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))))`);
  const png = await page.png(); writeFileSync(out, png); return png;
}
try {
  for (const [name, [w, h, body]] of Object.entries(A)) {
    const f = join(SRC, name + '.svg'); writeFileSync(f, body);
    await shot(f, w, h, join(OUT, name + '.png'));
    console.log('wrote', name);
  }
  // favicons straight from brand/mark.svg (rounded corners stay transparent)
  const ico = [];
  for (const s of [16, 32, 48]) ico.push([s, await shot(MARK, s, s, join(OUT, `favicon-${s}.png`), true)]);
  const head = Buffer.alloc(6 + 16 * ico.length); head.writeUInt16LE(1, 2); head.writeUInt16LE(ico.length, 4);
  let off = head.length;
  ico.forEach(([s, png], i) => { const o = 6 + 16 * i; head[o] = s; head[o + 1] = s; head.writeUInt16LE(1, o + 4); head.writeUInt16LE(32, o + 6); head.writeUInt32LE(png.length, o + 8); head.writeUInt32LE(off, o + 12); off += png.length; });
  writeFileSync(join(OUT, 'favicon.ico'), Buffer.concat([head, ...ico.map(i => i[1])]));
  console.log('wrote favicon set');
} finally { await page.close(); }

// ---------- copy what the site references ----------
for (const [from, to] of [['og-1200x630.png', 'og.png'], ['og-manifesto-1200x630.png', 'og-manifesto.png'], ['favicon.ico', 'favicon.ico'], ['favicon-16.png', 'favicon-16.png'], ['favicon-32.png', 'favicon-32.png'], ['apple-touch-icon-180.png', 'apple-touch-icon.png'], ['icon-512-maskable.png', 'icon-512-maskable.png']]) copyFileSync(join(OUT, from), join(SITE, to));
writeFileSync(join(SITE, 'favicon.svg'), readFileSync(MARK));
writeFileSync(join(SITE, 'site.webmanifest'), JSON.stringify({ name: 'bleh.xyz', short_name: 'bleh', description: 'Open market data. Early preview.', start_url: '../', display: 'browser', background_color: '#16141c', theme_color: '#16141c', icons: [{ src: 'icon-512-maskable.png', sizes: '512x512', type: 'image/png', purpose: 'any maskable' }, { src: 'apple-touch-icon.png', sizes: '180x180', type: 'image/png' }] }, null, 2) + '\n');
console.log('copied site assets');
