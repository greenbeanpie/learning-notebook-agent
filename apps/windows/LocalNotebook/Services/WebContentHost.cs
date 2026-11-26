using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.Web.WebView2.Core;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Security.Cryptography;
using System.Text;
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
/// One shared WebView2 for inline &amp; block LaTeX formulas, created on demand and released after 120 s
/// without use (G6). Coalesces concurrent formulas into a single atlas screenshot and crops each
/// expression directly into a cached WriteableBitmap so multi-formula documents render in ~1 frame.
/// </summary>
public sealed class WebContentHost : IDisposable
{
    public const int MaxFormulaLength = 2000;
    private const int MaxCacheEntries = 1000;
    private const int MaxBatchCount = 80;
    // Estimated atlas area (DIP^2) per batch; the atlas is at most 1560 wide so this keeps it near 1024 tall.
    private const double BatchAreaBudget = 1_100_000;
    private const int MaxAtlasWidth = 1600;
    private const int MaxAtlasHeight = 1024;
    private const int MaxDiskEntries = 6000;
    private const string DiskCacheVersion = "v1-katex-1.05em";
    private const int RenderTimeoutMs = 8000;
    private const string HostName = "katex.local";

    private sealed record PendingFormula(
        string Key,
        string Latex,
        bool DisplayMode,
        string DiskKey,
        TaskCompletionSource<MathRenderResult> Tcs);

    private sealed record CropToPersist(string DiskKey, byte[] Pixels, int PxW, int PxH, int W, int H, int Descent, bool Overflow);

    private static readonly string LocalRoot = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "LocalNotebook");
    private static readonly string DiskCacheDir = Path.Combine(LocalRoot, "formula-cache");
    private static readonly string UserDataDir = Path.Combine(LocalRoot, "webview-math");
    private CoreWebView2Environment? _env;
    private bool _diskPruned;

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
        _idleTimer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(120) };
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

    private int _uncachedSeen;

    /// <summary>Number of displayed formulas that were found in neither memory nor disk cache.</summary>
    public int UncachedSeen => _uncachedSeen;

    public void NoteUncached() => _uncachedSeen++;

    /// <summary>
    /// True when rendering this formula costs no WebView2 (memory cache or disk cache has it).
    /// </summary>
    public bool IsCached(string latex, bool displayMode, double scale)
    {
        if (_cache.ContainsKey(CacheKey(latex, displayMode))) return true;
        try
        {
            var dk = DiskKeyFor(latex, displayMode, scale);
            return File.Exists(Path.Combine(DiskCacheDir, dk + ".meta"))
                && File.Exists(Path.Combine(DiskCacheDir, dk + ".png"));
        }
        catch { return false; }
    }

    /// <summary>
    /// Starts WebView2 by rendering one tiny formula, after an idle delay. No-op if cancelled or disposed.
    /// </summary>
    public async Task PrewarmAsync(Panel hostContainer, CancellationToken ct)
    {
        try
        {
            await Task.Delay(1500, ct);
            if (ct.IsCancellationRequested || _disposed || _webView != null) return;
            var tcs = new TaskCompletionSource();
            if (!hostContainer.DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () =>
                {
                    if (ct.IsCancellationRequested || _disposed) { tcs.TrySetResult(); return; }
                    _ = RenderFormulaAsync(hostContainer, "x", false)
                        .ContinueWith(_ => tcs.TrySetResult(), TaskScheduler.Default);
                }))
                return;
            await tcs.Task;
        }
        catch (OperationCanceledException) { }
        catch { }
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
        var scale = hostContainer.XamlRoot?.RasterizationScale ?? 1.0;
        _pendingQueue.Enqueue(new PendingFormula(cacheKey, latex, displayMode, DiskKeyFor(latex, displayMode, scale), tcs));

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
                var drained = new List<PendingFormula>(_pendingQueue.Count);
                while (_pendingQueue.Count > 0)
                {
                    var item = _pendingQueue.Dequeue();
                    if (_cache.TryGetValue(item.Key, out var cached))
                    {
                        _inFlight.Remove(item.Key);
                        item.Tcs.TrySetResult(cached);
                        continue;
                    }
                    drained.Add(item);
                }
                if (drained.Count == 0) continue;

                // Disk cache first: a hit never starts WebView2.
                var diskHits = await Task.WhenAll(drained.Select(TryLoadFromDiskAsync));
                var misses = new List<PendingFormula>();
                for (var i = 0; i < drained.Count; i++)
                {
                    var item = drained[i];
                    var hit = diskHits[i];
                    if (hit == null) { misses.Add(item); continue; }
                    CachePut(item.Key, hit);
                    if (hit.Image != null) _bitmapCache[item.Key] = hit.Image;
                    _inFlight.Remove(item.Key);
                    item.Tcs.TrySetResult(hit);
                }

                var idx = 0;
                while (idx < misses.Count && !_disposed)
                {
                    var batch = new List<PendingFormula>();
                    double area = 0;
                    while (idx < misses.Count && batch.Count < MaxBatchCount)
                    {
                        var est = EstimateArea(misses[idx]);
                        if (batch.Count > 0 && area + est > BatchAreaBudget) break;
                        area += est;
                        batch.Add(misses[idx++]);
                    }
                    await RenderBatchInternalAsync(hostContainer, batch);
                }
                if (_disposed)
                {
                    foreach (var left in misses.Skip(idx))
                    {
                        _inFlight.Remove(left.Key);
                        left.Tcs.TrySetResult(new MathRenderResult(false, null, 0, 0, 0, false, L10n.T("math.disposed")));
                    }
                }
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

    private static double EstimateArea(PendingFormula f)
    {
        var w = Math.Min(1560, 24 + f.Latex.Length * (f.DisplayMode ? 9.0 : 7.5));
        var h = f.DisplayMode ? 64 : 36;
        return (w + 8) * (h + 8);
    }

    private static string DiskKeyFor(string latex, bool displayMode, double scale)
    {
        var raw = $"{DiskCacheVersion}|{(displayMode ? 'D' : 'I')}|{scale:0.##}|{latex}";
        return Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(raw)));
    }

    private static async Task<MathRenderResult?> TryLoadFromDiskAsync(PendingFormula item)
    {
        try
        {
            var metaPath = Path.Combine(DiskCacheDir, item.DiskKey + ".meta");
            var pngPath = Path.Combine(DiskCacheDir, item.DiskKey + ".png");
            if (!File.Exists(metaPath) || !File.Exists(pngPath)) return null;
            var parts = (await File.ReadAllTextAsync(metaPath)).Split(' ');
            if (parts.Length < 4
                || !int.TryParse(parts[0], out var w) || !int.TryParse(parts[1], out var h)
                || !int.TryParse(parts[2], out var descent)) return null;
            var bytes = await File.ReadAllBytesAsync(pngPath);
            using var mem = new InMemoryRandomAccessStream();
            await mem.WriteAsync(bytes.AsBuffer());
            mem.Seek(0);
            var bmp = new BitmapImage();
            await bmp.SetSourceAsync(mem);
            return new MathRenderResult(true, null, w, h, descent, parts[3] == "1", null, bmp);
        }
        catch
        {
            return null;
        }
    }

    private async Task PersistAsync(List<CropToPersist> crops)
    {
        try
        {
            Directory.CreateDirectory(DiskCacheDir);
            if (!_diskPruned)
            {
                _diskPruned = true;
                PruneDiskCache();
            }
            foreach (var c in crops)
            {
                using var ms = new InMemoryRandomAccessStream();
                var enc = await BitmapEncoder.CreateAsync(BitmapEncoder.PngEncoderId, ms);
                enc.SetPixelData(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied, (uint)c.PxW, (uint)c.PxH, 96, 96, c.Pixels);
                await enc.FlushAsync();
                ms.Seek(0);
                var buf = new byte[ms.Size];
                await ms.ReadAsync(buf.AsBuffer(), (uint)ms.Size, InputStreamOptions.None);
                var pngPath = Path.Combine(DiskCacheDir, c.DiskKey + ".png");
                var tmp = pngPath + ".tmp";
                await File.WriteAllBytesAsync(tmp, buf);
                File.Move(tmp, pngPath, true);
                // Meta is written last: its presence marks a complete entry.
                await File.WriteAllTextAsync(
                    Path.Combine(DiskCacheDir, c.DiskKey + ".meta"),
                    $"{c.W} {c.H} {c.Descent} {(c.Overflow ? 1 : 0)}");
            }
        }
        catch
        {
            // Disk cache is best effort.
        }
    }

    private static void PruneDiskCache()
    {
        try
        {
            var files = new DirectoryInfo(DiskCacheDir).GetFiles("*.meta");
            if (files.Length <= MaxDiskEntries) return;
            foreach (var f in files.OrderBy(f => f.LastWriteTimeUtc).Take(files.Length - MaxDiskEntries / 2))
            {
                f.Delete();
                var png = Path.ChangeExtension(f.FullName, ".png");
                if (File.Exists(png)) File.Delete(png);
            }
        }
        catch { }
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

    private string _batchStage = "";
    private readonly HashSet<string> _retried = new();

    /// <summary>
    /// Runs one batch under a watchdog: a hung WebView2 call must not freeze every later formula.
    /// A stalled batch drops the view (the next batch recreates it) and is retried once.
    /// </summary>
    private async Task RenderBatchInternalAsync(Panel hostContainer, List<PendingFormula> batch)
    {
        _batchStage = "start";
        var work = RenderBatchCoreAsync(hostContainer, batch);
        if (await Task.WhenAny(work, Task.Delay(RenderTimeoutMs * 3)) == work)
        {
            await work;
            return;
        }
        Diag($"batch of {batch.Count} stalled at stage '{_batchStage}'; resetting WebView2");
        Release();
        foreach (var item in batch)
        {
            if (_retried.Add(item.Key)) _pendingQueue.Enqueue(item);
            else
            {
                _inFlight.Remove(item.Key);
                item.Tcs.TrySetResult(new MathRenderResult(false, null, 0, 0, 0, false, L10n.T("math.timeout")));
            }
        }
    }

    private async Task RenderBatchCoreAsync(Panel hostContainer, List<PendingFormula> batch)
    {
        try
        {
            _idleTimer.Stop();
            _batchStage = "ensure";
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
            _batchStage = "measure";
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

            // Cap atlas height: if the atlas is too tall, render only the first half now and requeue the rest.
            while (measure.AtlasHeight > MaxAtlasHeight && batch.Count > 1)
            {
                var keep = batch.Count / 2;
                foreach (var extra in batch.Skip(keep)) _pendingQueue.Enqueue(extra);
                batch = batch.Take(keep).ToList();
                inputList.RemoveRange(keep, inputList.Count - keep);
                jsonArg = JsonSerializer.Serialize(inputList, Interop.LnaJsonContext.Default.ListMathBatchItemInputDto);
                var rawAgain = await _webView.CoreWebView2.ExecuteScriptAsync($"window.lnaRenderBatch({jsonArg})").AsTask();
                var innerAgain = JsonSerializer.Deserialize(rawAgain, Interop.LnaJsonContext.Default.String);
                var again = string.IsNullOrEmpty(innerAgain) ? null : JsonSerializer.Deserialize(innerAgain, Interop.LnaJsonContext.Default.MathBatchMeasureDto);
                if (again == null || again.Items.Count != batch.Count)
                {
                    ResolveBatchError(batch, L10n.T("math.empty_result"));
                    return;
                }
                measure = again;
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

            var atlasW = Math.Clamp(measure.AtlasWidth, 4, MaxAtlasWidth);
            var atlasH = Math.Clamp(measure.AtlasHeight, 4, MaxAtlasHeight);
            _webView.Width = atlasW;
            _webView.Height = atlasH;
            _webView.UpdateLayout();

            // Wait for WebView2 viewport resize, font loading, and two compositor rAF frames
            // before measuring final bounding boxes and capturing the atlas screenshot.
            _batchStage = "paint";
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

            _batchStage = "remeasure";
            var remeasureRaw = await _webView.CoreWebView2.ExecuteScriptAsync("window.lnaMeasureBatch()").AsTask();
            var remeasureInner = JsonSerializer.Deserialize(remeasureRaw, Interop.LnaJsonContext.Default.String);
            if (!string.IsNullOrEmpty(remeasureInner))
            {
                var freshMeasure = JsonSerializer.Deserialize(remeasureInner, Interop.LnaJsonContext.Default.MathBatchMeasureDto);
                if (freshMeasure != null && freshMeasure.Items.Count == batch.Count)
                {
                    measure = freshMeasure;
                    var newW = Math.Clamp(measure.AtlasWidth, 4, MaxAtlasWidth);
                    var newH = Math.Clamp(measure.AtlasHeight, 4, MaxAtlasHeight);
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

            _batchStage = "capture";
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

            var toPersist = new List<CropToPersist>();
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

                var cropped = new byte[pxW * pxH * 4];
                var rowBytes = pxW * 4;
                for (var rowIdx = 0; rowIdx < pxH; rowIdx++)
                {
                    System.Buffer.BlockCopy(atlasPixels, (pxY + rowIdx) * atlasStride + pxX * 4, cropped, rowIdx * rowBytes, rowBytes);
                }
                var wb = new WriteableBitmap(pxW, pxH);
                using (var dst = wb.PixelBuffer.AsStream())
                {
                    dst.Write(cropped, 0, cropped.Length);
                }
                wb.Invalidate();

                if (!m.Overflow) toPersist.Add(new CropToPersist(item.DiskKey, cropped, pxW, pxH, w, h, descent, m.Overflow));
                var okRes = new MathRenderResult(true, null, w, h, descent, m.Overflow, null, wb);
                CachePut(item.Key, okRes);
                _bitmapCache[item.Key] = wb;
                _inFlight.Remove(item.Key);
                item.Tcs.TrySetResult(okRes);
            }
            if (toPersist.Count > 0) _ = PersistAsync(toPersist);
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

            // Software rendering and a private profile keep the formula browser off the GPU (no dGPU wake-up).
            if (_env == null)
            {
                var envTask = CoreWebView2Environment.CreateWithOptionsAsync(
                    null,
                    UserDataDir,
                    new CoreWebView2EnvironmentOptions { AdditionalBrowserArguments = "--disable-gpu" }).AsTask();
                if (await Task.WhenAny(envTask, Task.Delay(RenderTimeoutMs)) != envTask)
                {
                    Diag("environment creation timed out");
                    return false;
                }
                _env = await envTask;
            }
            var ensureTask = view.EnsureCoreWebView2Async(_env).AsTask();
            if (await Task.WhenAny(ensureTask, Task.Delay(RenderTimeoutMs)) != ensureTask)
            {
                Diag("WebView2 creation timed out");
                return false;
            }
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
            {
                Diag("view.html did not load");
                return false;
            }

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
        catch (Exception ex)
        {
            Diag("EnsureReady failed: " + ex);
            _pageReady = false;
            return false;
        }
    }

    /// <summary>Appends a line to logs/math.log; formula rendering otherwise fails silently.</summary>
    internal static void Diag(string message)
    {
        try
        {
            var dir = Path.Combine(LocalRoot, "logs");
            Directory.CreateDirectory(dir);
            File.AppendAllText(Path.Combine(dir, "math.log"), $"{DateTime.Now:O} {message}{Environment.NewLine}");
        }
        catch { }
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
