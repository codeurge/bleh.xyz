/* bleh.xyz site script. Progressive enhancement only. The one network request
   reads the public web snapshot (docs/specs/web-snapshot-v0.md) for the data panel. */
(function () {
  var root = document.documentElement;
  var reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;

  // theme toggle (remembers the choice; default follows the system)
  var tbtn = document.getElementById('theme');
  if (tbtn) tbtn.addEventListener('click', function () {
    var cur = root.getAttribute('data-theme') || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
    var next = cur === 'dark' ? 'light' : 'dark';
    root.setAttribute('data-theme', next);
    try { localStorage.setItem('bleh-theme', next); } catch (e) {}
  });

  // sticky nav border
  var nav = document.querySelector('.nav');
  if (nav) addEventListener('scroll', function () { nav.classList.toggle('stuck', scrollY > 8); }, { passive: true });

  // scroll reveals
  var els = document.querySelectorAll('.reveal,.popme');
  if ('IntersectionObserver' in window && !reduce) {
    var io = new IntersectionObserver(function (es) {
      es.forEach(function (e) { if (e.isIntersecting) { e.target.classList.add('in'); io.unobserve(e.target); } });
    }, { rootMargin: '0px 0px -8% 0px', threshold: 0.08 });
    els.forEach(function (el) { io.observe(el); });
  } else els.forEach(function (el) { el.classList.add('in'); });

  // code tabs (WAI-ARIA tabs pattern)
  document.querySelectorAll('[role=tablist]').forEach(function (list) {
    var tabs = [].slice.call(list.querySelectorAll('[role=tab]'));
    function pick(tab, focus) {
      tabs.forEach(function (t) {
        var on = t === tab;
        t.setAttribute('aria-selected', on); t.tabIndex = on ? 0 : -1;
        document.getElementById(t.getAttribute('aria-controls')).hidden = !on;
      });
      if (focus) tab.focus();
    }
    tabs.forEach(function (t, i) {
      t.addEventListener('click', function () { pick(t); });
      t.addEventListener('keydown', function (e) {
        var d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
        if (e.key === 'Home') { e.preventDefault(); pick(tabs[0], true); }
        else if (e.key === 'End') { e.preventDefault(); pick(tabs[tabs.length - 1], true); }
        else if (d) { e.preventDefault(); pick(tabs[(i + d + tabs.length) % tabs.length], true); }
      });
    });
  });

  // ---- reference data panel: public web snapshot, shown only if it loads ----
  (function () {
    var sec = document.getElementById('data'), tiles = document.getElementById('tiles');
    var meta = document.querySelector('meta[name="bleh-data"]');
    if (!sec || !tiles || !meta || !window.fetch) return;
    var base = meta.content.replace(/\/?$/, '/');
    // label, source, instrument, unit, decimals
    var PICK = [
      ['SOFR', 'nyfed', 'nyfed:ref:SOFR', '%', 2],
      ['Effective fed funds', 'nyfed', 'nyfed:ref:EFFR', '%', 2],
      ['Overnight bank funding', 'nyfed', 'nyfed:ref:OBFR', '%', 2],
      ['Treasury 3-month', 'frb-h15', 'frb-h15:ref:RIFLGFCM03_N.B', '%', 2],
      ['Treasury 2-year', 'frb-h15', 'frb-h15:ref:RIFLGFCY02_N.B', '%', 2],
      ['Treasury 10-year', 'frb-h15', 'frb-h15:ref:RIFLGFCY10_N.B', '%', 2],
      ['Treasury 30-year', 'frb-h15', 'frb-h15:ref:RIFLGFCY30_N.B', '%', 2],
      ['US dollars per euro', 'frb-h10', 'frb-h10:ref:RXI@US_N.B.EU', '', 4],
      ['Yen per US dollar', 'frb-h10', 'frb-h10:ref:RXI_N.B.JA', '', 2],
      ['Class III milk', 'usda-dairy', 'usda-dairy:ref:FMMO.class_3_Price', '$/cwt', 2],
      // Quantities (ADR 0023): the unit comes from the series when it has one.
      ['US crude stocks', 'eia', 'eia:ref:PET.WCESTUS1.W', '', 0],
      ['Cushing crude stocks', 'eia', 'eia:ref:PET.W_EPC0_SAX_YCUOK_MBBL.W', '', 0],
      ['Natural gas storage', 'eia', 'eia:ref:NG.NW2_EPG0_SWO_R48_BCF.W', '', 0],
      ['Regular gasoline, retail', 'eia', 'eia:ref:PET.EMM_EPMR_PTE_NUS_DPG.W', '$/gal', 3],
      ['Diesel, retail', 'eia', 'eia:ref:PET.EMD_EPD2D_PTE_NUS_DPG.W', '$/gal', 3],
      ['CPI-U, all items', 'bls', 'bls:ref:CUUR0000SA0', 'index', 3]
    ];
    var UNITS = { 'thousand_barrels': 'kbbl', 'thousand_barrels_per_day': 'kb/d', 'barrels': 'bbl', 'bcf': 'Bcf', 'billion_cubic_feet': 'Bcf',
      'mmcf': 'MMcf', 'million_cubic_feet': 'MMcf', 'percent': '%', 'pounds': 'lb', 'metric_tons': 't', 'head': 'head',
      'usd': 'USD', 'million_usd': '$M', 'billion_usd': '$B' };
    var NAMES = { 'nyfed': 'Federal Reserve Bank of New York', 'frb-h15': 'Federal Reserve H.15', 'frb-h10': 'Federal Reserve H.10',
      'us-treasury': 'U.S. Treasury', 'usda-dairy': 'USDA dairy', 'usda-lmr': 'USDA livestock', 'cftc-cot': 'CFTC Commitments of Traders', 'ercot': 'ERCOT',
      'cftc-part43': 'CFTC Part 43 swaps', 'sec-sbsr': 'SEC security-based swaps', 'eia': 'EIA', 'bls': 'BLS' };
    function get(path) {
      return fetch(base + path, { mode: 'cors', credentials: 'omit' }).then(function (r) {
        if (!r.ok) throw new Error(path + ' ' + r.status); return r.json();
      });
    }
    function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
    function num(v, dp) { var n = parseFloat(v); return isFinite(n) ? n.toLocaleString('en-US', { minimumFractionDigits: dp, maximumFractionDigits: dp }) : String(v); }
    function spark(points) {
      var vals = points.map(function (p) { return parseFloat(p[1]); }).filter(isFinite);
      if (vals.length < 2) return null;
      vals = vals.slice(-90);
      var lo = Math.min.apply(null, vals), hi = Math.max.apply(null, vals), w = 120, h = 32, span = hi - lo || 1;
      var d = vals.map(function (v, i) { return (i * w / (vals.length - 1)).toFixed(1) + ',' + (h - 2 - (v - lo) / span * (h - 4)).toFixed(1); }).join(' ');
      var ns = 'http://www.w3.org/2000/svg', svg = document.createElementNS(ns, 'svg'), pl = document.createElementNS(ns, 'polyline');
      svg.setAttribute('viewBox', '0 0 ' + w + ' ' + h); svg.setAttribute('class', 'spark'); svg.setAttribute('aria-hidden', 'true');
      pl.setAttribute('points', d); svg.appendChild(pl); return svg;
    }
    var raw = document.getElementById('data-raw'); if (raw) raw.href = base + 'latest.json';
    get('latest.json').then(function (latest) {
      var bySource = {}; (latest.sources || []).forEach(function (s) { bySource[s.source] = s; });
      var need = {}; PICK.forEach(function (p) { if (bySource[p[1]]) need[p[1]] = true; });
      var names = Object.keys(need);
      return Promise.all(names.map(function (n) { return get('reference/' + n + '.json').catch(function () { return null; }); }))
        .then(function (files) { return { latest: latest, bySource: bySource, files: files, names: names }; });
    }).then(function (d) {
      var series = {};
      d.files.forEach(function (f) { if (f) (f.series || []).forEach(function (s) { series[s.instrument] = s; }); });
      var shown = {};
      PICK.forEach(function (p) {
        var s = series[p[2]]; if (!s || !s.latest) return;
        var t = el('div', 'tile');
        t.appendChild(el('span', 'tl', p[0]));
        var v = el('span', 'tv', num(s.latest.value, p[4]));
        var u = s.unit ? (UNITS[s.unit] || s.unit.replace(/_/g, ' ')) : p[3];
        if (u) v.appendChild(el('small', null, ' ' + u));
        t.appendChild(v);
        var pts = s.points || [];
        if (pts.length > 1) {
          var ch = parseFloat(pts[pts.length - 1][1]) - parseFloat(pts[pts.length - 2][1]);
          if (isFinite(ch)) t.appendChild(el('span', 'tc ' + (ch > 0 ? 'up' : ch < 0 ? 'dn' : ''), (ch > 0 ? '+' : ch < 0 ? '−' : '±') + num(Math.abs(ch), p[4])));
        }
        var sp = spark(pts); if (sp) t.appendChild(sp);
        t.appendChild(el('span', 'ts', (NAMES[p[1]] || p[1]) + ' · ' + s.latest.as_of_date));
        tiles.appendChild(t); shown[p[1]] = true;
      });
      if (!tiles.children.length) return;
      var srcs = d.latest.sources || [];
      var m = document.getElementById('data-meta');
      if (m) m.textContent = srcs.length + ' open sources, ' + srcs.reduce(function (a, s) { return a + (s.records || 0); }, 0).toLocaleString('en-US') +
        ' records read. Snapshot built ' + new Date(d.latest.generated_ms).toUTCString().replace(' GMT', ' UTC') + '.';
      var ul = document.getElementById('notices');
      srcs.forEach(function (s) {
        var li = el('li'); li.appendChild(el('b', null, (NAMES[s.source] || s.source) + ' ')); li.appendChild(el('span', 'mono', s.admission + (s.latest_as_of ? ' · through ' + s.latest_as_of : '')));
        if (s.notice) li.appendChild(el('p', null, s.notice));
        ul.appendChild(li);
      });
      // Notices of the sources shown above stay visible next to the values.
      Object.keys(shown).forEach(function (n) {
        var s = d.bySource[n]; if (s && s.notice) tiles.parentNode.insertBefore(el('p', 'notice', s.notice), m);
      });
      sec.hidden = false;
    }).catch(function () { /* no snapshot reachable: the panel stays hidden */ });
  })();

  // ---- illustrative order book and tape: seeded random walk, drawn locally ----
  var book = document.getElementById('book'), tape = document.getElementById('tape');
  if (!book || !tape) return;
  var MK = {
    'BTC-PERP': { p: 104250, tick: 1, dp: 0, sz: 0.9, sd: 3 },
    'GOLD': { p: 4012.4, tick: 0.1, dp: 1, sz: 14, sd: 2 },
    'WTI': { p: 71.36, tick: 0.01, dp: 2, sz: 220, sd: 0 }
  };
  var seed = 7, m, mid, levels, blk = 812440112, timer, clock = 0;
  function rnd() { seed = (seed * 1664525 + 1013904223) >>> 0; return seed / 4294967296; }
  function fmt(v, dp) { return v.toLocaleString('en-US', { minimumFractionDigits: dp, maximumFractionDigits: dp }); }
  function hex(n) { var s = ''; for (var i = 0; i < n; i++) s += '0123456789abcdef'[rnd() * 16 | 0]; return s; }
  function mkLevel() { var n = 1 + (rnd() * 5 | 0), o = []; for (var i = 0; i < n; i++) o.push(0.15 + rnd() * rnd() * 1.6); return o; }
  function reset(name) {
    m = MK[name]; mid = m.p; levels = { a: [], b: [] };
    for (var i = 0; i < 6; i++) { levels.a.push(mkLevel()); levels.b.push(mkLevel()); }
    tape.innerHTML = ''; for (var k = 0; k < 12; k++) trade(true);
    draw();
  }
  function row(side, i, orders, fresh) {
    var px = mid + (side === 'a' ? 1 : -1) * (i + 0.5) * m.tick * (m.dp === 0 ? 2 : 1), tot = 0, h = '';
    orders.forEach(function (o, j) { tot += o; h += '<i' + (fresh && j === orders.length - 1 ? ' class="new"' : '') + ' style="width:' + Math.max(4, o * 26).toFixed(0) + 'px"></i>'; });
    return '<div class="lvl ' + side + '"><span class="px">' + fmt(px, m.dp) + '</span><span class="ords">' + h + '</span><span class="sz">' + fmt(tot * m.sz, m.sd) + '</span></div>';
  }
  function draw(fs, fi) {
    var h = '';
    for (var i = 5; i >= 0; i--) h += row('a', i, levels.a[i], fs === 'a' && fi === i);
    h += '<div class="spread"><span>spread <b>' + fmt(m.tick * (m.dp === 0 ? 2 : 1), m.dp) + '</b></span><span>mid <b>' + fmt(mid, m.dp) + '</b></span></div>';
    for (var j = 0; j < 6; j++) h += row('b', j, levels.b[j], fs === 'b' && fi === j);
    book.innerHTML = h;
  }
  function trade(quiet) {
    var buy = rnd() > 0.48, li = document.createElement('li');
    clock += 40 + (rnd() * 900 | 0);
    var ms = 34200000 + clock, t = new Date(ms).toISOString().substr(11, 8);
    li.className = (buy ? 'buy' : 'sell') + (quiet ? '' : ' fresh');
    li.innerHTML = '<span class="t">' + t + '</span><span class="p">' + fmt(mid + (buy ? 0.5 : -0.5) * m.tick, m.dp) + '</span><span class="s">' + fmt((0.05 + rnd() * rnd() * 2) * m.sz, m.sd) + '</span><span class="d" aria-label="' + (buy ? 'buyer' : 'seller') + ' aggressor">' + (buy ? '▲' : '▼') + '</span>';
    tape.insertBefore(li, tape.firstChild);
    while (tape.children.length > 12) tape.removeChild(tape.lastChild);
    return buy;
  }
  function tick() {
    var r = rnd(), side = rnd() > 0.5 ? 'a' : 'b', i = rnd() * 6 | 0, L = levels[side][i];
    if (r < 0.34) {            // aggressor takes the front of the queue
      var buy = trade(); var q = levels[buy ? 'a' : 'b'][0]; q.shift();
      if (!q.length) { levels[buy ? 'a' : 'b'].shift(); levels[buy ? 'a' : 'b'].push(mkLevel()); levels[buy ? 'b' : 'a'].unshift([0.3 + rnd()]); levels[buy ? 'b' : 'a'].pop(); mid += (buy ? 1 : -1) * m.tick * (m.dp === 0 ? 2 : 1); }
      draw();
    } else if (r < 0.72) { if (L.length < 7) L.push(0.15 + rnd() * rnd() * 1.6); draw(side, i); } // book_add
    else { if (L.length > 1) L.splice(rnd() * L.length | 0, 1); draw(); }                           // book_cancel
    if (rnd() < 0.3) { blk++; var s = document.getElementById('sig'); if (s) s.innerHTML = 'block <b>' + blk.toLocaleString('en-US') + '</b> · root <b>0x' + hex(6) + '…' + hex(4) + '</b>'; }
  }
  document.querySelectorAll('.ptabs button').forEach(function (b) {
    b.addEventListener('click', function () {
      document.querySelectorAll('.ptabs button').forEach(function (x) { x.setAttribute('aria-pressed', x === b); });
      reset(b.textContent.trim());
    });
  });
  reset('BTC-PERP');
  function start() { if (!timer && !reduce) timer = setInterval(tick, 420); }
  function stop() { clearInterval(timer); timer = null; }
  document.addEventListener('visibilitychange', function () { document.hidden ? stop() : start(); });
  if ('IntersectionObserver' in window) new IntersectionObserver(function (e) { e[0].isIntersecting ? start() : stop(); }).observe(book);
  else start();
})();

// brand film: a muted preview loops while the film is on screen (not with reduced motion
// or Data Saver); "Play with sound" restarts it from the top with sound and simple controls.
// Without this script the <video> keeps its native controls.
(function () {
  var v = document.getElementById('film-video'); if (!v) return;
  var reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  var saveData = navigator.connection && navigator.connection.saveData;
  var frame = v.parentNode, btn = frame.querySelector('.film-play'), label = btn.querySelector('.film-play-label');
  var bar = frame.querySelector('.film-bar'), seek = bar.querySelector('.film-seek'), time = bar.querySelector('.film-time');
  var toggleBtn = bar.querySelector('[data-act=toggle]'), muteBtn = bar.querySelector('[data-act=mute]');
  var tall = matchMedia('(max-width: 640px) and (orientation: portrait)').matches;
  frame.classList.toggle('tall', tall);
  v.poster = v.getAttribute(tall ? 'data-tall-poster' : 'data-wide-poster');
  v.removeAttribute('controls');
  while (v.firstChild) v.removeChild(v.firstChild);
  v.src = v.getAttribute(tall ? 'data-tall' : 'data-wide');
  v.muted = true; v.loop = true;
  btn.hidden = false;
  var engaged = false, seen = false, idleTimer;

  function play() { var p = v.play(); if (p && p.catch) p.catch(function () { sync(); }); }
  function fmt(t) { t = Math.max(0, Math.floor(t || 0)); return Math.floor(t / 60) + ':' + ('0' + t % 60).slice(-2); }
  function sync() {
    frame.classList.toggle('paused', v.paused); frame.classList.toggle('muted', v.muted);
    toggleBtn.setAttribute('aria-label', v.paused ? 'Play' : 'Pause');
    muteBtn.setAttribute('aria-label', v.muted ? 'Unmute' : 'Mute');
  }
  function wake() {
    frame.classList.remove('idle'); clearTimeout(idleTimer);
    if (engaged) idleTimer = setTimeout(function () { frame.classList.add('idle'); }, 2200);
  }
  function engage() {
    engaged = true; seen = false;
    frame.classList.add('engaged'); btn.hidden = true; bar.hidden = false;
    v.loop = false; v.muted = false; v.preload = 'auto';
    try { v.currentTime = 0; } catch (e) {}
    play(); sync(); wake();
  }
  btn.addEventListener('click', function () { engage(); toggleBtn.focus({ preventScroll: true }); });
  v.addEventListener('click', function () { if (!engaged) engage(); else if (v.paused) play(); else v.pause(); });
  bar.addEventListener('click', function (e) {
    var b = e.target.closest('[data-act]'); if (!b) return;
    var act = b.getAttribute('data-act');
    if (act === 'toggle') { if (v.paused) play(); else v.pause(); }
    else if (act === 'mute') v.muted = !v.muted;
    else if (act === 'full') {
      if (document.fullscreenElement) document.exitFullscreen();
      else if (frame.requestFullscreen) frame.requestFullscreen();
      else if (v.webkitEnterFullscreen) v.webkitEnterFullscreen();
    }
  });
  seek.addEventListener('input', function () { v.currentTime = parseFloat(seek.value); });
  v.addEventListener('loadedmetadata', function () { seek.max = v.duration || 30; });
  v.addEventListener('timeupdate', function () { if (!engaged) return; seek.value = v.currentTime; time.textContent = fmt(v.currentTime); });
  ['play', 'pause', 'volumechange'].forEach(function (ev) { v.addEventListener(ev, sync); });
  v.addEventListener('ended', function () {
    engaged = false; frame.classList.remove('engaged', 'idle'); bar.hidden = true; btn.hidden = false;
    label.firstChild.nodeValue = 'Watch again'; btn.setAttribute('aria-label', 'Watch the 30-second film again');
  });
  ['mousemove', 'touchstart', 'focusin', 'keydown'].forEach(function (ev) { frame.addEventListener(ev, wake, { passive: true }); });

  // Preview while in view; once playing with sound, pause when scrolled away.
  if ('IntersectionObserver' in window) new IntersectionObserver(function (es) {
    var vis = es[0].isIntersecting;
    if (engaged) { if (vis) seen = true; else if (seen && !v.paused) v.pause(); return; }
    if (reduce || saveData) return;
    if (vis) { v.preload = 'auto'; v.muted = true; play(); } else if (!v.paused) v.pause();
  }, { threshold: 0.35 }).observe(frame);

  // "Watch the film" in the hero: scroll to it and start with sound (same click, so audio is allowed).
  document.querySelectorAll('[data-film-play]').forEach(function (a) {
    a.addEventListener('click', function (e) {
      e.preventDefault();
      frame.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'center' });
      engage();
    });
  });
})();

// open archive: probes the known archives, lists the signed days of the one picked, re-checks
// its hash chain in the browser (each manifest's SHA-256 against the index and the next
// manifest's previous_manifest_sha256), and queries one instrument's Parquet files with
// DuckDB-WASM, loaded from jsDelivr only when asked. Results can be charted and exported.
// docs/specs/manifest-v0.md, docs/specs/archive-layout-v0.md.
(function () {
  var sec = document.getElementById('archive'); if (!sec || !window.fetch || !window.crypto || !crypto.subtle) return;
  var meta = document.querySelector('meta[name="bleh-archive"]');
  var ORIGIN = (meta && meta.content || 'https://data.bleh.xyz/').replace(/\/?$/, '/');
  // The archives this page knows. Unpublished ones (404) are shown as coming soon.
  var ARCHIVES = [
    { name: 'hyperliquid-backfill', label: 'Hyperliquid trades', desc: "Backfilled from the venue's public history, from 2025-07-27." },
    { name: 'reference-normalized', label: 'Reference series', desc: 'Rates, exchange rates, dairy prices and positioning from public bodies.' },
    { name: 'swaps-normalized', label: 'Swap prints', desc: 'Public swap reports from the CFTC and SEC repositories.' },
    { name: 'hyperliquid-backfill-early', label: 'Hyperliquid, early period', desc: '2025-03-22 to 2025-07-26, once its sequencing method is decided.' }
  ];
  var DUCKDB = 'https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.29.0/+esm';
  var AUTO = 40;    // archives with up to this many days have every manifest checked on load
  var SHOW = 200;   // rows drawn in the result table; exports include every row
  var SMALL = 24;   // a selection of up to this many files defaults to all instruments
  var LISTED = 50;  // file links listed under "Take it with you"
  var PRESET = {
    trade: 'SELECT ts_source_ms, instrument, price / 10 ^ price_scale AS price,\n       size / 10 ^ size_scale AS size, aggressor, block_number, trade_id\nFROM day\nORDER BY block_number, event_index',
    bar: 'SELECT start_ms, instrument, interval, open / 10 ^ price_scale AS open,\n       high / 10 ^ price_scale AS high, low / 10 ^ price_scale AS low,\n       close / 10 ^ price_scale AS close, volume / 10 ^ size_scale AS volume, trade_count\nFROM day\nORDER BY instrument, interval, start_ms',
    reference_value: 'SELECT instrument, measure, as_of_date, value, tenor, currency, revision\nFROM day\nORDER BY instrument, as_of_date, revision'
  };
  var ANY = 'SELECT * FROM day';
  // Price over time, per kind: x (epoch ms), y, optional low/high band, filter, tie-break.
  var CHART = {
    trade: { what: 'Trade price', x: 'epoch_ms(ts_source_ms)', y: 'price / 10 ^ price_scale', by: 't' },
    bar: { what: 'Bar close', x: 'epoch_ms(start_ms)', y: 'close / 10 ^ price_scale', lo: 'low / 10 ^ price_scale', hi: 'high / 10 ^ price_scale', by: 't',
      where: 'interval = (SELECT interval FROM day GROUP BY interval ORDER BY count(*) DESC LIMIT 1)' },
    reference_value: { what: 'Value', x: 'coalesce(epoch_ms(period_start_ms), epoch_ms(TRY_CAST(as_of_date AS DATE)))', y: 'TRY_CAST(value AS DOUBLE)', by: 'revision',
      where: 'measure = (SELECT measure FROM day GROUP BY measure ORDER BY count(*) DESC LIMIT 1)' }
  };
  var $ = function (id) { return document.getElementById(id); };
  var cur = null, token = 0, db = null, conn = null, loading = null, result = null, sqlEdited = false, chartData = null;

  function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
  function clear(e) { while (e.firstChild) e.removeChild(e.firstChild); }
  function hex(buf) { return Array.prototype.map.call(new Uint8Array(buf), function (b) { return ('0' + b.toString(16)).slice(-2); }).join(''); }
  function size(n) { return n < 1024 ? n + ' B' : n < 1048576 ? (n / 1024).toFixed(1) + ' KB' : n < 1073741824 ? (n / 1048576).toFixed(1) + ' MB' : (n / 1073741824).toFixed(2) + ' GB'; }
  function count(n) { return n.toLocaleString('en-US'); }
  function plural(n, one, many) { return count(n) + ' ' + (n === 1 ? one : many || one + 's'); }
  function sq(s) { return "'" + String(s).replace(/'/g, "''") + "'"; }
  // Object keys keep a literal %23 for '#', so an HTTPS URL escapes the '%' (archive-layout-v0 § 3.1).
  function fileUrl(path) { return cur.base + path.replace(/%/g, '%25').replace(/#/g, '%23').replace(/\?/g, '%3F').replace(/ /g, '%20'); }
  function fetchOk(u) { return fetch(u, { mode: 'cors', credentials: 'omit' }).then(function (r) { if (!r.ok) throw new Error(u + ' ' + r.status); return r; }); }
  function stat(value, label, cls) { var d = el('div', 'arch-stat' + (cls ? ' ' + cls : '')); d.appendChild(el('b', null, value)); d.appendChild(el('span', null, label)); return d; }
  // Run fn over items, at most n at a time.
  function pool(items, n, fn) {
    var i = 0;
    function next() { if (i >= items.length) return Promise.resolve(); var it = items[i++]; return fn(it).then(next); }
    var w = []; for (var k = 0; k < Math.min(n, items.length); k++) w.push(next());
    return Promise.all(w);
  }

  // ---- probe every known archive; show the section if any is published ----
  Promise.all(ARCHIVES.map(function (a) {
    return fetch(ORIGIN + a.name + '/_manifests/index.json', { mode: 'cors', credentials: 'omit' })
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (j) { a.index = j && j.manifests && j.manifests.length ? j.manifests : null; }, function () { a.index = null; });
  })).then(function () {
    var pick = $('arch-pick'), first = null;
    ARCHIVES.forEach(function (a) {
      var b = el('button', 'arch-opt'); b.type = 'button';
      b.appendChild(el('b', null, a.label));
      b.appendChild(el('span', null, a.desc));
      b.appendChild(el('em', 'mono', a.index ? plural(a.index.length, 'day') + ' · ' + a.name : 'Coming soon · ' + a.name));
      if (a.index) { b.setAttribute('aria-pressed', 'false'); b.addEventListener('click', function () { if (cur && cur.a === a) return; select(a); }); first = first || a; }
      else { b.disabled = true; b.classList.add('soon'); }
      a.btn = b; pick.appendChild(b);
    });
    if (!first) return;   // nothing published: keep the section hidden
    sec.hidden = false;
    select(first);
  });

  // ---- one archive: index, days table, chain check ----
  function select(a) {
    var my = ++token;
    cur = { a: a, base: ORIGIN + a.name + '/', idx: a.index, days: {}, at: {}, magnets: {}, my: my, sel: null };
    a.index.forEach(function (e, i) { cur.at[e.date] = i; });
    ARCHIVES.forEach(function (x) { if (x.index) x.btn.setAttribute('aria-pressed', x === a ? 'true' : 'false'); });
    result = null; chartData = null;
    clear($('q-out')); $('q-chart').hidden = true; $('q-export').hidden = true; $('q-status').textContent = ''; $('q-export-status').textContent = '';
    $('arch-index').href = cur.base + '_manifests/index.json';
    [].forEach.call(document.querySelectorAll('.arch-cmd-name'), function (s) { s.textContent = a.name; });
    var latest = cur.idx[cur.idx.length - 1];
    $('arch-cmd-date').textContent = latest.date;
    $('arch-signer').textContent = '…';

    var rows = $('arch-rows'), pick = $('q-day');
    clear(rows); clear(pick);
    cur.idx.slice().reverse().forEach(function (e) {
      var tr = el('tr'); tr.setAttribute('data-date', e.date);
      var q = el('button', null, e.date); q.type = 'button'; q.setAttribute('aria-label', 'Query ' + e.date);
      q.addEventListener('click', function () { choose(e.date); $('q-find').focus(); });
      var td = el('td'); td.appendChild(q); tr.appendChild(td);
      tr.appendChild(el('td', null, '–')); tr.appendChild(el('td', null, '–')); tr.appendChild(el('td', null, '–'));
      var links = el('td'), wrap = el('span', 'arch-links');
      var m = el('a', null, 'manifest'); m.href = cur.base + '_manifests/' + e.date + '/manifest.json'; wrap.appendChild(m);
      links.appendChild(wrap); tr.appendChild(links); rows.appendChild(tr);
      var o = el('option', null, e.date); o.value = e.date; pick.appendChild(o);
    });

    fetchOk(cur.base + '_manifests/magnets.txt').then(function (r) { return r.text(); }).then(function (t) {
      if (my !== token) return;
      t.split('\n').forEach(function (line) { var i = line.indexOf(' '); if (i > 0) cur.magnets[line.slice(0, i)] = line.slice(i + 1).trim(); });
      [].forEach.call(rows.querySelectorAll('tr'), function (tr) {
        var mg = cur.magnets[tr.getAttribute('data-date')]; if (!mg) return;
        var a2 = el('a', null, 'magnet'); a2.href = mg; tr.querySelector('.arch-links').appendChild(a2);
      });
      if (cur.sel) renderGet();
    }).catch(function () { /* magnets are optional */ });

    stats(null);
    var from = Math.max(0, cur.idx.length - AUTO);
    $('arch-checkall').hidden = from === 0;
    check(from);
    choose(latest.date);
  }

  // Fetch, hash and parse one day's manifest (cached per archive selection).
  function loadDay(date) {
    var c = cur; if (c.days[date]) return c.days[date];
    var e = c.idx[c.at[date]];
    c.days[date] = fetchOk(c.base + '_manifests/' + date + '/manifest.json').then(function (r) { return r.arrayBuffer(); }).then(function (buf) {
      return crypto.subtle.digest('SHA-256', buf).then(function (h) {
        var man = JSON.parse(new TextDecoder().decode(buf)).manifest || {}, files = man.files || [], rc = 0, sz = 0;
        files.forEach(function (x) { rc += x.record_count || 0; sz += x.size || 0; });
        var d = { date: date, sha: hex(h), man: man, files: files, records: rc, bytes: sz, parsed: files.map(parse) };
        d.ok = d.sha === e.sha256 && man.date === e.date && man.archive === c.a.name && man.sequence === e.sequence;
        if (c.my === token) fillRow(d);
        return d;
      });
    });
    c.days[date].catch(function () { delete c.days[date]; });
    return c.days[date];
  }
  function fillRow(d) {
    var tr = document.querySelector('#arch-rows tr[data-date="' + d.date + '"]'); if (!tr) return;
    var td = tr.children;
    td[1].textContent = d.files.length; td[2].textContent = count(d.records); td[3].textContent = size(d.bytes);
    var o = $('q-day').querySelector('option[value="' + d.date + '"]'); if (o) o.textContent = d.date + ' · ' + plural(d.files.length, 'file');
    if (!d.ok) tr.classList.add('bad');
  }

  // Check the chain from index position `from` to the latest day.
  function check(from) {
    var c = cur, n = c.idx.length, done = 0, list = c.idx.slice(from).map(function (e) { return e.date; });
    var t = setInterval(function () { if (c.my === token && done < list.length) stats({ progress: done, of: list.length }); }, 400);
    return pool(list, 6, function (date) { return loadDay(date).then(function () { done++; }); }).then(function () {
      return Promise.all(list.map(loadDay));
    }).then(function (ds) {
      clearInterval(t); if (c.my !== token) return;
      var broken = null, files = 0, records = 0, bytes = 0;
      ds.forEach(function (d, k) {
        var i = from + k, prev = i === 0 ? null : k > 0 ? ds[k - 1].sha : c.idx[i - 1].sha256;
        if (!d.ok || (d.man.previous_manifest_sha256 || null) !== prev) broken = broken || d.date;
        files += d.files.length; records += d.records; bytes += d.bytes;
      });
      c.checked = { from: from, broken: broken, files: files, records: records, bytes: bytes };
      $('arch-checkall').hidden = from === 0;
      $('arch-signer').textContent = ds[ds.length - 1].man.signer || 'none stated';
      stats(c.checked);
    }, function (err) {
      clearInterval(t); if (c.my !== token) return;
      stats({ error: String(err && err.message || err) });
    });
  }
  $('arch-checkall').addEventListener('click', function () { this.hidden = true; check(0); });

  function stats(s) {
    var st = $('arch-stats'), n = cur.idx.length, partial = s && s.from > 0;
    clear(st);
    st.appendChild(stat(count(n), 'Days published'));
    if (s && s.records != null) {
      st.appendChild(stat(count(s.records), partial ? 'Records, last ' + (n - s.from) + ' days' : 'Records'));
      st.appendChild(stat(size(s.bytes), plural(s.files, 'file') + (partial ? ', last ' + (n - s.from) + ' days' : '')));
    }
    st.appendChild(stat(cur.idx[n - 1].date, 'Latest day'));
    if (!s || s.progress != null) st.appendChild(stat('Checking…', s ? count(s.progress) + ' of ' + count(s.of) + ' manifests' : 'Hash chain'));
    else if (s.error) st.appendChild(stat('Unchecked', 'A manifest did not load', 'bad'));
    else if (s.broken) st.appendChild(stat('Broken', 'Hash chain at ' + s.broken, 'bad'));
    else st.appendChild(stat('✓ Intact', partial ? 'Hash chain, last ' + (n - s.from) + ' days checked here' : 'Hash chain, checked here', 'ok'));
  }

  // ---- files of a day: kind and instrument come from the Hive path (archive-layout-v0 § 3.1) ----
  function parse(f) {
    var seg = {};
    f.path.split('/').forEach(function (s) { var i = s.indexOf('='); if (i > 0) seg[s.slice(0, i)] = s.slice(i + 1); });
    return {
      f: f, seg: seg, pq: /\.parquet$/.test(f.path),
      kind: seg.kind || (seg.stream ? 'raw ' + seg.stream : 'other'),
      inst: seg.instrument ? seg.instrument.replace(/~/g, ':').replace(/%23/g, '#') : null
    };
  }

  function choose(date) {
    var c = cur;
    $('q-day').value = date;
    [].forEach.call(document.querySelectorAll('#arch-rows tr'), function (tr) { tr.classList.toggle('sel', tr.getAttribute('data-date') === date); });
    $('q-count').textContent = 'Loading the manifest…'; $('q-run').disabled = true;
    loadDay(date).then(function (d) {
      if (c.my !== token || $('q-day').value !== date) return;
      c.day = d;
      if (!d.ok) $('q-count').textContent = 'This manifest does not match the index.';
      fillKinds();
    }, function (e) {
      if (c.my !== token) return;
      $('q-count').textContent = 'Manifest unreachable: ' + (e && e.message || e);
    });
  }
  $('q-day').addEventListener('change', function () { choose(this.value); });

  function fillKinds() {
    var d = cur.day, ks = {}, sel = $('q-kind'), keep = sel.value;
    d.parsed.forEach(function (p) { ks[p.kind] = (ks[p.kind] || 0) + 1; });
    var names = Object.keys(ks).sort();
    clear(sel);
    if (names.length > 1) { var all = el('option', null, 'All kinds · ' + plural(d.files.length, 'file')); all.value = '*'; sel.appendChild(all); }
    names.forEach(function (k) { var o = el('option', null, k + ' · ' + plural(ks[k], 'file')); o.value = k; sel.appendChild(o); });
    sel.value = ks[keep] || (keep === '*' && names.length > 1) ? keep : ks.bar ? 'bar' : ks.trade ? 'trade' : names[0];
    $('q-kind-wrap').hidden = names.length < 2;
    fillInstruments();
  }
  $('q-kind').addEventListener('change', fillInstruments);

  function inKind(p) { var k = $('q-kind').value; return k === '*' || p.kind === k; }

  function fillInstruments() {
    var d = cur.day, by = {}, keep = cur.inst;
    d.parsed.forEach(function (p) {
      if (!inKind(p) || !p.inst) return;
      var x = by[p.inst] || (by[p.inst] = { id: p.inst, seg: p.seg.instrument, bytes: 0, files: 0 });
      x.bytes += p.f.size || 0; x.files++;
    });
    cur.insts = Object.keys(by).sort().map(function (k) { return by[k]; });
    var n = d.parsed.filter(inKind).length;
    if (keep && by[keep]) cur.inst = keep;
    else if (n <= SMALL || !cur.insts.length) cur.inst = '';
    else cur.inst = cur.insts.reduce(function (a, b) { return b.bytes > a.bytes ? b : a; }).id;   // the busiest instrument
    renderInstruments();
    if (!sqlEdited) $('q-sql').value = PRESET[$('q-kind').value] || ANY;
    selectionChanged();
  }

  function renderInstruments() {
    var sel = $('q-inst'), q = $('q-find').value.trim().toLowerCase(), list = cur.insts;
    var shown = q ? list.filter(function (x) { return x.id.toLowerCase().indexOf(q) >= 0; }) : list;
    if (q && shown.length && !shown.some(function (x) { return x.id === cur.inst; })) cur.inst = shown[0].id;
    clear(sel);
    var all = el('option', null, 'All instruments'); all.value = ''; sel.appendChild(all);
    shown.forEach(function (x) { var o = el('option', null, x.id + ' · ' + size(x.bytes)); o.value = x.id; sel.appendChild(o); });
    if (cur.inst && !shown.some(function (x) { return x.id === cur.inst; })) {
      var o = el('option', null, cur.inst); o.value = cur.inst; sel.appendChild(o);   // keep the current pick visible
    }
    sel.value = cur.inst;
    sel.disabled = !list.length;
    $('q-find').disabled = list.length < 2;
  }
  $('q-find').addEventListener('input', function () { if (!cur || !cur.insts) return; var before = cur.inst; renderInstruments(); if (cur.inst !== before) selectionChanged(); else countNote(); });
  $('q-inst').addEventListener('change', function () { cur.inst = this.value; selectionChanged(); });

  // The files the query reads: the day's files of the chosen kind and instrument.
  function selection() {
    var d = cur.day, files = d.parsed.filter(function (p) { return inKind(p) && (!cur.inst || p.inst === cur.inst); });
    var pq = files.filter(function (p) { return p.pq; }), kinds = {}, insts = {};
    pq.forEach(function (p) { kinds[p.kind] = 1; if (p.inst) insts[p.inst] = 1; });
    return { date: d.date, files: files, pq: pq, kind: Object.keys(kinds).length === 1 ? Object.keys(kinds)[0] : null,
      inst: Object.keys(insts).length === 1 ? Object.keys(insts)[0] : null, bytes: pq.reduce(function (a, p) { return a + (p.f.size || 0); }, 0) };
  }

  function countNote() {
    var q = $('q-find').value.trim(), n = cur.insts.length, s = cur.sel;
    var shown = q ? cur.insts.filter(function (x) { return x.id.toLowerCase().indexOf(q.toLowerCase()) >= 0; }).length : n;
    var t = (q ? count(shown) + ' of ' : '') + plural(n, 'instrument') + ' · ';
    t += s.pq.length ? 'the query reads ' + plural(s.pq.length, 'file') + ', ' + size(s.bytes) : 'no Parquet files in this selection';
    $('q-count').textContent = t;
  }

  function selectionChanged() {
    cur.sel = selection();
    countNote();
    $('q-run').disabled = !cur.sel.pq.length;
    renderGet();
  }

  // ---- "Take it with you": file links, manifest, magnet, snippets, notices ----
  function renderGet() {
    var s = cur.sel, d = cur.day; if (!s) return;
    var links = $('get-links'); clear(links);
    var m = el('a', null, 'manifest.json'); m.href = cur.base + '_manifests/' + d.date + '/manifest.json'; links.appendChild(m);
    var an = el('a', null, 'anchors'); an.href = cur.base + '_manifests/' + d.date + '/manifest.anchors.json'; links.appendChild(an);
    if (cur.magnets[d.date]) { var mg = el('a', null, 'magnet'); mg.href = cur.magnets[d.date]; links.appendChild(mg); }
    var ul = $('get-files'); clear(ul);
    s.files.slice(0, LISTED).forEach(function (p) {
      var li = el('li'), a = el('a', 'mono', (p.inst || p.kind) + ' / ' + p.f.path.split('/').pop());
      a.href = fileUrl(p.f.path); a.setAttribute('download', '');
      a.title = p.f.path;
      li.appendChild(a); li.appendChild(el('span', 'mono', size(p.f.size || 0) + (p.f.record_count != null ? ' · ' + plural(p.f.record_count, 'record') : '')));
      ul.appendChild(li);
    });
    $('get-files-h').textContent = s.files.length > LISTED ? 'Files (first ' + LISTED + ' of ' + count(s.files.length) + ')' : plural(s.files.length, 'file');
    $('get-files-more').textContent = s.files.length > LISTED ? 'The manifest lists all ' + count(s.files.length) + ', with their SHA-256 and IPFS CIDs.' : 'Each file’s SHA-256 and IPFS CID are in the manifest.';
    snippets();
    var venues = {}; s.files.forEach(function (p) { if (p.seg.venue) venues[p.seg.venue] = 1; });
    var notes = (d.man.notices || []).filter(function (x) { return venues[x.source]; });
    var nl = $('get-notices'); clear(nl);
    notes.forEach(function (x) { var li = el('li'); li.appendChild(el('b', null, x.source + ' ')); li.appendChild(el('span', 'mono', x.admission)); li.appendChild(el('p', null, x.notice)); nl.appendChild(li); });
    $('get-notices-wrap').hidden = !notes.length;
    $('arch-get').hidden = false;
  }

  function snippets() {
    var s = cur.sel, d = cur.day; if (!s) return;
    var conds = [], k = $('q-kind').value;
    if (k !== '*' && k !== 'other') conds.push(s.pq.length && s.pq[0].seg.kind ? 'starts_with(f.path, ' + sq('kind=' + k + '/') + ')' : 'contains(f.path, ' + sq(k.replace(/^raw /, 'stream=') + '/') + ')');
    if (cur.inst) { var seg = (cur.insts.filter(function (x) { return x.id === cur.inst; })[0] || {}).seg; conds.push('contains(f.path, ' + sq('/instrument=' + seg + '/') + ')'); }
    conds.push("ends_with(f.path, '.parquet')");
    var setv = "SET VARIABLE files = (\n  SELECT list(" + sq(cur.base) + " || replace(f.path, '%', '%25'))\n" +
      "  FROM (SELECT unnest(manifest.files) AS f\n        FROM read_json(" + sq(cur.base + '_manifests/' + d.date + '/manifest.json') + "))\n" +
      "  WHERE " + conds.join('\n    AND ') + ");";
    var view = "CREATE VIEW day AS SELECT * FROM read_parquet(getvariable('files'), hive_partitioning = false, union_by_name = true);";
    var sql = $('q-sql').value.trim().replace(/;+\s*$/, '') || ANY;
    $('sn-sql').textContent = '-- duckdb 1.1 or later; reads ' + (cur.inst || 'every instrument') + ' on ' + d.date + '\n' + setv + '\n' + view + '\n\n' +
      sql + ';\n\nCOPY (' + sql + ') TO \'out.parquet\' (FORMAT parquet);\nCOPY (' + sql + ') TO \'out.csv\' (HEADER);\n';
    $('sn-py').textContent = '# pip install duckdb\nimport duckdb\n\ncon = duckdb.connect()\ncon.sql("""\n' + setv + '\n""")\ncon.sql("' + view.replace(/;$/, '') + '")\n\n' +
      'rel = con.sql("""\n' + sql + '\n""")\nprint(rel)\nrel.write_csv("out.csv")\nrel.write_parquet("out.parquet")\n';
  }
  $('q-sql').addEventListener('input', function () { sqlEdited = true; clearTimeout(this._t); this._t = setTimeout(snippets, 250); });
  $('sn-copy').addEventListener('click', function () {
    var pane = $('c-sn-sql').hidden ? $('sn-py') : $('sn-sql'), text = pane.textContent, note = $('sn-copied');
    function done(ok) { note.textContent = ok ? 'Copied.' : 'Select the snippet and copy it.'; setTimeout(function () { note.textContent = ''; }, 2500); }
    if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).then(function () { done(true); }, function () { done(false); });
    else done(false);
  });

  // ---- DuckDB-WASM ----
  function engine() {
    if (conn) return Promise.resolve(conn);
    if (loading) return loading;
    loading = import(DUCKDB).then(function (duckdb) {
      return duckdb.selectBundle(duckdb.getJsDelivrBundles()).then(function (b) {
        var url = URL.createObjectURL(new Blob(['importScripts("' + b.mainWorker + '");'], { type: 'text/javascript' }));
        db = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(duckdb.LogLevel.WARNING), new Worker(url));
        return db.instantiate(b.mainModule, b.pthreadWorker).then(function () { URL.revokeObjectURL(url); return db.connect(); });
      });
    }).then(function (c) { conn = c; return c; }, function (e) { loading = null; throw e; });
    return loading;
  }

  // Arrow hands DATE and TIMESTAMP columns over as epoch milliseconds (type ids 8 and 10).
  function cell(v, typeId) {
    if (v == null) return '';
    if (typeof v === 'number' && (typeId === 8 || typeId === 10)) {
      var iso = new Date(v).toISOString();
      return typeId === 8 ? iso.slice(0, 10) : iso.replace('T', ' ').replace('.000Z', 'Z');
    }
    if (typeof v === 'bigint') return v.toString();
    if (v instanceof Date) return v.toISOString();
    if (typeof v === 'object') { try { return JSON.stringify(v, function (k, x) { return typeof x === 'bigint' ? x.toString() : x; }); } catch (e) { return String(v); } }
    return String(v);
  }

  // The query's full result goes into a table in the engine; the page draws only the first
  // SHOW rows, and the exports copy the whole table.
  function materialize(c, sql) {
    // DESCRIBE, SUMMARIZE, SHOW and the like are not plain SELECTs: wrap them.
    var plain = /^\s*(select|with|from|values|table|\()/i.test(sql);
    return c.query('CREATE OR REPLACE TEMP TABLE last_result AS ' + (plain ? sql : 'SELECT * FROM (' + sql + ')'));
  }

  $('q-run').addEventListener('click', function () {
    var btn = this, out = $('q-out'), status = $('q-status'), s = cur && cur.sel, c0 = cur;
    if (!s || !s.pq.length) return;
    var urls = s.pq.map(function (p) { return sq(fileUrl(p.f.path)); }).join(', ');
    var sql = $('q-sql').value.trim().replace(/;+\s*$/, '') || ANY;
    btn.disabled = true; clear(out); $('q-export').hidden = true; $('q-chart').hidden = true; result = null; chartData = null; $('q-export-status').textContent = '';
    status.textContent = conn ? 'Running…' : 'Loading DuckDB…';
    var t0 = performance.now(), c, total;
    engine().then(function (cc) {
      c = cc; status.textContent = 'Reading ' + plural(s.pq.length, 'file') + '…';
      return c.query('CREATE OR REPLACE VIEW day AS SELECT * FROM read_parquet([' + urls + '], hive_partitioning = false, union_by_name = true)');
    }).then(function () { return materialize(c, sql); })
      .then(function () { return c.query('SELECT count(*)::DOUBLE AS n FROM last_result'); })
      .then(function (r) { total = r.toArray()[0].n; return c.query('SELECT * FROM last_result LIMIT ' + SHOW); })
      .then(function (res) {
        if (c0 !== cur) return;
        var fields = res.schema.fields, rows = res.toArray();
        var table = el('table'), head = el('tr'), cap = el('caption', 'sr', 'Query result, ' + (total > rows.length ? 'first ' + rows.length + ' of ' + count(total) + ' rows' : plural(total, 'row')));
        table.appendChild(cap);
        fields.forEach(function (f) { var th = el('th', null, f.name); th.scope = 'col'; head.appendChild(th); });
        var thead = el('thead'); thead.appendChild(head); table.appendChild(thead);
        var body = el('tbody');
        rows.forEach(function (r) { var tr = el('tr'); fields.forEach(function (f) { var v = cell(r[f.name], f.type && f.type.typeId); var td = el('td', null, v); td.title = v; tr.appendChild(td); }); body.appendChild(tr); });
        table.appendChild(body); out.appendChild(table);
        status.textContent = plural(total, 'row') + (total > rows.length ? ', first ' + SHOW + ' shown; downloads include all' : '') + ' · ' + Math.round(performance.now() - t0) + ' ms';
        result = { name: [cur.a.name, s.date, s.inst ? s.pq[0].seg.instrument : $('q-kind').value === '*' ? 'all' : $('q-kind').value].join('_').replace(/[^A-Za-z0-9._-]+/g, '-'), rows: total };
        $('q-csv').textContent = 'Download CSV'; $('q-parquet').textContent = 'Download Parquet';
        $('q-export').hidden = false;
        if (s.inst && CHART[s.kind]) return chart(c, s);
      }).catch(function (e) {
        out.appendChild(el('div', 'err', String(e && e.message || e)));
        status.textContent = 'Query failed';
      }).then(function () { btn.disabled = !(cur && cur.sel && cur.sel.pq.length); });
  });

  // ---- exports: COPY the full result inside the engine, then hand the bytes to the browser ----
  function exportAs(fmt) {
    if (!result || !conn) return;
    var r = result, tmp = 'bleh-export.' + fmt, note = $('q-export-status'), b1 = $('q-csv'), b2 = $('q-parquet');
    b1.disabled = b2.disabled = true; note.textContent = 'Writing ' + fmt.toUpperCase() + ' of ' + plural(r.rows, 'row') + '…';
    db.registerEmptyFileBuffer(tmp).then(function () {
      return conn.query('COPY last_result TO ' + sq(tmp) + (fmt === 'csv' ? ' (FORMAT csv, HEADER)' : ' (FORMAT parquet, COMPRESSION zstd)'));
    }).then(function () { return db.copyFileToBuffer(tmp); })
      .then(function (buf) {
        db.dropFile(tmp).catch(function () {});
        var blob = new Blob([buf], { type: fmt === 'csv' ? 'text/csv' : 'application/vnd.apache.parquet' });
        var a = el('a'); a.href = URL.createObjectURL(blob); a.download = r.name + '.' + fmt;
        document.body.appendChild(a); a.click(); a.remove();
        setTimeout(function () { URL.revokeObjectURL(a.href); }, 60000);
        note.textContent = a.download + ', ' + size(blob.size) + '. Keep any source notices with it.';
      }).catch(function (e) { note.textContent = 'Export failed: ' + (e && e.message || e); })
      .then(function () { b1.disabled = b2.disabled = false; });
  }
  $('q-csv').addEventListener('click', function () { exportAs('csv'); });
  $('q-parquet').addEventListener('click', function () { exportAs('parquet'); });

  // ---- chart: price over time for one instrument, at most about 600 points ----
  function chart(c, s) {
    var my = token;
    var k = CHART[s.kind], lo = k.lo || k.y, hi = k.hi || k.y;
    var q = 'WITH p AS (SELECT (' + k.x + ')::DOUBLE AS t, (' + k.y + ')::DOUBLE AS v, (' + lo + ')::DOUBLE AS lo, (' + hi + ')::DOUBLE AS hi' +
      (k.by === 't' ? '' : ', ' + k.by + ' AS o') + ' FROM day' + (k.where ? ' WHERE ' + k.where : '') + '),\n' +
      'q AS (SELECT * FROM p WHERE t IS NOT NULL AND v IS NOT NULL),\n' +
      's AS (SELECT min(t) AS a, max(t) AS b FROM q)\n' +
      'SELECT floor((t - s.a) / greatest((s.b - s.a) / 600, 1)) AS k, min(t) AS t, min(lo) AS lo, max(hi) AS hi, arg_max(v, ' + (k.by === 't' ? 't' : 'o') + ') AS v, count(*)::DOUBLE AS n\n' +
      'FROM q, s GROUP BY k ORDER BY k';
    return c.query(q).then(function (res) {
      if (my !== token) return;
      var pts = res.toArray().map(function (r) { return { t: r.t, v: r.v, lo: r.lo, hi: r.hi, n: r.n }; });
      if (!pts.length) return;
      var n = pts.reduce(function (a, p) { return a + p.n; }, 0);
      chartData = { pts: pts, what: k.what, inst: s.inst, date: s.date, n: n, kind: s.kind };
      $('q-chart').hidden = false;
      drawChart();
    }).catch(function (e) {
      $('q-chart').hidden = false; clear($('q-plot'));
      $('q-chart-cap').textContent = 'No chart for this selection: ' + (e && e.message || e);
      $('q-chart-read').textContent = '';
    });
  }

  function fmtV(v, span) {
    var dp = Math.max(0, Math.min(8, 3 - Math.floor(Math.log10(Math.abs(span) || Math.abs(v) || 1))));
    return v.toLocaleString('en-US', { minimumFractionDigits: dp, maximumFractionDigits: dp });
  }
  function fmtT(t, step) {
    var iso = new Date(t).toISOString();
    return step >= 86400000 ? iso.slice(0, 10) : step >= 60000 ? iso.slice(11, 16) : iso.slice(11, 19);
  }
  // Time ticks at round UTC steps (1 s to 7 days), at most n of them.
  function timeTicks(a, b, n) {
    var S = [1, 5, 15, 30, 60, 300, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400, 172800, 604800], step = S[S.length - 1] * 1000;
    for (var i = 0; i < S.length; i++) if ((b - a) / (S[i] * 1000) <= n - 1) { step = S[i] * 1000; break; }
    var out = []; for (var t = Math.ceil(a / step) * step; t <= b; t += step) out.push(t);
    return { ticks: out, step: step };
  }
  function niceTicks(a, b, n) {
    var span = b - a || Math.abs(a) || 1, step = Math.pow(10, Math.floor(Math.log10(span / n))), err = span / n / step;
    step *= err >= 7.5 ? 10 : err >= 3.5 ? 5 : err >= 1.5 ? 2 : 1;
    var out = []; for (var v = Math.ceil(a / step) * step; v <= b + step * 1e-9; v += step) out.push(v);
    return out;
  }

  function drawChart() {
    var d = chartData, plot = $('q-plot'); if (!d) return;
    clear(plot);
    var unit = d.kind === 'trade' ? 'trades' : d.kind === 'bar' ? 'bars' : 'values';
    $('q-chart-cap').textContent = d.what + ' · ' + d.inst + ' · ' + d.date + ' UTC · ' + count(d.n) + ' ' + unit + (d.pts.length > 1 && d.pts.length < d.n ? ' in ' + d.pts.length + ' points, each the last in its interval' + (d.kind !== 'reference_value' ? ', band from low to high' : '') : '');
    var p = d.pts, read = $('q-chart-read');
    if (p.length < 2) {
      var only = p[0];
      plot.appendChild(el('p', 'arch-one', fmtV(only.v, 0)));
      read.textContent = 'One point at ' + new Date(only.t).toISOString().replace('T', ' ').replace('.000Z', ' UTC') + (only.n > 1 ? ' (latest of ' + only.n + ')' : '');
      return;
    }
    var W = Math.max(240, Math.round(plot.clientWidth || 600)), H = W < 480 ? 200 : 240, L = 8, R = 8, T = 10, B = 22;
    var vmin = Infinity, vmax = -Infinity;
    p.forEach(function (x) { vmin = Math.min(vmin, x.lo, x.v); vmax = Math.max(vmax, x.hi, x.v); });
    if (vmin === vmax) { vmin -= Math.abs(vmin) * 0.01 || 1; vmax += Math.abs(vmax) * 0.01 || 1; }
    var ticks = niceTicks(vmin, vmax, H < 220 ? 3 : 4), span = vmax - vmin;
    var labels = ticks.map(function (v) { return fmtV(v, span); });
    L = 14 + Math.max.apply(null, labels.map(function (s) { return s.length; })) * 6.6;
    var t0 = p[0].t, t1 = p[p.length - 1].t, tspan = t1 - t0 || 1;
    function X(t) { return L + (t - t0) / tspan * (W - L - R); }
    function Y(v) { return T + (vmax - v) / (vmax - vmin) * (H - T - B); }
    var ns = 'http://www.w3.org/2000/svg';
    function svgEl(tag, attrs) { var e = document.createElementNS(ns, tag); for (var a in attrs) e.setAttribute(a, attrs[a]); return e; }
    var svg = svgEl('svg', { viewBox: '0 0 ' + W + ' ' + H, width: W, height: H, role: 'img', 'class': 'arch-svg' });
    var first = p[0], last = p[p.length - 1];
    svg.setAttribute('aria-label', d.what + ' of ' + d.inst + ' on ' + d.date + ': from ' + fmtV(first.v, span) + ' to ' + fmtV(last.v, span) +
      ', low ' + fmtV(vmin, span) + ', high ' + fmtV(vmax, span) + '. The rows are in the table below.');
    ticks.forEach(function (v, i) {
      var y = Y(v); if (y < T - 1 || y > H - B + 1) return;
      svg.appendChild(svgEl('line', { x1: L, x2: W - R, y1: y, y2: y, 'class': 'grid' }));
      var tx = svgEl('text', { x: L - 6, y: y + 4, 'text-anchor': 'end' }); tx.textContent = labels[i]; svg.appendChild(tx);
    });
    var tt = timeTicks(t0, t1, W < 480 ? 4 : 7);
    tt.ticks.forEach(function (t) {
      var x = X(t), tx2 = svgEl('text', { x: x, y: H - 6, 'text-anchor': x < L + 20 ? 'start' : x > W - R - 20 ? 'end' : 'middle' });
      tx2.textContent = fmtT(t, tt.step); svg.appendChild(tx2);
      svg.appendChild(svgEl('line', { x1: x, x2: x, y1: H - B, y2: H - B + 4, 'class': 'grid' }));
    });
    if (p.some(function (x) { return x.hi > x.lo; })) {
      var band = p.map(function (x) { return X(x.t).toFixed(1) + ',' + Y(x.hi).toFixed(1); }).concat(p.slice().reverse().map(function (x) { return X(x.t).toFixed(1) + ',' + Y(x.lo).toFixed(1); }));
      svg.appendChild(svgEl('polygon', { points: band.join(' '), 'class': 'band' }));
    }
    svg.appendChild(svgEl('polyline', { points: p.map(function (x) { return X(x.t).toFixed(1) + ',' + Y(x.v).toFixed(1); }).join(' '), 'class': 'line' }));
    var guide = svgEl('line', { y1: T, y2: H - B, 'class': 'guide', visibility: 'hidden' }), dot = svgEl('circle', { r: 3.5, 'class': 'dot', visibility: 'hidden' });
    svg.appendChild(guide); svg.appendChild(dot);
    function show(x) {
      guide.setAttribute('x1', X(x.t)); guide.setAttribute('x2', X(x.t)); dot.setAttribute('cx', X(x.t)); dot.setAttribute('cy', Y(x.v));
      guide.setAttribute('visibility', 'visible'); dot.setAttribute('visibility', 'visible');
      read.textContent = new Date(x.t).toISOString().replace('T', ' ').replace(/\.\d+Z$/, ' UTC') + ' · ' + fmtV(x.v, span) + (x.hi > x.lo ? ' (low ' + fmtV(x.lo, span) + ', high ' + fmtV(x.hi, span) + ')' : '');
    }
    function idle() { guide.setAttribute('visibility', 'hidden'); dot.setAttribute('visibility', 'hidden'); read.textContent = 'Last ' + fmtV(last.v, span) + ' · low ' + fmtV(vmin, span) + ' · high ' + fmtV(vmax, span); }
    svg.addEventListener('pointermove', function (e) {
      var r = svg.getBoundingClientRect(), t = t0 + ((e.clientX - r.left) * W / r.width - L) / (W - L - R) * tspan, lo2 = 0, hi2 = p.length - 1;
      while (hi2 - lo2 > 1) { var mid = (lo2 + hi2) >> 1; if (p[mid].t < t) lo2 = mid; else hi2 = mid; }
      show(Math.abs(p[lo2].t - t) < Math.abs(p[hi2].t - t) ? p[lo2] : p[hi2]);
    });
    svg.addEventListener('pointerleave', idle);
    plot.appendChild(svg); idle();
  }
  var rw; addEventListener('resize', function () { clearTimeout(rw); rw = setTimeout(function () { if (chartData && !$('q-chart').hidden) drawChart(); }, 150); });
})();
