using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace LocalNotebook.Services;

/// Singleton WebView2 host for LaTeX formulas and interactive reports, released after 60s idle (Section 13.4).
public sealed class WebContentHost : IDisposable
{
    private WebView2? _webView;
    private readonly DispatcherTimer _idleTimer;

    public WebContentHost()
    {
        _idleTimer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(60) };
        _idleTimer.Tick += (_, _) => Release();
    }

    public async Task RenderHtmlAsync(Panel hostContainer, string sanitizedHtml)
    {
        _idleTimer.Stop();
        if (_webView == null)
        {
            _webView = new WebView2();
            await _webView.EnsureCoreWebView2Async();
            _webView.CoreWebView2.Settings.AreDefaultScriptDialogsEnabled = false;
            _webView.CoreWebView2.Settings.IsWebMessageEnabled = false;
            _webView.NavigationStarting += (_, args) =>
            {
                if (!args.Uri.StartsWith("data:") && !args.Uri.StartsWith("about:"))
                {
                    args.Cancel = true;
                }
            };
        }

        if (!hostContainer.Children.Contains(_webView))
        {
            hostContainer.Children.Clear();
            hostContainer.Children.Add(_webView);
        }

        _webView.NavigateToString(sanitizedHtml);
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
        }
    }

    public void Dispose() => Release();
}
