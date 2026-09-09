/* ==========================================================================
   Living Docs — shared behaviour + zero-dependency SVG charting
   No CDN, no build step. Works over file:// and http://.
   ========================================================================== */
(function () {
  'use strict';

  /* ---------------- SVG helpers ---------------- */
  var NS = 'http://www.w3.org/2000/svg';
  function el(name, attrs, text) {
    var n = document.createElementNS(NS, name);
    for (var k in attrs) if (attrs[k] !== null && attrs[k] !== undefined) n.setAttribute(k, attrs[k]);
    if (text !== undefined) n.textContent = text;
    return n;
  }
  function css(v, fallback) {
    var c = getComputedStyle(document.documentElement).getPropertyValue(v).trim();
    return c || fallback;
  }
  function palette() {
    return ['--c1','--c2','--c3','--c4','--c5','--c6','--c7','--c8'].map(function (v) { return css(v, '#888'); });
  }
  function tone() {
    return { ink: css('--ink','#111'), ink2: css('--ink-2','#333'), muted: css('--muted','#777'),
             faint: css('--faint','#999'), border: css('--border','#ddd'), surface: css('--surface','#fff') };
  }
  function fmt(n, d) {
    if (d === undefined) d = 0;
    return n.toLocaleString('en-US', { minimumFractionDigits: d, maximumFractionDigits: d });
  }
  function svgRoot(w, h) {
    var s = el('svg', { viewBox: '0 0 ' + w + ' ' + h, width: '100%', role: 'img',
                        'font-family': css('--sans', 'sans-serif') });
    return s;
  }
  function niceMax(v) {
    if (v <= 0) return 1;
    var mag = Math.pow(10, Math.floor(Math.log10(v)));
    var r = v / mag;
    var step = r <= 1 ? 1 : r <= 2 ? 2 : r <= 2.5 ? 2.5 : r <= 5 ? 5 : 10;
    return step * mag;
  }

  /* ---------------- Charts ---------------- */
  var KD = {};

  /* Horizontal bars: [{label, value, color?, note?}] */
  KD.hbar = function (spec) {
    var d = spec.data, t = tone(), pal = palette();
    var gut = spec.labelWidth || 190, pad = 10, rowH = spec.rowH || 34, barH = 17;
    var w = 760, h = pad * 2 + d.length * rowH + 4;
    var max = spec.max || niceMax(Math.max.apply(null, d.map(function (x) { return x.value; })));
    var plot = w - gut - 92;
    var s = svgRoot(w, h);
    d.forEach(function (row, i) {
      var y = pad + i * rowH;
      s.appendChild(el('text', { x: gut - 12, y: y + barH / 2 + 4.5, 'text-anchor': 'end',
        'font-size': 12.6, fill: t.ink2, 'font-weight': 500 }, row.label));
      s.appendChild(el('rect', { x: gut, y: y, width: plot, height: barH, rx: 4, fill: t.border, opacity: .5 }));
      var bw = Math.max(2, plot * (row.value / max));
      var c = row.color ? css(row.color, row.color) : pal[i % pal.length];
      var r = el('rect', { x: gut, y: y, width: 0, height: barH, rx: 4, fill: c });
      r.appendChild(el('animate', { attributeName: 'width', from: 0, to: bw, dur: '.55s', fill: 'freeze' }));
      s.appendChild(r);
      s.appendChild(el('text', { x: gut + bw + 9, y: y + barH / 2 + 4.5, 'font-size': 12,
        fill: t.ink, 'font-weight': 600, 'font-family': css('--mono','monospace') },
        (spec.prefix || '') + fmt(row.value, spec.dp) + (spec.suffix || '')));
      if (row.note) s.appendChild(el('text', { x: gut, y: y + barH + 12.5, 'font-size': 10.8, fill: t.faint }, row.note));
    });
    return s;
  };

  /* Vertical grouped columns: {categories:[], series:[{name, values:[], color?}]} */
  KD.column = function (spec) {
    var t = tone(), pal = palette();
    var w = 760, h = spec.height || 300, L = 56, R = 14, T = spec.series.length > 1 ? 34 : 14, B = 44;
    var pw = w - L - R, ph = h - T - B;
    var all = [];
    spec.series.forEach(function (s) { all = all.concat(s.values); });
    var max = spec.max || niceMax(Math.max.apply(null, all) * 1.1);
    var s = svgRoot(w, h);
    for (var i = 0; i <= 4; i++) {
      var y = T + ph - (ph * i / 4);
      s.appendChild(el('line', { x1: L, y1: y, x2: L + pw, y2: y, stroke: t.border, 'stroke-width': 1 }));
      s.appendChild(el('text', { x: L - 10, y: y + 4, 'text-anchor': 'end', 'font-size': 11, fill: t.faint,
        'font-family': css('--mono','monospace') }, (spec.prefix || '') + fmt(max * i / 4, spec.dp)));
    }
    var n = spec.categories.length, slot = pw / n, ns = spec.series.length;
    var bw = Math.min(spec.barWidth || 44, (slot * .68) / ns);
    spec.categories.forEach(function (cat, ci) {
      var cx = L + slot * ci + slot / 2;
      s.appendChild(el('text', { x: cx, y: T + ph + 20, 'text-anchor': 'middle', 'font-size': 11.6, fill: t.muted }, cat));
      spec.series.forEach(function (se, si) {
        var v = se.values[ci], bh = ph * (v / max);
        var x = cx - (ns * bw) / 2 + si * bw + 1;
        var c = se.color ? css(se.color, se.color) : pal[si % pal.length];
        var r = el('rect', { x: x, y: T + ph, width: bw - 2, height: 0, rx: 3, fill: c });
        r.appendChild(el('animate', { attributeName: 'height', from: 0, to: bh, dur: '.6s', fill: 'freeze' }));
        r.appendChild(el('animate', { attributeName: 'y', from: T + ph, to: T + ph - bh, dur: '.6s', fill: 'freeze' }));
        s.appendChild(r);
        if (spec.valueLabels !== false)
          s.appendChild(el('text', { x: x + (bw - 2) / 2, y: T + ph - bh - 6, 'text-anchor': 'middle',
            'font-size': 10.6, fill: t.ink2, 'font-weight': 600, 'font-family': css('--mono','monospace') },
            (spec.prefix || '') + fmt(v, spec.dp)));
      });
    });
    if (ns > 1) legend(s, spec.series, L, 12, pal);
    return s;
  };

  /* Multi-series line: {labels:[], series:[{name, values:[], color?}]} */
  KD.line = function (spec) {
    var t = tone(), pal = palette();
    var w = 760, h = spec.height || 290, L = 58, R = 16, T = 32, B = 40;
    var pw = w - L - R, ph = h - T - B;
    var all = []; spec.series.forEach(function (s) { all = all.concat(s.values); });
    var min = spec.min !== undefined ? spec.min : Math.min(0, Math.min.apply(null, all));
    var max = spec.max || niceMax(Math.max.apply(null, all) * 1.08);
    var s = svgRoot(w, h);
    for (var i = 0; i <= 4; i++) {
      var y = T + ph - (ph * i / 4), val = min + (max - min) * i / 4;
      s.appendChild(el('line', { x1: L, y1: y, x2: L + pw, y2: y, stroke: t.border }));
      s.appendChild(el('text', { x: L - 10, y: y + 4, 'text-anchor': 'end', 'font-size': 11, fill: t.faint,
        'font-family': css('--mono','monospace') }, (spec.prefix || '') + fmt(val, spec.dp)));
    }
    var X = function (i) { return L + (spec.labels.length === 1 ? pw / 2 : pw * i / (spec.labels.length - 1)); };
    var Y = function (v) { return T + ph - ph * ((v - min) / (max - min)); };
    spec.labels.forEach(function (lb, i) {
      if (spec.labels.length > 14 && i % 2) return;
      s.appendChild(el('text', { x: X(i), y: T + ph + 19, 'text-anchor': 'middle', 'font-size': 11, fill: t.muted }, lb));
    });
    if (min < 0) s.appendChild(el('line', { x1: L, y1: Y(0), x2: L + pw, y2: Y(0), stroke: t.muted, 'stroke-dasharray': '3 3' }));
    spec.series.forEach(function (se, si) {
      var c = se.color ? css(se.color, se.color) : pal[si % pal.length];
      var pts = se.values.map(function (v, i) { return X(i) + ',' + Y(v); }).join(' ');
      if (se.area) {
        s.appendChild(el('polygon', { points: L + ',' + Y(min) + ' ' + pts + ' ' + (L + pw) + ',' + Y(min),
          fill: c, opacity: .1 }));
      }
      var p = el('polyline', { points: pts, fill: 'none', stroke: c, 'stroke-width': 2.4,
        'stroke-linejoin': 'round', 'stroke-linecap': 'round' });
      s.appendChild(p);
      se.values.forEach(function (v, i) {
        if (se.values.length > 16 && i % 2) return;
        s.appendChild(el('circle', { cx: X(i), cy: Y(v), r: 3, fill: t.surface, stroke: c, 'stroke-width': 2 }));
      });
    });
    legend(s, spec.series, L, 12, pal);
    return s;
  };

  /* Donut: [{label, value, color?}] */
  KD.donut = function (spec) {
    var d = spec.data, t = tone(), pal = palette();
    var w = 760, h = spec.height || 260, cx = 130, cy = h / 2, R = 92, r = 58;
    var total = d.reduce(function (a, b) { return a + b.value; }, 0);
    var s = svgRoot(w, h), ang = -Math.PI / 2;
    d.forEach(function (seg, i) {
      var sweep = (seg.value / total) * Math.PI * 2, end = ang + sweep;
      var large = sweep > Math.PI ? 1 : 0;
      var p = ['M', cx + R * Math.cos(ang), cy + R * Math.sin(ang),
               'A', R, R, 0, large, 1, cx + R * Math.cos(end), cy + R * Math.sin(end),
               'L', cx + r * Math.cos(end), cy + r * Math.sin(end),
               'A', r, r, 0, large, 0, cx + r * Math.cos(ang), cy + r * Math.sin(ang), 'Z'].join(' ');
      var c = seg.color ? css(seg.color, seg.color) : pal[i % pal.length];
      s.appendChild(el('path', { d: p, fill: c, stroke: t.surface, 'stroke-width': 2 }));
      ang = end;
    });
    if (spec.centerLabel) {
      s.appendChild(el('text', { x: cx, y: cy - 2, 'text-anchor': 'middle', 'font-size': 22, 'font-weight': 680,
        fill: t.ink, 'letter-spacing': '-.03em' }, spec.centerLabel));
      s.appendChild(el('text', { x: cx, y: cy + 17, 'text-anchor': 'middle', 'font-size': 11, fill: t.muted }, spec.centerSub || ''));
    }
    var ly = cy - (d.length * 26) / 2 + 10;
    d.forEach(function (seg, i) {
      var c = seg.color ? css(seg.color, seg.color) : pal[i % pal.length];
      s.appendChild(el('rect', { x: 268, y: ly - 9, width: 11, height: 11, rx: 3, fill: c }));
      s.appendChild(el('text', { x: 287, y: ly, 'font-size': 13, fill: t.ink2 }, seg.label));
      s.appendChild(el('text', { x: 740, y: ly, 'text-anchor': 'end', 'font-size': 12.4, fill: t.ink,
        'font-weight': 600, 'font-family': css('--mono','monospace') },
        (spec.prefix || '') + fmt(seg.value, spec.dp) + '  ·  ' + Math.round(seg.value / total * 100) + '%'));
      ly += 26;
    });
    return s;
  };


  /* Funnel: {data:[{label,value,note}], prefix, power} — widths use a power scale
     so three orders of magnitude stay legible; the numbers carry the real magnitude. */
  KD.funnel = function (spec) {
    var d = spec.data, t = tone(), pal = palette();
    var pw = 486, cx = 496, gut = 244, rowH = spec.rowH || 58, pad = 8;
    var w = 760, h = pad * 2 + d.length * rowH;
    var max = d[0].value, pow = spec.power || 0.3;
    var wid = function (v) { return Math.max(26, pw * Math.pow(v / max, pow)); };
    var s = svgRoot(w, h);
    d.forEach(function (row, i) {
      var y = pad + i * rowH, w1 = wid(row.value);
      var w2 = i < d.length - 1 ? wid(d[i + 1].value) : w1 * 0.82;
      var c = row.color ? css(row.color, row.color) : pal[i % pal.length];
      s.appendChild(el('polygon', {
        points: [cx - w1/2, y, cx + w1/2, y, cx + w2/2, y + rowH - 6, cx - w2/2, y + rowH - 6].join(' '),
        fill: c, opacity: row.dim ? .3 : .82
      }));
      s.appendChild(el('text', { x: gut - 14, y: y + rowH/2 - 4, 'text-anchor': 'end',
        'font-size': 12.6, 'font-weight': 570, fill: t.ink }, row.label));
      if (row.note) {
        var note = row.note.length > 38 ? row.note.slice(0, 37) + '\u2026' : row.note;
        s.appendChild(el('text', { x: gut - 14, y: y + rowH/2 + 11, 'text-anchor': 'end',
          'font-size': 10.8, fill: t.faint }, note));
      }
      var txt = (spec.prefix || '') + row.display;
      var midW = (w1 + w2) / 2, needs = txt.length * 8.6 + 18;
      if (midW >= needs) {
        s.appendChild(el('text', { x: cx, y: y + rowH/2 + 1, 'text-anchor': 'middle',
          'font-size': 14, 'font-weight': 680, fill: css('--surface','#fff'),
          'font-family': css('--mono','monospace') }, txt));
      } else {
        s.appendChild(el('text', { x: cx + midW/2 + 12, y: y + rowH/2 + 1, 'text-anchor': 'start',
          'font-size': 14, 'font-weight': 680, fill: t.ink,
          'font-family': css('--mono','monospace') }, txt));
      }
    });
    return s;
  };

  /* Roadmap / gantt: {periods:[], rows:[{label, start, span, color?, badge?}]} */
  KD.roadmap = function (spec) {
    var t = tone(), pal = palette();
    var w = 760, gut = 178, rowH = 36, headH = 30;
    var h = headH + spec.rows.length * rowH + 12;
    var pw = w - gut - 12, colW = pw / spec.periods.length;
    var s = svgRoot(w, h);
    spec.periods.forEach(function (p, i) {
      s.appendChild(el('text', { x: gut + colW * i + colW / 2, y: 15, 'text-anchor': 'middle',
        'font-size': 11, 'font-weight': 640, fill: t.muted, 'letter-spacing': '.05em' }, p.toUpperCase()));
      if (i) s.appendChild(el('line', { x1: gut + colW * i, y1: 22, x2: gut + colW * i, y2: h - 8,
        stroke: t.border, 'stroke-dasharray': '3 4' }));
    });
    s.appendChild(el('line', { x1: gut, y1: 22, x2: w - 12, y2: 22, stroke: t.border }));
    spec.rows.forEach(function (row, i) {
      var y = headH + i * rowH;
      s.appendChild(el('text', { x: 0, y: y + 20, 'font-size': 12.4, fill: t.ink2, 'font-weight': 520 }, row.label));
      var c = row.color ? css(row.color, row.color) : pal[i % pal.length];
      var x = gut + colW * row.start + 3, bw = colW * row.span - 6;
      var rect = el('rect', { x: x, y: y + 7, width: 0, height: 22, rx: 6, fill: c, opacity: .88 });
      rect.appendChild(el('animate', { attributeName: 'width', from: 0, to: bw, dur: '.6s', fill: 'freeze' }));
      s.appendChild(rect);
      if (row.badge) s.appendChild(el('text', { x: x + 10, y: y + 22, 'font-size': 10.8, 'font-weight': 660,
        fill: css('--surface','#fff') }, row.badge));
    });
    return s;
  };

  function legend(s, series, x, y, pal) {
    var cx = x;
    series.forEach(function (se, i) {
      var c = se.color ? css(se.color, se.color) : pal[i % pal.length];
      s.appendChild(el('rect', { x: cx, y: y - 8, width: 10, height: 10, rx: 3, fill: c }));
      var txt = el('text', { x: cx + 16, y: y + 1, 'font-size': 12, fill: tone().muted }, se.name);
      s.appendChild(txt);
      cx += 26 + (se.name.length * 6.6);
    });
  }

  window.DK = KD;   // DK — "doc kit"

  /* ---------------- Declarative mount + redraw ---------------- */
  var mounts = [];
  window.DKMount = function (selector, kind, spec) {
    var node = document.querySelector(selector);
    if (!node) return;
    mounts.push({ node: node, kind: kind, spec: spec });
    render(node, kind, spec);
  };
  function render(node, kind, spec) {
    node.textContent = '';
    node.appendChild(KD[kind](typeof spec === 'function' ? spec() : spec));
  }
  function redrawAll() { mounts.forEach(function (m) { render(m.node, m.kind, m.spec); }); }
  window.DKRedraw = redrawAll;

  /* ---------------- TOC + scroll-spy ---------------- */
  function buildTOC() {
    var side = document.querySelector('[data-toc]');
    if (!side) return;
    var heads = document.querySelectorAll('.d-wrap h2[id], .d-wrap h3[id]');
    if (!heads.length) return;
    heads.forEach(function (hd) {
      var a = document.createElement('a');
      a.href = '#' + hd.id;
      a.textContent = hd.dataset.toc || hd.firstChild.textContent.trim();
      a.className = 'lvl-' + hd.tagName[1];
      side.appendChild(a);
      var an = document.createElement('a');
      an.href = '#' + hd.id; an.className = 'anchor'; an.textContent = '#'; an.setAttribute('aria-hidden','true');
      hd.appendChild(an);
    });
    var links = side.querySelectorAll('a');
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (!e.isIntersecting) return;
        links.forEach(function (l) { l.classList.toggle('on', l.getAttribute('href') === '#' + e.target.id); });
      });
    }, { rootMargin: '-70px 0px -72% 0px', threshold: 0 });
    heads.forEach(function (hd) { io.observe(hd); });
  }

  /* ---------------- Tabs ---------------- */
  function initTabs() {
    document.querySelectorAll('.tabs').forEach(function (box) {
      var btns = box.querySelectorAll('.tabs-nav button');
      var panels = box.querySelectorAll('.tab-panel');
      btns.forEach(function (b, i) {
        b.setAttribute('aria-selected', i === 0 ? 'true' : 'false');
        if (panels[i]) panels[i].hidden = i !== 0;
        b.addEventListener('click', function () {
          btns.forEach(function (x, j) {
            x.setAttribute('aria-selected', x === b ? 'true' : 'false');
            if (panels[j]) panels[j].hidden = x !== b;
          });
          redrawAll();
        });
      });
    });
  }

  /* ---------------- Screenshot tilt ---------------- */
  /* Rotates the frame toward the pointer. Reads are batched into one rAF so a
     fast sweep cannot queue a layout per mousemove event. */
  function initTilt() {
    var fine = window.matchMedia('(hover: hover) and (pointer: fine)').matches;
    var still = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (!fine || still) return;

    document.querySelectorAll('figure.shot').forEach(function (fig) {
      var frame = fig.querySelector('.shot-frame');
      if (!frame) return;
      var raf = 0, ry = 0, rx = 0, mx = 50, my = 0;

      function paint() {
        raf = 0;
        frame.style.transform =
          'rotateX(' + rx.toFixed(2) + 'deg) rotateY(' + ry.toFixed(2) + 'deg) scale(1)';
        frame.style.setProperty('--mx', mx.toFixed(1) + '%');
        frame.style.setProperty('--my', my.toFixed(1) + '%');
      }

      fig.addEventListener('pointermove', function (e) {
        var r = frame.getBoundingClientRect();
        var px = (e.clientX - r.left) / r.width;          /* 0 .. 1 */
        var py = (e.clientY - r.top) / r.height;
        mx = px * 100; my = py * 100;
        ry = (px - 0.5) * 20;      /* left/right swing */
        rx = (0.5 - py) * 12;      /* toward/away       */
        fig.classList.add('tracking');
        if (!raf) raf = requestAnimationFrame(paint);
      });

      fig.addEventListener('pointerleave', function () {
        if (raf) { cancelAnimationFrame(raf); raf = 0; }
        fig.classList.remove('tracking');
        frame.style.transform = '';   /* back to flat, per the CSS default */
      });
    });
  }

  document.addEventListener('DOMContentLoaded', function () {
    buildTOC(); initTabs(); initTilt();
    if (window.DKInit) window.DKInit();
  });
})();
