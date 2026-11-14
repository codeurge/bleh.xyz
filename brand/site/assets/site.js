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
      ['Class III milk', 'usda-dairy', 'usda-dairy:ref:FMMO.class_3_Price', '$/cwt', 2]
    ];
    var NAMES = { 'nyfed': 'Federal Reserve Bank of New York', 'frb-h15': 'Federal Reserve H.15', 'frb-h10': 'Federal Reserve H.10',
      'us-treasury': 'U.S. Treasury', 'usda-dairy': 'USDA dairy', 'usda-lmr': 'USDA livestock', 'cftc-cot': 'CFTC Commitments of Traders', 'ercot': 'ERCOT',
      'cftc-part43': 'CFTC Part 43 swaps', 'sec-sbsr': 'SEC security-based swaps' };
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
        if (p[3]) v.appendChild(el('small', null, ' ' + p[3]));
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

// open archive: lists the signed days of the public archive, re-checks the hash chain in the
// browser (each manifest's SHA-256 against the index and its previous_manifest_sha256), and
// queries a day's Parquet files with DuckDB-WASM, loaded from jsDelivr only when asked.
// docs/specs/manifest-v0.md, docs/specs/archive-layout-v0.md § 4.
(function () {
  var sec = document.getElementById('archive'); if (!sec || !window.fetch || !window.crypto || !crypto.subtle) return;
  var BASE = 'https://data.bleh.xyz/reference-normalized/';
  var DUCKDB = 'https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.29.0/+esm';
  var $ = function (id) { return document.getElementById(id); };
  var days = [], conn = null, loading = null;

  function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
  function hex(buf) { return Array.prototype.map.call(new Uint8Array(buf), function (b) { return ('0' + b.toString(16)).slice(-2); }).join(''); }
  function size(n) { return n < 1024 ? n + ' B' : n < 1048576 ? (n / 1024).toFixed(1) + ' KB' : n < 1073741824 ? (n / 1048576).toFixed(1) + ' MB' : (n / 1073741824).toFixed(2) + ' GB'; }
  function get(path, kind) { return fetch(BASE + path, { mode: 'cors', credentials: 'omit' }).then(function (r) { if (!r.ok) throw new Error(path + ' ' + r.status); return kind === 'buf' ? r.arrayBuffer() : r.text(); }); }
  function stat(value, label, cls) { var d = el('div', 'arch-stat' + (cls ? ' ' + cls : '')); d.appendChild(el('b', null, value)); d.appendChild(el('span', null, label)); return d; }

  get('_manifests/index.json').then(function (t) {
    var idx = JSON.parse(t).manifests || [];
    return Promise.all([
      Promise.all(idx.map(function (e) {
        return get('_manifests/' + e.date + '/manifest.json', 'buf').then(function (buf) {
          return crypto.subtle.digest('SHA-256', buf).then(function (h) {
            return { entry: e, sha: hex(h), doc: JSON.parse(new TextDecoder().decode(buf)) };
          });
        });
      })),
      get('_manifests/magnets.txt').catch(function () { return ''; })
    ]);
  }).then(function (res) {
    var list = res[0], magnets = {};
    res[1].split('\n').forEach(function (line) { var i = line.indexOf(' '); if (i > 0) magnets[line.slice(0, i)] = line.slice(i + 1).trim(); });
    var broken = null, prev = null, files = 0, records = 0, bytes = 0;
    list.forEach(function (m) {
      var man = m.doc.manifest || {};
      if (m.sha !== m.entry.sha256 || (prev && man.previous_manifest_sha256 !== prev)) broken = broken || m.entry.date;
      prev = m.sha;
      var f = man.files || [], rc = 0, sz = 0;
      f.forEach(function (x) { rc += x.record_count || 0; sz += x.size || 0; });
      files += f.length; records += rc; bytes += sz;
      days.push({ date: man.date, files: f, records: rc, bytes: sz, signer: man.signer, magnet: magnets[man.date] });
    });
    if (!days.length) return;
    var latest = days[days.length - 1];

    var st = $('arch-stats');
    st.appendChild(stat(days.length.toLocaleString('en-US'), 'Days published'));
    st.appendChild(stat(records.toLocaleString('en-US'), 'Records'));
    st.appendChild(stat(size(bytes), files + ' Parquet files'));
    st.appendChild(stat(latest.date, 'Latest day'));
    st.appendChild(broken ? stat('Broken', 'Hash chain at ' + broken, 'bad') : stat('✓ Intact', 'Hash chain, checked here', 'ok'));

    $('arch-index').href = BASE + '_manifests/index.json';
    $('arch-signer').textContent = latest.signer || 'none stated';
    $('arch-cmd-date').textContent = latest.date;

    var rows = $('arch-rows'), pick = $('q-day');
    days.slice().reverse().forEach(function (d) {
      var tr = el('tr'); tr.setAttribute('data-date', d.date);
      var q = el('button', null, d.date); q.type = 'button'; q.setAttribute('aria-label', 'Query ' + d.date);
      q.addEventListener('click', function () { choose(d.date); $('q-sql').focus(); });
      var td = el('td'); td.appendChild(q); tr.appendChild(td);
      tr.appendChild(el('td', null, d.files.length));
      tr.appendChild(el('td', null, d.records.toLocaleString('en-US')));
      tr.appendChild(el('td', null, size(d.bytes)));
      var links = el('td'), wrap = el('span', 'arch-links');
      var a = el('a', null, 'manifest'); a.href = BASE + '_manifests/' + d.date + '/manifest.json'; wrap.appendChild(a);
      if (d.magnet) { var mg = el('a', null, 'magnet'); mg.href = d.magnet; wrap.appendChild(mg); }
      links.appendChild(wrap); tr.appendChild(links); rows.appendChild(tr);
      var o = el('option', null, d.date + ' · ' + d.files.length + ' files'); o.value = d.date; pick.appendChild(o);
    });
    pick.addEventListener('change', function () { choose(pick.value); });
    choose(latest.date);
    sec.hidden = false;
  }).catch(function () { /* archive unreachable: keep the section hidden */ });

  function choose(date) {
    $('q-day').value = date;
    [].forEach.call(document.querySelectorAll('#arch-rows tr'), function (tr) { tr.classList.toggle('sel', tr.getAttribute('data-date') === date); });
  }

  function engine() {
    if (conn) return Promise.resolve(conn);
    if (loading) return loading;
    loading = import(DUCKDB).then(function (duckdb) {
      var bundle;
      return duckdb.selectBundle(duckdb.getJsDelivrBundles()).then(function (b) {
        bundle = b;
        var url = URL.createObjectURL(new Blob(['importScripts("' + b.mainWorker + '");'], { type: 'text/javascript' }));
        var db = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(duckdb.LogLevel.WARNING), new Worker(url));
        return db.instantiate(bundle.mainModule, bundle.pthreadWorker).then(function () { URL.revokeObjectURL(url); return db.connect(); });
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

  $('q-run').addEventListener('click', function () {
    var btn = this, out = $('q-out'), status = $('q-status');
    var day = days.filter(function (d) { return d.date === $('q-day').value; })[0];
    if (!day) return;
    var urls = day.files.map(function (f) { return "'" + (BASE + f.path).replace(/'/g, "''") + "'"; }).join(', ');
    btn.disabled = true; out.textContent = '';
    status.textContent = conn ? 'Running…' : 'Loading DuckDB…';
    var t0 = performance.now();
    engine().then(function (c) {
      status.textContent = 'Running…';
      return c.query('CREATE OR REPLACE VIEW day AS SELECT * FROM read_parquet([' + urls + '], hive_partitioning = true, union_by_name = true)')
        .then(function () { return c.query($('q-sql').value); });
    }).then(function (res) {
      var fields = res.schema.fields, names = fields.map(function (f) { return f.name; }), all = res.toArray(), shown = all.slice(0, 200);
      var table = el('table'), head = el('tr');
      names.forEach(function (n) { head.appendChild(el('th', null, n)); });
      var thead = el('thead'); thead.appendChild(head); table.appendChild(thead);
      var body = el('tbody');
      shown.forEach(function (r) { var tr = el('tr'); fields.forEach(function (f) { var v = cell(r[f.name], f.type && f.type.typeId); var td = el('td', null, v); td.title = v; tr.appendChild(td); }); body.appendChild(tr); });
      table.appendChild(body); out.appendChild(table);
      status.textContent = all.length.toLocaleString('en-US') + ' rows' + (all.length > shown.length ? ', first 200 shown' : '') + ' · ' + Math.round(performance.now() - t0) + ' ms';
    }).catch(function (e) {
      out.appendChild(el('div', 'err', String(e && e.message || e)));
      status.textContent = 'Query failed';
    }).then(function () { btn.disabled = false; });
  });
})();
