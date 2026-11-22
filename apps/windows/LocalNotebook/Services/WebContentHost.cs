using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.Web.WebView2.Core;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Text.Json;
using Windows.Storage.Streams;

namespace LocalNotebook.Services;

public sealed record MathRenderResult(
    bool Success,
    byte[]? PngBytes,
    int Width,
    int Height,
    int Descent,
    bool Overflow,
    string? Error);

/// <summary>
/// One shared WebView2 for inline & block LaTeX formulas, created on demand and released after 60 s
/// without use (G6). Renders each formula once into a cached transparent PNG so chat lists never spawn
/// per-message WebViews. Enforces length, timeout, and error fallbacks.
/// </summary>
public sealed class WebContentHost : IDisposable
{
    public const int MaxFormulaLength = 2000;
    private const int MaxCacheEntries = 1000;
    private const int RenderTimeoutMs = 5000;
    private const string HostName = "katex.local";

    private WebView2? _webView;
    private bool _pageReady;
    private bool _disposed;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private readonly Dictionary<string, MathRenderResult> _cache = new();
    private readonly Dictionary<string, BitmapImage> _bitmapCache = new();
    private readonly LinkedList<string> _cacheOrder = new();
    private readonly DispatcherTimer _idleTimer;

    public WebContentHost()
    {
        _idleTimer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(60) };
        _idleTimer.Tick += (_, _) => Release();
    }

    public bool IsLoaded => _webView != null;

    private static string CacheKey(string latex, bool displayMode) => $"{(displayMode ? 'D' : 'I')}:{latex}";

    /// <summary>
    /// Looks up a previously rendered and decoded formula bitmap synchronously on the UI thread.
    /// </summary>
    public bool TryGetCachedBitmap(
        string latex,
        bool displayMode,
        out MathRenderResult result,
        out BitmapImage? bitmap)
    {
        var key = CacheKey(latex, displayMode);
        if (_cache.TryGetValue(key, out var cached))
        {
            result = cached;
            return _bitmapCache.TryGetValue(key, out bitmap);
        }
        result = new MathRenderResult(false, null, 0, 0, 0, false, null);
        bitmap = null;
        return false;
    }

    /// <summary>
    /// Stores a decoded formula <see cref="BitmapImage"/> for synchronous reuse across re-renders.
    /// </summary>
    public void CacheBitmap(string latex, bool displayMode, BitmapImage bitmap)
    {
        var key = CacheKey(latex, displayMode);
        if (_cache.ContainsKey(key))
        {
            _bitmapCache[key] = bitmap;
        }
    }

    /// <summary>
    /// Renders a single LaTeX expression to a cached transparent PNG.
    /// Must be called on the UI thread (WinUI DispatcherQueue).
    /// </summary>
    public async Task<MathRenderResult> RenderFormulaAsync(Panel hostContainer, string latex, bool displayMode)
    {
        if (_disposed)
            return new MathRenderResult(false, null, 0, 0, 0, false, "公式渲染器已释放");
        if (string.IsNullOrWhiteSpace(latex))
            return new MathRenderResult(false, null, 0, 0, 0, false, "公式内容为空");
        if (latex.Length > MaxFormulaLength)
            return new MathRenderResult(false, null, 0, 0, 0, false, $"公式长度超过 {MaxFormulaLength} 字符上限");

        var cacheKey = CacheKey(latex, displayMode);
        if (_cache.TryGetValue(cacheKey, out var cached))
            return cached;

        await _gate.WaitAsync();
        try
        {
            if (_cache.TryGetValue(cacheKey, out cached))
                return cached;

            _idleTimer.Stop();
            var ready = await EnsureReadyAsync(hostContainer);
            if (!ready || _webView?.CoreWebView2 == null)
            {
                return new MathRenderResult(false, null, 0, 0, 0, false, "WebView2 公式引擎未就绪");
            }

            var jsonArg = JsonSerializer.Serialize(latex, Interop.LnaJsonContext.Default.String);
            var dmArg = displayMode ? "true" : "false";
            var evalTask = _webView.CoreWebView2.ExecuteScriptAsync($"window.lnaRenderFormula({jsonArg}, {dmArg})").AsTask();
            if (await Task.WhenAny(evalTask, Task.Delay(RenderTimeoutMs)) != evalTask)
            {
                return new MathRenderResult(false, null, 0, 0, 0, false, "公式渲染超时");
            }

            var rawJsonString = await evalTask;
            // ExecuteScriptAsync returns a JSON-encoded string literal.
            var innerJson = JsonSerializer.Deserialize(rawJsonString, Interop.LnaJsonContext.Default.String);
            if (string.IsNullOrEmpty(innerJson))
            {
                return new MathRenderResult(false, null, 0, 0, 0, false, "公式测量返回空结果");
            }

            var measure = JsonSerializer.Deserialize(innerJson, Interop.LnaJsonContext.Default.MathMeasureDto);
            if (measure == null || !measure.Ok)
            {
                var err = measure?.Error ?? "KaTeX 解析失败";
                var failRes = new MathRenderResult(false, null, 0, 0, 0, false, err);
                CachePut(cacheKey, failRes);
                return failRes;
            }

            var w = Math.Clamp(measure.Width, 4, 1600);
            var h = Math.Clamp(measure.Height, 4, 800);
            var descent = Math.Clamp(measure.Descent, 0, h);
            _webView.Width = w;
            _webView.Height = h;
            _webView.UpdateLayout();
            // Allow one compositor frame for the resized WebView2 viewport to repaint.
            await Task.Delay(24);

            using var mem = new InMemoryRandomAccessStream();
            var capTask = _webView.CoreWebView2.CapturePreviewAsync(CoreWebView2CapturePreviewImageFormat.Png, mem).AsTask();
            if (await Task.WhenAny(capTask, Task.Delay(RenderTimeoutMs)) != capTask)
            {
                return new MathRenderResult(false, null, 0, 0, 0, false, "公式截图超时");
            }
            await capTask;

            var size = (uint)mem.Size;
            if (size == 0)
            {
                return new MathRenderResult(false, null, 0, 0, 0, false, "公式截图为空");
            }
            var bytes = new byte[size];
            mem.Seek(0);
            await mem.ReadAsync(bytes.AsBuffer(), size, InputStreamOptions.None);

            var okRes = new MathRenderResult(true, bytes, w, h, descent, measure.Overflow, null);
            CachePut(cacheKey, okRes);
            return okRes;
        }
        catch (Exception ex)
        {
            return new MathRenderResult(false, null, 0, 0, 0, false, $"公式渲染异常：{ex.Message}");
        }
        finally
        {
            _idleTimer.Start();
            _gate.Release();
        }
    }

    private async Task<bool> EnsureReadyAsync(Panel hostContainer)
    {
        if (_webView != null && _pageReady)
        {
            if (!hostContainer.Children.Contains(_webView))
            {
                hostContainer.Children.Clear();
                hostContainer.Children.Add(_webView);
            }
            return true;
        }

        try
        {
            if (_webView != null)
            {
                if (_webView.Parent is Panel oldP) oldP.Children.Remove(_webView);
                _webView.Close();
                _webView = null;
            }

            _pageReady = false;
            var view = new WebView2
            {
                Width = 400,
                Height = 120,
                DefaultBackgroundColor = Windows.UI.Color.FromArgb(0, 0, 0, 0),
            };
            _webView = view;
            hostContainer.Children.Clear();
            hostContainer.Children.Add(view);

            var ensureTask = view.EnsureCoreWebView2Async().AsTask();
            if (await Task.WhenAny(ensureTask, Task.Delay(RenderTimeoutMs)) != ensureTask)
                return false;
            await ensureTask;

            var core = view.CoreWebView2;
            core.Settings.AreDefaultScriptDialogsEnabled = false;
            core.Settings.IsWebMessageEnabled = false;
            core.Settings.AreDevToolsEnabled = false;
            core.Settings.AreDefaultContextMenusEnabled = false;
            core.Settings.IsStatusBarEnabled = false;

            var folder = Path.Combine(AppContext.BaseDirectory, "Assets", "katex");
            core.SetVirtualHostNameToFolderMapping(
                HostName,
                folder,
                CoreWebView2HostResourceAccessKind.DenyCors);

            view.NavigationStarting += (_, args) =>
            {
                if (!args.Uri.StartsWith($"https://{HostName}/", StringComparison.Ordinal))
                    args.Cancel = true;
            };
            core.NewWindowRequested += (_, args) => args.Handled = true;

            var loaded = new TaskCompletionSource<bool>();
            view.NavigationCompleted += (_, args) =>
            {
                _pageReady = args.IsSuccess;
                loaded.TrySetResult(args.IsSuccess);
            };
            view.Source = new Uri($"https://{HostName}/view.html");
            var completed = await Task.WhenAny(loaded.Task, Task.Delay(RenderTimeoutMs));
            return completed == loaded.Task && await loaded.Task;
        }
        catch
        {
            _pageReady = false;
            return false;
        }
    }

    private void CachePut(string key, MathRenderResult value)
    {
        if (_cache.ContainsKey(key)) return;
        if (_cacheOrder.Count >= MaxCacheEntries && _cacheOrder.First is { } oldest)
        {
            _cache.Remove(oldest.Value);
            _bitmapCache.Remove(oldest.Value);
            _cacheOrder.RemoveFirst();
        }
        _cache[key] = value;
        _cacheOrder.AddLast(key);
    }

    public void Release()
    {
        _idleTimer.Stop();
        if (_webView != null)
        {
            if (_webView.Parent is Panel p) p.Children.Remove(_webView);
            _webView.Close();
            _webView = null;
            _pageReady = false;
        }
    }

    public void Dispose()
    {
        _disposed = true;
        Release();
        _bitmapCache.Clear();
        _gate.Dispose();
    }
}
