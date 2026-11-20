using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Text.Json;

namespace LocalNotebook.Services;

/// <summary>
/// One shared WebView2 for LaTeX formulas, created on demand and released after 60 s without use (G6).
/// The page is local (KaTeX bundled under Assets/katex), has a CSP that forbids network access and
/// inline script, receives text only (never HTML), and every navigation away from the local host is cancelled.
/// </summary>
public sealed class WebContentHost : IDisposable
{
    private const string HostName = "katex.local";
    private WebView2? _webView;
    private bool _pageReady;
    private readonly DispatcherTimer _idleTimer;

    public WebContentHost()
    {
        _idleTimer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(60) };
        _idleTimer.Tick += (_, _) => Release();
    }

    public bool IsLoaded => _webView != null;

    /// <summary>Shows <paramref name="text"/> with its formulas typeset inside <paramref name="hostContainer"/>.</summary>
    public async Task RenderMathAsync(Panel hostContainer, string text)
    {
        _idleTimer.Stop();
        if (_webView == null)
        {
            _pageReady = false;
            var view = new WebView2();
            _webView = view;
            hostContainer.Children.Clear();
            hostContainer.Children.Add(view);
            await view.EnsureCoreWebView2Async();
            var core = view.CoreWebView2;
            core.Settings.AreDefaultScriptDialogsEnabled = false;
            core.Settings.IsWebMessageEnabled = false;
            core.Settings.AreDevToolsEnabled = false;
            core.Settings.AreDefaultContextMenusEnabled = false;
            core.Settings.IsStatusBarEnabled = false;
            var folder = Path.Combine(AppContext.BaseDirectory, "Assets", "katex");
            core.SetVirtualHostNameToFolderMapping(HostName, folder,
                Microsoft.Web.WebView2.Core.CoreWebView2HostResourceAccessKind.DenyCors);
            view.NavigationStarting += (_, args) =>
            {
                if (!args.Uri.StartsWith($"https://{HostName}/", StringComparison.Ordinal)) args.Cancel = true;
            };
            core.NewWindowRequested += (_, args) => args.Handled = true;
            var loaded = new TaskCompletionSource();
            view.NavigationCompleted += (_, _) => { _pageReady = true; loaded.TrySetResult(); };
            view.Source = new Uri($"https://{HostName}/view.html");
            await Task.WhenAny(loaded.Task, Task.Delay(8000));
        }
        else if (!hostContainer.Children.Contains(_webView))
        {
            hostContainer.Children.Clear();
            hostContainer.Children.Add(_webView);
        }

        if (_pageReady)
        {
            // The text is passed as a JSON string literal, so it can never become script.
            await _webView.CoreWebView2.ExecuteScriptAsync($"window.lnaRender({JsonSerializer.Serialize(text, Interop.LnaJsonContext.Default.String)})");
        }
        _idleTimer.Start();
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

    public void Dispose() => Release();
}
