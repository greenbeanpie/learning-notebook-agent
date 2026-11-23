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

// Renders a batch of LaTeX expressions into #atlas in a single layout pass so the host
// can capture one screenshot and crop each formula.
window.lnaRenderBatch = function (items) {
  var stage = document.getElementById('stage');
  var atlas = document.getElementById('atlas');
  if (stage) stage.style.display = 'none';
  atlas.style.display = 'flex';
  atlas.innerHTML = '';

  var list = Array.isArray(items) ? items : [];
  var nodes = [];
  var results = [];

  for (var i = 0; i < list.length; i++) {
    var item = list[i] || {};
    var src = String(item.latex || '');
    var dm = !!item.displayMode;
    if (src.length > 2000) {
      results.push({ ok: false, error: '公式长度超过 2000 字符上限' });
      nodes.push(null);
      continue;
    }
    if (!src.trim()) {
      results.push({ ok: false, error: '公式内容为空' });
      nodes.push(null);
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
      nodes.push({ el: el, probe: probe });
      results.push({ ok: true });
    } catch (err) {
      var msg = (err && err.message) ? String(err.message) : 'KaTeX 解析失败';
      results.push({ ok: false, error: msg });
      nodes.push(null);
    }
  }

  var atlasRect = atlas.getBoundingClientRect();
  var atlasWidth = Math.min(Math.max(Math.ceil(atlasRect.width), 4), 1600);
  var atlasHeight = Math.min(Math.max(Math.ceil(atlasRect.height), 4), 3200);

  for (var j = 0; j < nodes.length; j++) {
    var n = nodes[j];
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
    results[j] = {
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
    items: results
  });
};
