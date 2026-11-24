using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.Web.WebView2.Core;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Text.Json;
using Windows.Graphics.Imaging;
using Windows.Storage.Streams;

namespace LocalNotebook.Services;

public sealed record MathRenderResult(
    bool Success,
    byte[]? PngBytes,
    int Width,
    int Height,
    int Descent,
    bool Overflow,
    string? Error,
    ImageSource? Image = null);

/// <summary>
/// One shared WebView2 for inline &amp; block LaTeX formulas, created on demand and released after 60 s
/// without use (G6). Coalesces concurrent formulas into a single atlas screenshot and crops each
/// expression directly into a cached WriteableBitmap so multi-formula documents render in ~1 frame.
/// </summary>
public sealed class WebContentHost : IDisposable
{
    public const int MaxFormulaLength = 2000;
    private const int MaxCacheEntries = 1000;
    private const int MaxBatchSize = 40;
    private const int RenderTimeoutMs = 8000;
    private const string HostName = "katex.local";

    private sealed record PendingFormula(
        string Key,
        string Latex,
        bool DisplayMode,
        TaskCompletionSource<MathRenderResult> Tcs);

    private WebView2? _webView;
    private bool _pageReady;
    private bool _disposed;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private readonly Dictionary<string, MathRenderResult> _cache = new();
    private readonly Dictionary<string, ImageSource> _bitmapCache = new();
    private readonly LinkedList<string> _cacheOrder = new();
    private readonly Queue<PendingFormula> _pendingQueue = new();
    private readonly Dictionary<string, TaskCompletionSource<MathRenderResult>> _inFlight = new();
    private bool _batchScheduled;
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
        out ImageSource? bitmap)
    {
        var key = CacheKey(latex, displayMode);
        if (_cache.TryGetValue(key, out var cached))
        {
            result = cached;
            if (cached.Image != null)
            {
                bitmap = cached.Image;
                return true;
            }
            return _bitmapCache.TryGetValue(key, out bitmap);
        }
        result = new MathRenderResult(false, null, 0, 0, 0, false, null);
        bitmap = null;
        return false;
    }

    /// <summary>
    /// Stores a decoded formula <see cref="ImageSource"/> for synchronous reuse across re-renders.
    /// </summary>
    public void CacheBitmap(string latex, bool displayMode, ImageSource bitmap)
    {
        var key = CacheKey(latex, displayMode);
        if (_cache.ContainsKey(key))
        {
            _bitmapCache[key] = bitmap;
        }
    }

    /// <summary>
    /// Queues a LaTeX expression for coalesced batch rendering to a cached <see cref="WriteableBitmap"/>.
    /// Must be called on the UI thread (WinUI DispatcherQueue).
    /// </summary>
    public Task<MathRenderResult> RenderFormulaAsync(Panel hostContainer, string latex, bool displayMode)
    {
        if (_disposed)
            return Task.FromResult(new MathRenderResult(false, null, 0, 0, 0, false, L10n.T("math.disposed")));
        if (string.IsNullOrWhiteSpace(latex))
            return Task.FromResult(new MathRenderResult(false, null, 0, 0, 0, false, L10n.T("math.empty")));
        if (latex.Length > MaxFormulaLength)
            return Task.FromResult(new MathRenderResult(false, null, 0, 0, 0, false, L10n.Fmt("math.too_long", MaxFormulaLength)));

        var cacheKey = CacheKey(latex, displayMode);
        if (_cache.TryGetValue(cacheKey, out var cached))
            return Task.FromResult(cached);

        if (_inFlight.TryGetValue(cacheKey, out var existingTcs))
            return existingTcs.Task;

        var tcs = new TaskCompletionSource<MathRenderResult>(TaskCreationOptions.RunContinuationsAsynchronously);
        _inFlight[cacheKey] = tcs;
        _pendingQueue.Enqueue(new PendingFormula(cacheKey, latex, displayMode, tcs));

        if (!_batchScheduled)
        {
            _batchScheduled = true;
            if (!hostContainer.DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Normal, () => _ = ProcessQueueAsync(hostContainer)))
            {
                _ = ProcessQueueAsync(hostContainer);
            }
        }

        return tcs.Task;
    }

    private async Task ProcessQueueAsync(Panel hostContainer)
    {
        _batchScheduled = false;
        if (_disposed)
        {
            FailRemainingQueue(L10n.T("math.disposed"));
            return;
        }

        await _gate.WaitAsync();
        try
        {
            while (_pendingQueue.Count > 0 && !_disposed)
            {
                var batch = new List<PendingFormula>(Math.Min(MaxBatchSize, _pendingQueue.Count));
                while (batch.Count < MaxBatchSize && _pendingQueue.Count > 0)
                {
                    var item = _pendingQueue.Dequeue();
                    if (_cache.TryGetValue(item.Key, out var cached))
                    {
                        _inFlight.Remove(item.Key);
                        item.Tcs.TrySetResult(cached);
                        continue;
                    }
                    batch.Add(item);
                }
                if (batch.Count == 0) continue;

                await RenderBatchInternalAsync(hostContainer, batch);
            }
        }
        finally
        {
            _idleTimer.Start();
            _gate.Release();
            if (_pendingQueue.Count > 0 && !_batchScheduled && !_disposed)
            {
                _batchScheduled = true;
                hostContainer.DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Normal, () => _ = ProcessQueueAsync(hostContainer));
            }
        }
    }

    private void FailRemainingQueue(string error)
    {
        var fail = new MathRenderResult(false, null, 0, 0, 0, false, error);
        while (_pendingQueue.Count > 0)
        {
            var item = _pendingQueue.Dequeue();
            _inFlight.Remove(item.Key);
            item.Tcs.TrySetResult(fail);
        }
    }

    private void ResolveBatchError(List<PendingFormula> batch, string error, bool cacheFailure = false)
    {
        var fail = new MathRenderResult(false, null, 0, 0, 0, false, error);
        foreach (var item in batch)
        {
            if (cacheFailure) CachePut(item.Key, fail);
            _inFlight.Remove(item.Key);
            item.Tcs.TrySetResult(fail);
        }
    }

    private async Task RenderBatchInternalAsync(Panel hostContainer, List<PendingFormula> batch)
    {
        try
        {
            _idleTimer.Stop();
            var ready = await EnsureReadyAsync(hostContainer);
            if (!ready || _webView?.CoreWebView2 == null)
            {
                ResolveBatchError(batch, L10n.T("math.not_ready"));
                return;
            }

            var inputList = new List<Interop.MathBatchItemInputDto>(batch.Count);
            foreach (var item in batch)
            {
                inputList.Add(new Interop.MathBatchItemInputDto
                {
                    Latex = item.Latex,
                    DisplayMode = item.DisplayMode,
                });
            }

            var jsonArg = JsonSerializer.Serialize(inputList, Interop.LnaJsonContext.Default.ListMathBatchItemInputDto);
            var evalTask = _webView.CoreWebView2.ExecuteScriptAsync($"window.lnaRenderBatch({jsonArg})").AsTask();
            if (await Task.WhenAny(evalTask, Task.Delay(RenderTimeoutMs)) != evalTask)
            {
                ResolveBatchError(batch, L10n.T("math.timeout"));
                return;
            }

            var rawJsonString = await evalTask;
            var innerJson = JsonSerializer.Deserialize(rawJsonString, Interop.LnaJsonContext.Default.String);
            if (string.IsNullOrEmpty(innerJson))
            {
                ResolveBatchError(batch, L10n.T("math.empty_result"));
                return;
            }

            var measure = JsonSerializer.Deserialize(innerJson, Interop.LnaJsonContext.Default.MathBatchMeasureDto);
            if (measure == null || measure.Items.Count != batch.Count)
            {
                ResolveBatchError(batch, L10n.T("math.empty_result"));
                return;
            }

            // If every item in the batch failed KaTeX parsing, skip screenshot capture completely.
            var hasAnyOk = false;
            foreach (var m in measure.Items)
            {
                if (m.Ok) { hasAnyOk = true; break; }
            }
            if (!hasAnyOk)
            {
                for (var i = 0; i < batch.Count; i++)
                {
                    var item = batch[i];
                    var err = measure.Items[i].Error ?? L10n.T("math.parse_failed");
                    var failRes = new MathRenderResult(false, null, 0, 0, 0, false, err);
                    CachePut(item.Key, failRes);
                    _inFlight.Remove(item.Key);
                    item.Tcs.TrySetResult(failRes);
                }
                return;
            }

            var atlasW = Math.Clamp(measure.AtlasWidth, 4, 1600);
            var atlasH = Math.Clamp(measure.AtlasHeight, 4, 3200);
            _webView.Width = atlasW;
            _webView.Height = atlasH;
            _webView.UpdateLayout();

            // Wait for WebView2 viewport resize, font loading, and two compositor rAF frames
            // before measuring final bounding boxes and capturing the atlas screenshot.
            await _webView.CoreWebView2.ExecuteScriptAsync("window.lnaStartPaintWait()").AsTask();
            for (var attempt = 0; attempt < 15; attempt++)
            {
                await Task.Delay(16);
                var readyRaw = await _webView.CoreWebView2
                    .ExecuteScriptAsync($"window.lnaIsReadyForCapture({atlasW}, {atlasH})")
                    .AsTask();
                if (string.Equals(readyRaw, "true", StringComparison.Ordinal))
                    break;
            }

            var remeasureRaw = await _webView.CoreWebView2.ExecuteScriptAsync("window.lnaMeasureBatch()").AsTask();
            var remeasureInner = JsonSerializer.Deserialize(remeasureRaw, Interop.LnaJsonContext.Default.String);
            if (!string.IsNullOrEmpty(remeasureInner))
            {
                var freshMeasure = JsonSerializer.Deserialize(remeasureInner, Interop.LnaJsonContext.Default.MathBatchMeasureDto);
                if (freshMeasure != null && freshMeasure.Items.Count == batch.Count)
                {
                    measure = freshMeasure;
                    var newW = Math.Clamp(measure.AtlasWidth, 4, 1600);
                    var newH = Math.Clamp(measure.AtlasHeight, 4, 3200);
                    if (newW != atlasW || newH != atlasH)
                    {
                        atlasW = newW;
                        atlasH = newH;
                        _webView.Width = atlasW;
                        _webView.Height = atlasH;
                        _webView.UpdateLayout();
                        await Task.Delay(32);
                    }
                }
            }

            using var mem = new InMemoryRandomAccessStream();
            var capTask = _webView.CoreWebView2.CapturePreviewAsync(CoreWebView2CapturePreviewImageFormat.Png, mem).AsTask();
            if (await Task.WhenAny(capTask, Task.Delay(RenderTimeoutMs)) != capTask)
            {
                ResolveBatchError(batch, L10n.T("math.screenshot_timeout"));
                return;
            }
            await capTask;

            if (mem.Size == 0)
            {
                ResolveBatchError(batch, L10n.T("math.screenshot_empty"));
                return;
            }

            mem.Seek(0);
            var decoder = await BitmapDecoder.CreateAsync(mem);
            var pixelData = await decoder.GetPixelDataAsync(
                BitmapPixelFormat.Bgra8,
                BitmapAlphaMode.Premultiplied,
                new BitmapTransform(),
                ExifOrientationMode.IgnoreExifOrientation,
                ColorManagementMode.DoNotColorManage);
            var atlasPixels = pixelData.DetachPixelData();
            var atlasPxW = (int)decoder.PixelWidth;
            var atlasPxH = (int)decoder.PixelHeight;
            var atlasStride = atlasPxW * 4;
            var scaleX = (double)atlasPxW / atlasW;
            var scaleY = (double)atlasPxH / atlasH;

            for (var i = 0; i < batch.Count; i++)
            {
                var item = batch[i];
                var m = measure.Items[i];
                if (!m.Ok)
                {
                    var err = m.Error ?? L10n.T("math.parse_failed");
                    var failRes = new MathRenderResult(false, null, 0, 0, 0, false, err);
                    CachePut(item.Key, failRes);
                    _inFlight.Remove(item.Key);
                    item.Tcs.TrySetResult(failRes);
                    continue;
                }

                var w = Math.Clamp(m.Width, 4, 1600);
                var h = Math.Clamp(m.Height, 4, 800);
                var descent = Math.Clamp(m.Descent, 0, h);

                var pxX = Math.Clamp((int)Math.Round(m.X * scaleX), 0, Math.Max(0, atlasPxW - 1));
                var pxY = Math.Clamp((int)Math.Round(m.Y * scaleY), 0, Math.Max(0, atlasPxH - 1));
                var pxW = Math.Clamp((int)Math.Round(w * scaleX), 1, Math.Max(1, atlasPxW - pxX));
                var pxH = Math.Clamp((int)Math.Round(h * scaleY), 1, Math.Max(1, atlasPxH - pxY));

                var wb = new WriteableBitmap(pxW, pxH);
                using (var dst = wb.PixelBuffer.AsStream())
                {
                    var rowBytes = pxW * 4;
                    for (var rowIdx = 0; rowIdx < pxH; rowIdx++)
                    {
                        var srcOffset = (pxY + rowIdx) * atlasStride + pxX * 4;
                        dst.Write(atlasPixels, srcOffset, rowBytes);
                    }
                }
                wb.Invalidate();

                var okRes = new MathRenderResult(true, null, w, h, descent, m.Overflow, null, wb);
                CachePut(item.Key, okRes);
                _bitmapCache[item.Key] = wb;
                _inFlight.Remove(item.Key);
                item.Tcs.TrySetResult(okRes);
            }
        }
        catch (Exception ex)
        {
            ResolveBatchError(batch, L10n.Fmt("math.render_exception", ex.Message));
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
            if (completed != loaded.Task || !await loaded.Task)
                return false;

            // Preload all KaTeX @font-face fonts so the first batch never measures fallback glyphs.
            var warmupRaw = await core.ExecuteScriptAsync("window.lnaWarmupFonts()").AsTask();
            if (!string.Equals(warmupRaw, "true", StringComparison.Ordinal))
            {
                for (var i = 0; i < 100; i++)
                {
                    await Task.Delay(15);
                    var fontsRaw = await core.ExecuteScriptAsync("window.__lnaFontsReady === true").AsTask();
                    if (string.Equals(fontsRaw, "true", StringComparison.Ordinal))
                        break;
                }
            }
            return true;
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
        FailRemainingQueue(L10n.T("math.disposed"));
        Release();
        _bitmapCache.Clear();
        _gate.Dispose();
    }
}
