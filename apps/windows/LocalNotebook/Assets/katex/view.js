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
