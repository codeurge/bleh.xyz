/* bleh.xyz site script. No network requests. Everything here is progressive enhancement. */
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
