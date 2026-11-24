// Renders a single LaTeX expression with strict limits and returns its bounding box as JSON.
// Host passes plain text only; KaTeX runs with trust:false and throwOnError:true.
window.lnaRenderFormula = function (latex, displayMode) {
  var el = document.getElementById('formula');
  el.className = displayMode ? 'display' : '';
  el.innerHTML = '';

  var src = String(latex || '');
  if (src.length > 2000) {
    return JSON.stringify({ ok: false, error: '公式长度超过 2000 字符上限' });
  }
  if (!src.trim()) {
    return JSON.stringify({ ok: false, error: '公式内容为空' });
  }

  try {
    katex.render(src, el, {
      displayMode: !!displayMode,
      throwOnError: true,
      strict: false,
      trust: false,
      maxSize: 10,
      maxExpand: 1000
    });
    var rect = el.getBoundingClientRect();
    var probe = document.getElementById('probe');
    var probeRect = probe ? probe.getBoundingClientRect() : null;
    var rawW = Math.ceil(rect.width);
    var rawH = Math.ceil(rect.height);
    var width = Math.min(Math.max(rawW, 4), 1600);
    var height = Math.min(Math.max(rawH, 4), 800);
    var descent = probeRect
      ? Math.max(0, Math.min(height, Math.round(rect.bottom - probeRect.top)))
      : 0;
    return JSON.stringify({
      ok: true,
      width: width,
      height: height,
      descent: descent,
      overflow: rawW > 1600 || rawH > 800
    });
  } catch (err) {
    el.innerHTML = '';
    var msg = (err && err.message) ? String(err.message) : 'KaTeX 解析失败';
    return JSON.stringify({ ok: false, error: msg });
  }
};

// Preloads all @font-face KaTeX fonts so initial measurements never use fallback fonts.
window.__lnaFontsReady = false;
window.lnaWarmupFonts = function () {
  if (!document.fonts || !document.fonts.forEach) {
    window.__lnaFontsReady = true;
    return true;
  }
  var loads = [];
  document.fonts.forEach(function (f) {
    if (f.status !== 'loaded') {
      loads.push(f.load().catch(function () {}));
    }
  });
  if (loads.length === 0) {
    window.__lnaFontsReady = true;
    return true;
  }
  Promise.all(loads).then(function () {
    window.__lnaFontsReady = true;
  });
  return false;
};

window.__lnaPainted = false;
window.lnaStartPaintWait = function () {
  window.__lnaPainted = false;
  requestAnimationFrame(function () {
    requestAnimationFrame(function () {
      window.__lnaPainted = true;
    });
  });
};

window.lnaIsReadyForCapture = function (targetW, targetH) {
  var fontsOk = !document.fonts || document.fonts.status !== 'loading';
  var sizeOk = Math.abs(window.innerWidth - targetW) <= 2 && Math.abs(window.innerHeight - targetH) <= 2;
  return fontsOk && sizeOk && !!window.__lnaPainted;
};

var lastBatchNodes = [];
var lastBatchResults = [];

window.lnaMeasureBatch = function () {
  var atlas = document.getElementById('atlas');
  var atlasRect = atlas.getBoundingClientRect();
  var atlasWidth = Math.min(Math.max(Math.ceil(atlasRect.width), 4), 1600);
  var atlasHeight = Math.min(Math.max(Math.ceil(atlasRect.height), 4), 3200);

  for (var j = 0; j < lastBatchNodes.length; j++) {
    var n = lastBatchNodes[j];
    if (!n) continue;
    var r = n.el.getBoundingClientRect();
    var pr = n.probe.getBoundingClientRect();
    var x = Math.max(0, Math.floor(r.left - atlasRect.left));
    var y = Math.max(0, Math.floor(r.top - atlasRect.top));
    var rawW = Math.ceil(r.right - atlasRect.left) - x;
    var rawH = Math.ceil(r.bottom - atlasRect.top) - y;
    var w = Math.min(Math.max(rawW, 4), 1600);
    var h = Math.min(Math.max(rawH, 4), 800);
    var descent = Math.max(0, Math.min(h, Math.round(r.bottom - pr.top)));
    lastBatchResults[j] = {
      ok: true,
      x: x,
      y: y,
      width: w,
      height: h,
      descent: descent,
      overflow: rawW > 1600 || rawH > 800
    };
  }

  return JSON.stringify({
    atlasWidth: atlasWidth,
    atlasHeight: atlasHeight,
    items: lastBatchResults
  });
};

// Renders a batch of LaTeX expressions into #atlas in a single layout pass so the host
// can capture one screenshot and crop each formula.
window.lnaRenderBatch = function (items) {
  var stage = document.getElementById('stage');
  var atlas = document.getElementById('atlas');
  if (stage) stage.style.display = 'none';
  atlas.style.display = 'flex';
  atlas.innerHTML = '';

  var list = Array.isArray(items) ? items : [];
  lastBatchNodes = [];
  lastBatchResults = [];

  for (var i = 0; i < list.length; i++) {
    var item = list[i] || {};
    var src = String(item.latex || '');
    var dm = !!item.displayMode;
    if (src.length > 2000) {
      lastBatchResults.push({ ok: false, error: '公式长度超过 2000 字符上限' });
      lastBatchNodes.push(null);
      continue;
    }
    if (!src.trim()) {
      lastBatchResults.push({ ok: false, error: '公式内容为空' });
      lastBatchNodes.push(null);
      continue;
    }

    var wrap = document.createElement('div');
    wrap.className = 'batch-item';
    var el = document.createElement('div');
    el.className = dm ? 'batch-formula display' : 'batch-formula';
    var probe = document.createElement('span');
    probe.className = 'batch-probe';
    wrap.appendChild(el);
    wrap.appendChild(probe);

    try {
      katex.render(src, el, {
        displayMode: dm,
        throwOnError: true,
        strict: false,
        trust: false,
        maxSize: 10,
        maxExpand: 1000
      });
      atlas.appendChild(wrap);
      lastBatchNodes.push({ el: el, probe: probe });
      lastBatchResults.push({ ok: true });
    } catch (err) {
      var msg = (err && err.message) ? String(err.message) : 'KaTeX 解析失败';
      lastBatchResults.push({ ok: false, error: msg });
      lastBatchNodes.push(null);
    }
  }

  return window.lnaMeasureBatch();
};
