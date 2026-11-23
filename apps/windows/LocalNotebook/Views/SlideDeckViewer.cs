using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>
/// Interactive 16:9 animated PPT presentation player built on WebView2.
/// Renders all 5 curated editorial themes and 12 rich slide layouts with
/// directional slide transitions, staggered element entrance animations,
/// auto-play slideshow mode, keyboard navigation, thumbnail strip, and speaker notes.
/// </summary>
internal sealed class SlideDeckViewer
{
    public FrameworkElement Root { get; }

    public SlideDeckViewer(JsonElement payload, bool compact, Action<StackPanel>? appendFeedback = null)
    {
        var slides = Arr(payload, "slides").ToList();
        if (slides.Count == 0)
        {
            Root = new TextBlock { Text = L10n.T("slides.empty"), Margin = new Thickness(0, 12, 0, 12) };
            return;
        }

        var webView = new WebView2
        {
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            MinHeight = compact ? 380 : 540,
            DefaultBackgroundColor = Windows.UI.Color.FromArgb(255, 19, 20, 24),
        };

        var html = BuildPresentationHtml(payload, L10n.IsEnglish);
        webView.Loaded += async (_, _) =>
        {
            try
            {
                await webView.EnsureCoreWebView2Async();
                webView.CoreWebView2.Settings.AreDefaultContextMenusEnabled = false;
                webView.CoreWebView2.Settings.AreDevToolsEnabled = false;
                webView.NavigateToString(html);
            }
            catch
            {
                // Keep graceful empty surface if WebView2 runtime is unavailable
            }
        };

        var stageFrame = new Border
        {
            CornerRadius = new CornerRadius(14),
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 19, 20, 24)),
            Child = webView,
        };

        if (compact)
        {
            var box = new StackPanel { Spacing = 10 };
            stageFrame.Height = 420;
            box.Children.Add(stageFrame);
            appendFeedback?.Invoke(box);
            Root = box;
        }
        else
        {
            var grid = new Grid { RowSpacing = 10 };
            grid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
            grid.Children.Add(stageFrame);
            if (appendFeedback != null)
            {
                var fbHost = new StackPanel { Spacing = 6 };
                appendFeedback(fbHost);
                if (fbHost.Children.Count > 0)
                {
                    grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
                    Grid.SetRow(fbHost, 1);
                    grid.Children.Add(fbHost);
                }
            }
            Root = grid;
        }
    }

    private static string BuildPresentationHtml(JsonElement payload, bool en)
    {
        var rawJson = payload.GetRawText()
            .Replace("</", "<\\/")
            .Replace("\u2028", "\\u2028")
            .Replace("\u2029", "\\u2029");

        var tPrev = en ? "Previous" : "上一页";
        var tNext = en ? "Next" : "下一页";
        var tPlay = en ? "Play" : "自动放映";
        var tPause = en ? "Pause" : "暂停放映";
        var tNotes = en ? "Speaker Notes" : "演讲备注";
        var tNoNotes = en ? "No speaker notes for this slide." : "本页暂无演讲备注。";
        var tAnimSlide = en ? "Slide" : "滑动切换";
        var tAnimFade = en ? "Fade" : "淡入淡出";
        var tAnimZoom = en ? "Zoom" : "缩放切换";
        var tFullscreen = en ? "Present" : "全屏放映";

        return $$"""
<!doctype html>
<html lang="{{(en ? "en" : "zh-CN")}}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
  :root {
    --ui-bg: #131418;
    --ui-panel: #1b1d24;
    --ui-border: rgba(255,255,255,0.10);
    --ui-text: #e8eaed;
    --ui-muted: #9aa0a6;
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  html, body {
    width: 100%; height: 100%;
    background: var(--ui-bg);
    color: var(--ui-text);
    font-family: "Microsoft YaHei UI", "Segoe UI", system-ui, -apple-system, sans-serif;
    overflow: hidden;
    user-select: none;
  }
  .app {
    display: flex; flex-direction: column;
    width: 100%; height: 100%;
    padding: 12px 14px 10px 14px;
    gap: 10px;
  }
  .stage-wrap {
    flex: 1; min-height: 0;
    display: flex; align-items: center; justify-content: center;
    position: relative;
  }
  .deck-frame {
    position: relative;
    width: 100%; max-width: 1080px;
    aspect-ratio: 16 / 9;
    max-height: 100%;
    border-radius: 12px;
    overflow: hidden;
    box-shadow: 0 18px 48px rgba(0,0,0,0.48), 0 0 0 1px rgba(255,255,255,0.08);
    background: #0f172a;
  }
  .autoplay-bar {
    position: absolute; top: 0; left: 0; height: 3px; width: 0%;
    background: linear-gradient(90deg, var(--accent, #38bdf8), var(--primary, #3b82f6));
    z-index: 30;
  }
  .slide-layer {
    position: absolute; inset: 0;
    width: 100%; height: 100%;
    display: flex; flex-direction: column;
    overflow: hidden;
  }
  /* Slide transitions */
  .anim-slide-next { animation: slideInNext 0.36s cubic-bezier(0.22, 1, 0.36, 1) both; }
  .anim-slide-prev { animation: slideInPrev 0.36s cubic-bezier(0.22, 1, 0.36, 1) both; }
  .anim-fade       { animation: fadeInSlide 0.34s ease-out both; }
  .anim-zoom       { animation: zoomInSlide 0.36s cubic-bezier(0.22, 1, 0.36, 1) both; }
  @keyframes slideInNext {
    from { opacity: 0; transform: translateX(48px) scale(0.985); }
    to   { opacity: 1; transform: translateX(0) scale(1); }
  }
  @keyframes slideInPrev {
    from { opacity: 0; transform: translateX(-48px) scale(0.985); }
    to   { opacity: 1; transform: translateX(0) scale(1); }
  }
  @keyframes fadeInSlide {
    from { opacity: 0; }
    to   { opacity: 1; }
  }
  @keyframes zoomInSlide {
    from { opacity: 0; transform: scale(0.94); }
    to   { opacity: 1; transform: scale(1); }
  }
  @keyframes elemRise {
    from { opacity: 0; transform: translateY(14px); }
    to   { opacity: 1; transform: translateY(0); }
  }
  .stagger { animation: elemRise 0.42s cubic-bezier(0.22, 1, 0.36, 1) both; }

  /* Click zones on left/right slide edges */
  .edge-nav {
    position: absolute; top: 0; bottom: 0; width: 11%;
    display: flex; align-items: center; justify-content: center;
    z-index: 20; cursor: pointer; opacity: 0; transition: opacity 0.18s;
  }
  .edge-nav:hover { opacity: 1; }
  .edge-nav.left  { left: 0; background: linear-gradient(90deg, rgba(0,0,0,0.28), transparent); }
  .edge-nav.right { right: 0; background: linear-gradient(270deg, rgba(0,0,0,0.28), transparent); }
  .edge-pill {
    width: 34px; height: 34px; border-radius: 50%;
    background: rgba(15,23,42,0.72); color: #fff;
    display: flex; align-items: center; justify-content: center;
    font-size: 16px; border: 1px solid rgba(255,255,255,0.2);
  }

  /* Slide internal typography & components */
  .s-top-bar { height: 5px; width: 100%; background: var(--primary); flex-shrink: 0; }
  .s-pad { padding: 3.6% 5% 2.6% 5%; flex: 1; display: flex; flex-direction: column; min-height: 0; }
  .s-eyebrow { font-size: clamp(9px, 1.05vw, 12px); font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; color: var(--primary); margin-bottom: 4px; }
  .s-title { font-size: clamp(18px, 2.5vw, 30px); font-weight: 700; line-height: 1.22; color: var(--text-dark); }
  .s-sub { font-size: clamp(11px, 1.25vw, 15px); color: var(--text-muted); margin-top: 4px; line-height: 1.4; }
  .s-rule { height: 1px; background: var(--border); margin: 10px 0 14px 0; flex-shrink: 0; }
  .s-body { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 10px; overflow: hidden; }
  .s-callout {
    margin-top: auto; padding: 9px 14px; border-radius: 8px;
    background: var(--card-alt); border: 1px solid var(--border);
    border-left: 4px solid var(--primary);
    font-size: clamp(11px, 1.2vw, 14px); font-weight: 600; color: var(--text-dark);
  }
  .s-footer {
    padding: 0 5% 1.8% 5%;
    display: flex; justify-content: space-between; align-items: center;
    font-size: clamp(9px, 0.95vw, 11px); color: var(--text-muted); flex-shrink: 0;
  }

  /* Dark cover / section / summary */
  .s-dark { background: var(--dark-bg); color: var(--text-on-dark); position: relative; }
  .s-dark-rail { position: absolute; right: 0; top: 0; bottom: 0; width: 20%; background: var(--dark-surface); border-left: 4px solid var(--accent); }
  .s-cover-inner { position: relative; z-index: 2; padding: 11% 24% 6% 7%; flex: 1; display: flex; flex-direction: column; justify-content: center; gap: 12px; }
  .s-cover-bar { width: 72px; height: 5px; border-radius: 3px; background: var(--accent); }
  .s-cover-title { font-size: clamp(22px, 3.6vw, 44px); font-weight: 700; line-height: 1.18; color: var(--text-on-dark); }
  .s-cover-sub { font-size: clamp(12px, 1.5vw, 18px); color: var(--text-on-dark-sub); line-height: 1.45; }
  .s-cover-list { display: flex; flex-direction: column; gap: 7px; margin-top: 6px; }
  .s-cover-item { font-size: clamp(11px, 1.3vw, 16px); color: var(--text-on-dark-sub); }
  .s-section-num { font-size: clamp(34px, 5.2vw, 64px); font-weight: 800; color: var(--accent); line-height: 1; }

  /* Cards grid */
  .grid-cards { display: grid; gap: 12px; flex: 1; min-height: 0; }
  .grid-2 { grid-template-columns: repeat(2, 1fr); }
  .grid-3 { grid-template-columns: repeat(3, 1fr); }
  .card {
    background: var(--card-bg); border: 1px solid var(--border); border-radius: 10px;
    border-top: 4px solid var(--primary); padding: 12px 14px;
    display: flex; flex-direction: column; gap: 6px; overflow: hidden;
  }
  .card-tag { font-size: clamp(9px, 0.95vw, 11px); font-weight: 700; color: var(--primary); text-transform: uppercase; }
  .card-t { font-size: clamp(12px, 1.4vw, 17px); font-weight: 700; color: var(--text-dark); line-height: 1.3; }
  .card-b { font-size: clamp(11px, 1.18vw, 14px); color: var(--text-body); line-height: 1.48; }

  /* Two-column */
  .col-card {
    background: var(--card-bg); border: 1px solid var(--border); border-radius: 10px;
    border-top: 4px solid var(--primary); padding: 14px 16px;
    display: flex; flex-direction: column; gap: 8px;
  }
  .col-item { font-size: clamp(11px, 1.25vw, 15px); color: var(--text-body); line-height: 1.45; }

  /* Metrics */
  .metric-card {
    background: var(--card-bg); border: 1px solid var(--border); border-radius: 10px;
    border-top: 4px solid var(--primary); padding: 16px 12px;
    text-align: center; display: flex; flex-direction: column; justify-content: center; gap: 6px;
  }
  .metric-val { font-size: clamp(22px, 3vw, 36px); font-weight: 800; color: var(--primary); }
  .metric-lbl { font-size: clamp(11px, 1.25vw, 15px); font-weight: 700; color: var(--text-dark); }
  .metric-det { font-size: clamp(10px, 1.05vw, 12.5px); color: var(--text-muted); }

  /* Timeline */
  .timeline-row { display: flex; gap: 12px; flex: 1; align-items: stretch; position: relative; padding-top: 8px; }
  .tl-step { flex: 1; display: flex; flex-direction: column; align-items: center; gap: 10px; }
  .tl-dot {
    width: 36px; height: 36px; border-radius: 50%;
    background: var(--primary); color: #fff; font-weight: 700; font-size: 13px;
    display: flex; align-items: center; justify-content: center; flex-shrink: 0;
    box-shadow: 0 4px 12px rgba(0,0,0,0.16);
  }
  .tl-card {
    flex: 1; width: 100%; background: var(--card-bg); border: 1px solid var(--border);
    border-radius: 10px; padding: 10px 12px; text-align: center;
    display: flex; flex-direction: column; gap: 6px;
  }

  /* Table */
  .ppt-table { width: 100%; border-collapse: collapse; font-size: clamp(11px, 1.18vw, 14px); border-radius: 8px; overflow: hidden; }
  .ppt-table th { background: var(--primary); color: #fff; padding: 9px 12px; text-align: left; font-weight: 700; }
  .ppt-table td { padding: 8px 12px; border-bottom: 1px solid var(--border); color: var(--text-body); background: var(--card-bg); }
  .ppt-table tr:nth-child(even) td { background: var(--card-alt); }
  .ppt-table td:first-child { font-weight: 700; color: var(--text-dark); }

  /* Hero Formula / Quote */
  .hero-box {
    background: var(--dark-bg); color: var(--text-on-dark);
    border-radius: 10px; border-left: 6px solid var(--accent);
    padding: 18px 24px; text-align: center;
    font-size: clamp(15px, 2vw, 24px); line-height: 1.45;
  }
  .hero-formula { font-family: "Consolas", "Cambria Math", monospace; font-weight: 700; }
  .hero-quote { font-style: italic; }

  /* Bullet cards */
  .bullet-row {
    background: var(--card-bg); border: 1px solid var(--border);
    border-left: 4px solid var(--primary); border-radius: 8px;
    padding: 9px 14px; font-size: clamp(11px, 1.28vw, 15px);
    color: var(--text-body); line-height: 1.45;
  }

  /* Agenda */
  .agenda-item {
    display: flex; align-items: center; gap: 12px;
    background: var(--card-bg); border: 1px solid var(--border);
    border-radius: 10px; padding: 10px 14px;
  }
  .agenda-num {
    width: 34px; height: 34px; border-radius: 8px;
    background: var(--primary); color: #fff; font-weight: 700; font-size: 14px;
    display: flex; align-items: center; justify-content: center; flex-shrink: 0;
  }

  /* Control bar & thumbnail strip */
  .controls {
    display: flex; align-items: center; justify-content: space-between;
    gap: 8px; flex-wrap: wrap;
    background: var(--ui-panel); border: 1px solid var(--ui-border);
    border-radius: 10px; padding: 6px 10px; flex-shrink: 0;
  }
  .ctrl-group { display: flex; align-items: center; gap: 6px; }
  button.btn, select.sel {
    background: rgba(255,255,255,0.07); color: var(--ui-text);
    border: 1px solid var(--ui-border); border-radius: 16px;
    padding: 5px 12px; font-size: 12px; cursor: pointer;
    font-family: inherit; transition: background 0.15s;
  }
  button.btn:hover:not(:disabled) { background: rgba(255,255,255,0.14); }
  button.btn:disabled { opacity: 0.35; cursor: default; }
  button.btn.primary { background: #2563eb; border-color: #3b82f6; color: #fff; }
  button.btn.primary:hover { background: #1d4ed8; }
  .counter { font-size: 12.5px; color: var(--ui-muted); min-width: 54px; text-align: center; font-variant-numeric: tabular-nums; }

  .thumbs {
    display: flex; gap: 6px; overflow-x: auto; padding-bottom: 2px; flex-shrink: 0;
  }
  .thumb {
    flex: 0 0 auto; min-width: 96px; max-width: 140px;
    background: var(--ui-panel); border: 1px solid var(--ui-border);
    border-radius: 8px; padding: 5px 8px; cursor: pointer;
    font-size: 11px; color: var(--ui-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
    transition: border-color 0.15s, background 0.15s;
  }
  .thumb.active { border-color: #60a5fa; background: rgba(59,130,246,0.18); color: #fff; font-weight: 600; }

  .notes-box {
    background: var(--ui-panel); border: 1px solid var(--ui-border);
    border-radius: 10px; padding: 8px 12px; font-size: 12.5px;
    color: var(--ui-muted); max-height: 92px; overflow-y: auto;
    line-height: 1.5; user-select: text; display: none; flex-shrink: 0;
  }
  .notes-box.open { display: block; }
</style>
</head>
<body>
<div class="app" id="appRoot">
  <div class="stage-wrap">
    <div class="deck-frame" id="deckFrame">
      <div class="autoplay-bar" id="autoBar"></div>
      <div class="edge-nav left" id="edgePrev"><div class="edge-pill">‹</div></div>
      <div class="edge-nav right" id="edgeNext"><div class="edge-pill">›</div></div>
      <div id="slideMount" class="slide-layer"></div>
    </div>
  </div>

  <div class="controls">
    <div class="ctrl-group">
      <button class="btn" id="btnPrev">‹ {{tPrev}}</button>
      <span class="counter" id="lblCounter">1 / 1</span>
      <button class="btn" id="btnNext">{{tNext}} ›</button>
    </div>
    <div class="ctrl-group">
      <button class="btn primary" id="btnPlay">▶ {{tPlay}}</button>
      <select class="sel" id="selAnim">
        <option value="slide">{{tAnimSlide}}</option>
        <option value="fade">{{tAnimFade}}</option>
        <option value="zoom">{{tAnimZoom}}</option>
      </select>
      <button class="btn" id="btnNotes">{{tNotes}}</button>
      <button class="btn" id="btnFs">⛶ {{tFullscreen}}</button>
    </div>
  </div>

  <div class="thumbs" id="thumbStrip"></div>
  <div class="notes-box" id="notesBox"></div>
</div>

<script>
const PAYLOAD = {{rawJson}};
const T_PLAY = "{{JsonEncodedText.Encode(tPlay)}}";
const T_PAUSE = "{{JsonEncodedText.Encode(tPause)}}";
const T_NO_NOTES = "{{JsonEncodedText.Encode(tNoNotes)}}";

const THEMES = {
  executive_navy:  { darkBg:'#0f172a', darkSurf:'#1e293b', lightBg:'#f8fafc', cardBg:'#ffffff', cardAlt:'#f1f5f9', primary:'#1d4ed8', accent:'#0ea5e9', warm:'#d97706', border:'#e2e8f0', textDark:'#0f172a', textBody:'#334155', textMuted:'#64748b', textOnDark:'#f8fafc', textOnDarkSub:'#cbd5e1' },
  editorial_slate: { darkBg:'#18181b', darkSurf:'#27272a', lightBg:'#fafaf9', cardBg:'#ffffff', cardAlt:'#f4f4f5', primary:'#4f46e5', accent:'#06b6d4', warm:'#e11d48', border:'#e4e4e7', textDark:'#18181b', textBody:'#3f3f46', textMuted:'#71717a', textOnDark:'#fafafa', textOnDarkSub:'#d4d4d8' },
  academic_sage:   { darkBg:'#132a22', darkSurf:'#1e3b30', lightBg:'#f7faf8', cardBg:'#ffffff', cardAlt:'#ecf4f0', primary:'#0f766e', accent:'#059669', warm:'#b45309', border:'#d8e6df', textDark:'#132a22', textBody:'#2d473e', textMuted:'#5c766c', textOnDark:'#f2faf6', textOnDarkSub:'#c2dcd1' },
  warm_terracotta: { darkBg:'#291c17', darkSurf:'#3d2b24', lightBg:'#fdfaf6', cardBg:'#ffffff', cardAlt:'#f6eee6', primary:'#c2410c', accent:'#d97706', warm:'#9a3412', border:'#e8dacc', textDark:'#291c17', textBody:'#4a372f', textMuted:'#7c665c', textOnDark:'#fffbf5', textOnDarkSub:'#e5d3c6' },
  midnight_aurora: { darkBg:'#0d1117', darkSurf:'#161b22', lightBg:'#f5f7fb', cardBg:'#ffffff', cardAlt:'#ebf0f7', primary:'#2563eb', accent:'#7c3aed', warm:'#0891b2', border:'#dce3ee', textDark:'#111827', textBody:'#374151', textMuted:'#6b7280', textOnDark:'#f3f4f6', textOnDarkSub:'#c7d2fe' }
};

const theme = THEMES[(PAYLOAD.theme || 'executive_navy').toLowerCase()] || THEMES.executive_navy;
const deckTitle = PAYLOAD.title || 'Presentation';
const slides = Array.isArray(PAYLOAD.slides) && PAYLOAD.slides.length ? PAYLOAD.slides : [{ layout:'title', title: deckTitle, bullets:[] }];

const rootStyle = document.getElementById('deckFrame').style;
rootStyle.setProperty('--dark-bg', theme.darkBg);
rootStyle.setProperty('--dark-surface', theme.darkSurf);
rootStyle.setProperty('--light-bg', theme.lightBg);
rootStyle.setProperty('--card-bg', theme.cardBg);
rootStyle.setProperty('--card-alt', theme.cardAlt);
rootStyle.setProperty('--primary', theme.primary);
rootStyle.setProperty('--accent', theme.accent);
rootStyle.setProperty('--warm', theme.warm);
rootStyle.setProperty('--border', theme.border);
rootStyle.setProperty('--text-dark', theme.textDark);
rootStyle.setProperty('--text-body', theme.textBody);
rootStyle.setProperty('--text-muted', theme.textMuted);
rootStyle.setProperty('--text-on-dark', theme.textOnDark);
rootStyle.setProperty('--text-on-dark-sub', theme.textOnDarkSub);

let current = 0;
let playing = false;
let playTimer = null;
const PLAY_MS = 5200;

function esc(s) {
  return String(s || '').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}
function fmt(s) {
  return esc(s).replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>').replace(/`(.+?)`/g, '<code>$1</code>');
}
function stagger(i) { return `style="animation-delay:${Math.min(i * 75, 450)}ms"`; }

function renderSlideHtml(s, idx) {
  const layout = (s.layout || 'bullets').toLowerCase();
  const title = s.title || `Slide ${idx + 1}`;
  const sub = s.subtitle || (idx === 0 ? (PAYLOAD.subtitle || '') : '');
  const bullets = Array.isArray(s.bullets) ? s.bullets.filter(x => String(x||'').trim()) : [];
  const callout = s.callout ? `<div class="s-callout stagger" ${stagger(5)}>${fmt(s.callout)}</div>` : '';
  const footer = (dark) => `<div class="s-footer" style="color:${dark ? 'var(--text-on-dark-sub)' : 'var(--text-muted)'}"><span>${esc(deckTitle)}</span><strong>${idx + 1} / ${slides.length}</strong></div>`;

  if (layout === 'title' || layout === 'summary') {
    const kicker = layout === 'summary' ? 'KEY TAKEAWAYS & SUMMARY' : 'PRESENTATION';
    const items = bullets.slice(0,5).map((b, i) => `<div class="s-cover-item stagger" ${stagger(i+2)}>▸ ${fmt(b)}</div>`).join('');
    return `<div class="slide-layer s-dark">
      <div class="s-dark-rail"></div>
      <div class="s-cover-inner">
        <div class="s-eyebrow stagger" style="color:var(--accent)">${kicker}</div>
        <div class="s-cover-bar stagger" ${stagger(1)}></div>
        <div class="s-cover-title stagger" ${stagger(1)}>${fmt(title)}</div>
        ${sub ? `<div class="s-cover-sub stagger" ${stagger(2)}>${fmt(sub)}</div>` : ''}
        ${items ? `<div class="s-cover-list">${items}</div>` : ''}
      </div>
      ${footer(true)}
    </div>`;
  }

  if (layout === 'section') {
    const num = s.section_number || String(idx + 1).padStart(2, '0');
    const desc = sub || bullets[0] || '';
    return `<div class="slide-layer s-dark" style="border-left:10px solid var(--primary)">
      <div class="s-cover-inner" style="padding-right:10%">
        <div class="s-section-num stagger">${esc(num)}</div>
        <div class="s-cover-bar stagger" ${stagger(1)}></div>
        <div class="s-cover-title stagger" ${stagger(2)}>${fmt(title)}</div>
        ${desc ? `<div class="s-cover-sub stagger" ${stagger(3)}>${fmt(desc)}</div>` : ''}
      </div>
      ${footer(true)}
    </div>`;
  }

  let eyebrow = '';
  let bodyHtml = '';
  const accents = ['var(--primary)', 'var(--accent)', 'var(--warm)'];

  if (layout === 'agenda') {
    eyebrow = 'Agenda';
    const steps = Array.isArray(s.steps) && s.steps.length ? s.steps : bullets.map((b, i) => {
      const p = String(b).split(/[:：]/);
      return { step: String(i+1).padStart(2,'0'), title: p[0], detail: p.slice(1).join('：') };
    });
    const cls = steps.length > 3 ? 'grid-cards grid-2' : 's-body';
    bodyHtml = `<div class="${cls}">${steps.slice(0,6).map((st, i) => `
      <div class="agenda-item stagger" ${stagger(i+1)}>
        <div class="agenda-num" style="background:${accents[i%3]}">${esc(st.step || String(i+1).padStart(2,'0'))}</div>
        <div><div class="card-t">${fmt(st.title)}</div>${st.detail ? `<div class="card-b">${fmt(st.detail)}</div>` : ''}</div>
      </div>`).join('')}</div>`;
  } else if (layout === 'cards') {
    const cards = Array.isArray(s.cards) && s.cards.length ? s.cards : bullets.map(b => {
      const p = String(b).split(/[:：]/);
      return { tag: '', title: p[0], body: p.slice(1).join('：') };
    });
    const gridCls = (cards.length === 3 || cards.length >= 5) ? 'grid-cards grid-3' : 'grid-cards grid-2';
    bodyHtml = `<div class="${gridCls}">${cards.slice(0,6).map((c, i) => `
      <div class="card stagger" ${stagger(i+1)} style="border-top-color:${accents[i%3]}">
        ${c.tag ? `<div class="card-tag" style="color:${accents[i%3]}">${esc(c.tag)}</div>` : ''}
        <div class="card-t">${fmt(c.title)}</div>
        <div class="card-b">${fmt(c.body)}</div>
      </div>`).join('')}</div>`;
  } else if (layout === 'two_column') {
    let cols = Array.isArray(s.columns) && s.columns.length >= 2 ? s.columns.slice(0,2) : null;
    if (!cols) {
      const mid = Math.ceil(bullets.length / 2);
      cols = [{ heading: '', items: bullets.slice(0, mid) }, { heading: '', items: bullets.slice(mid) }];
    }
    bodyHtml = `<div class="grid-cards grid-2">${cols.map((c, ci) => `
      <div class="col-card stagger" ${stagger(ci+1)} style="border-top-color:${accents[ci%2]}">
        ${c.heading ? `<div class="card-t">${fmt(c.heading)}</div>` : ''}
        ${(c.items || []).slice(0,6).map(it => `<div class="col-item">▸ ${fmt(it)}</div>`).join('')}
      </div>`).join('')}</div>`;
  } else if (layout === 'metrics') {
    eyebrow = 'Key Metrics';
    const ms = Array.isArray(s.metrics) && s.metrics.length ? s.metrics.slice(0,4) : bullets.slice(0,3).map(b => ({ value: String(b).slice(0,10), label: b, detail: '' }));
    const colsCss = `grid-template-columns:repeat(${Math.max(ms.length,1)},1fr)`;
    bodyHtml = `<div class="grid-cards" style="${colsCss};flex:0 0 auto">${ms.map((m, i) => `
      <div class="metric-card stagger" ${stagger(i+1)} style="border-top-color:${accents[i%3]}">
        <div class="metric-val" style="color:${accents[i%3]}">${esc(m.value)}</div>
        <div class="metric-lbl">${fmt(m.label)}</div>
        ${m.detail ? `<div class="metric-det">${fmt(m.detail)}</div>` : ''}
      </div>`).join('')}</div>` +
      (bullets.length ? `<div style="display:flex;flex-direction:column;gap:6px;margin-top:4px">${bullets.slice(0,3).map((b,i) => `<div class="bullet-row stagger" ${stagger(i+3)}>${fmt(b)}</div>`).join('')}</div>` : '');
  } else if (layout === 'timeline') {
    eyebrow = 'Timeline & Process';
    const steps = Array.isArray(s.steps) && s.steps.length ? s.steps.slice(0,5) : bullets.slice(0,5).map((b, i) => {
      const p = String(b).split(/[:：]/);
      return { step: String(i+1).padStart(2,'0'), title: p[0], detail: p.slice(1).join('：') };
    });
    bodyHtml = `<div class="timeline-row">${steps.map((st, i) => `
      <div class="tl-step stagger" ${stagger(i+1)}>
        <div class="tl-dot" style="background:${accents[i%2]}">${esc(st.step || String(i+1).padStart(2,'0'))}</div>
        <div class="tl-card">
          <div class="card-t">${fmt(st.title)}</div>
          <div class="card-b">${fmt(st.detail)}</div>
        </div>
      </div>`).join('')}</div>`;
  } else if (layout === 'table' && s.table && Array.isArray(s.table.headers) && s.table.headers.length) {
    eyebrow = 'Comparison';
    const ths = s.table.headers.map(h => `<th>${fmt(h)}</th>`).join('');
    const trs = (s.table.rows || []).slice(0,7).map((r, ri) => `<tr class="stagger" ${stagger(ri+1)}>${s.table.headers.map((_, ci) => `<td>${fmt(r[ci] || '')}</td>`).join('')}</tr>`).join('');
    bodyHtml = `<table class="ppt-table"><thead><tr>${ths}</tr></thead><tbody>${trs}</tbody></table>`;
  } else if (layout === 'formula' || layout === 'quote') {
    eyebrow = layout === 'formula' ? 'Core Formula' : 'Key Insight';
    const hero = (layout === 'formula' && s.formula) ? s.formula : (bullets[0] || '');
    const rest = (layout === 'formula' && s.formula) ? bullets : bullets.slice(1);
    bodyHtml = `<div class="hero-box stagger ${layout === 'formula' ? 'hero-formula' : 'hero-quote'}">${layout === 'quote' ? '“' + fmt(hero) + '”' : fmt(hero)}</div>` +
      rest.slice(0,4).map((b, i) => `<div class="bullet-row stagger" ${stagger(i+2)}>${fmt(b)}</div>`).join('');
  } else {
    bodyHtml = bullets.slice(0,6).map((b, i) => `<div class="bullet-row stagger" ${stagger(i+1)} style="border-left-color:${accents[i%2]}">${fmt(b)}</div>`).join('');
  }

  return `<div class="slide-layer" style="background:var(--light-bg)">
    <div class="s-top-bar"></div>
    <div class="s-pad">
      ${eyebrow ? `<div class="s-eyebrow">${esc(eyebrow)}</div>` : ''}
      <div class="s-title">${fmt(title)}</div>
      ${sub ? `<div class="s-sub">${fmt(sub)}</div>` : ''}
      <div class="s-rule"></div>
      <div class="s-body">${bodyHtml}${callout}</div>
    </div>
    ${footer(false)}
  </div>`;
}

function go(nextIdx, dir = 1) {
  current = Math.max(0, Math.min(slides.length - 1, nextIdx));
  const mount = document.getElementById('slideMount');
  const mode = document.getElementById('selAnim').value;
  const animCls = mode === 'fade' ? 'anim-fade' : mode === 'zoom' ? 'anim-zoom' : (dir >= 0 ? 'anim-slide-next' : 'anim-slide-prev');

  mount.className = 'slide-layer';
  mount.innerHTML = renderSlideHtml(slides[current], current);
  void mount.offsetWidth;
  mount.classList.add(animCls);

  document.getElementById('lblCounter').textContent = `${current + 1} / ${slides.length}`;
  document.getElementById('btnPrev').disabled = current === 0;
  document.getElementById('btnNext').disabled = current === slides.length - 1;

  const notes = slides[current].speaker_notes || '';
  document.getElementById('notesBox').textContent = notes.trim() ? notes : T_NO_NOTES;

  document.querySelectorAll('.thumb').forEach((el, i) => el.classList.toggle('active', i === current));
  const activeThumb = document.querySelector('.thumb.active');
  if (activeThumb) activeThumb.scrollIntoView({ behavior: 'smooth', inline: 'nearest', block: 'nearest' });
}

function startAutoPlay() {
  stopAutoPlay();
  playing = true;
  document.getElementById('btnPlay').textContent = '⏸ ' + T_PAUSE;
  const bar = document.getElementById('autoBar');
  function stepBar() {
    bar.style.transition = 'none';
    bar.style.width = '0%';
    void bar.offsetWidth;
    bar.style.transition = `width ${PLAY_MS}ms linear`;
    bar.style.width = '100%';
  }
  stepBar();
  playTimer = setInterval(() => {
    if (current < slides.length - 1) {
      go(current + 1, 1);
      stepBar();
    } else {
      stopAutoPlay();
    }
  }, PLAY_MS);
}

function stopAutoPlay() {
  playing = false;
  if (playTimer) { clearInterval(playTimer); playTimer = null; }
  document.getElementById('btnPlay').textContent = '▶ ' + T_PLAY;
  const bar = document.getElementById('autoBar');
  bar.style.transition = 'none';
  bar.style.width = '0%';
}

// Build thumbnail strip
const strip = document.getElementById('thumbStrip');
slides.forEach((s, i) => {
  const d = document.createElement('div');
  d.className = 'thumb';
  d.textContent = `${i + 1}. ${s.title || ''}`;
  d.onclick = () => { stopAutoPlay(); go(i, i >= current ? 1 : -1); };
  strip.appendChild(d);
});

document.getElementById('btnPrev').onclick = () => { stopAutoPlay(); go(current - 1, -1); };
document.getElementById('btnNext').onclick = () => { stopAutoPlay(); go(current + 1, 1); };
document.getElementById('edgePrev').onclick = () => { stopAutoPlay(); go(current - 1, -1); };
document.getElementById('edgeNext').onclick = () => { stopAutoPlay(); go(current + 1, 1); };
document.getElementById('btnPlay').onclick = () => playing ? stopAutoPlay() : startAutoPlay();
document.getElementById('btnNotes').onclick = () => document.getElementById('notesBox').classList.toggle('open');
document.getElementById('btnFs').onclick = () => {
  const el = document.getElementById('appRoot');
  if (!document.fullscreenElement) el.requestFullscreen?.();
  else document.exitFullscreen?.();
};

window.addEventListener('keydown', (e) => {
  if (e.key === 'ArrowRight' || e.key === 'PageDown' || e.key === ' ') { e.preventDefault(); stopAutoPlay(); go(current + 1, 1); }
  else if (e.key === 'ArrowLeft' || e.key === 'PageUp') { e.preventDefault(); stopAutoPlay(); go(current - 1, -1); }
  else if (e.key === 'Home') { stopAutoPlay(); go(0, -1); }
  else if (e.key === 'End') { stopAutoPlay(); go(slides.length - 1, 1); }
  else if (e.key === 'f' || e.key === 'F') { document.getElementById('btnFs').click(); }
});

go(0, 1);
</script>
</body>
</html>
""";
    }
}
