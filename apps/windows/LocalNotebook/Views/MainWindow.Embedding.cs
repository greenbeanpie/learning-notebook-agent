using LocalNotebook.Interop;
using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

public sealed partial class MainWindow
{
    private ProgressBar? _embProgress;
    private TextBlock? _embStatus;
    private Button? _embDownload;
    private Button? _embCancel;

    /// <summary>Semantic search set-up: shows the index state and downloads the default multilingual model.</summary>
    private async void OnOpenEmbeddingClick(object sender, RoutedEventArgs e)
    {
        var status = new TextBlock { TextWrapping = TextWrapping.Wrap };
        var progress = new ProgressBar { Maximum = 100, Visibility = Visibility.Collapsed };
        var download = new Button { Content = L10n.T("emb.download_enable") };
        var cancel = new Button { Content = L10n.T("emb.cancel_download"), Visibility = Visibility.Collapsed };
        var rebuild = new Button { Content = L10n.T("emb.rebuild") };
        _embProgress = progress; _embStatus = status; _embDownload = download; _embCancel = cancel;

        var panel = new StackPanel { Spacing = 10, MinWidth = 460, MaxWidth = 520 };
        panel.Children.Add(Line(L10n.T("emb.desc")));
        panel.Children.Add(status);
        panel.Children.Add(progress);
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        row.Children.Add(download);
        row.Children.Add(cancel);
        row.Children.Add(rebuild);
        panel.Children.Add(row);
        panel.Children.Add(new TextBlock { Text = L10n.T("emb.model_note"), TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.7 });

        async Task RefreshAsync()
        {
            var es = await CoreAsync("embedding.status", Args(), L10n.T("emb.op_status"));
            var dm = await CoreAsync("embedding.default_model", Args(), L10n.T("emb.op_default"));
            var dl = await CoreAsync("embedding.download_status", Args(), L10n.T("emb.op_dl_status"));
            if (es is not { } s || dm is not { } d) return;
            var running = dl is { } x && Bool(x, "running");
            var activeModel = Obj(s, "active_model");
            var total = Int(s, "total_chunks");
            var done = Int(s, "embedded_chunks");
            var mb = Int(d, "total_bytes") / 1_000_000;
            if (activeModel.ValueKind == JsonValueKind.Object)
                status.Text = L10n.Fmt("emb.status_active", Str(activeModel, "model_name"), Str(activeModel, "provider"), done, total);
            else
                status.Text = Bool(d, "installed") ? L10n.T("emb.status_installed") : L10n.Fmt("emb.status_not_enabled", mb);
            download.Content = Bool(d, "installed") ? L10n.T("emb.enable_installed") : L10n.Fmt("emb.download_size", mb);
            download.IsEnabled = !running;
            cancel.Visibility = running ? Visibility.Visible : Visibility.Collapsed;
            progress.Visibility = running ? Visibility.Visible : Visibility.Collapsed;
            rebuild.IsEnabled = activeModel.ValueKind == JsonValueKind.Object;
        }

        download.Click += async (_, _) =>
        {
            if (await CoreAsync("embedding.download", Args(), L10n.T("emb.op_download")) != null)
            {
                progress.Value = 0;
                await RefreshAsync();
            }
        };
        cancel.Click += async (_, _) => { await CoreAsync("embedding.download_cancel", Args(), L10n.T("emb.cancel_download")); };
        rebuild.Click += async (_, _) =>
        {
            if (!await ConfirmAsync(L10n.T("emb.rebuild"), L10n.T("emb.rebuild_confirm"), L10n.T("emb.rebuild"))) return;
            var r = await CoreAsync("embedding.rebuild", Args(("force", true)), L10n.T("emb.rebuild"));
            if (r is { } rr) status.Text = L10n.Fmt("emb.queued_rebuild", Int(rr, "sources_queued"));
        };

        await RefreshAsync();
        var dlg = new ContentDialog { Title = L10n.T("settings.tab_embedding"), Content = panel, CloseButtonText = L10n.T("common.close"), XamlRoot = Content.XamlRoot };
        _embRefresh = RefreshAsync;
        await dlg.ShowAsync();
        _embRefresh = null;
        _embProgress = null; _embStatus = null; _embDownload = null; _embCancel = null;
    }

    private Func<Task>? _embRefresh;

    private void OnEmbeddingEvent(ApiEventDto ev)
    {
        if (ev.Event == "embedding.download_progress" && _embProgress != null)
        {
            var total = Math.Max(1, Int(ev.Data, "overall_total"));
            _embProgress.Value = Int(ev.Data, "overall_done") * 100.0 / total;
            if (_embStatus != null)
                _embStatus.Text = L10n.Fmt("emb.downloading_fmt", Str(ev.Data, "file"), Int(ev.Data, "file_index") + 1, Int(ev.Data, "file_count"), Int(ev.Data, "overall_done") / 1_000_000, total / 1_000_000);
        }
        else if (ev.Event == "embedding.download_done")
        {
            if (Bool(ev.Data, "ok"))
                ShowStatus(InfoBarSeverity.Success, L10n.T("emb.enabled_title"), L10n.T("emb.enabled_msg"));
            else
            {
                var err = Obj(ev.Data, "error");
                if (Str(err, "code") != "CANCELLED")
                    ShowStatus(InfoBarSeverity.Error, L10n.T("emb.download_failed"), Str(err, "message"));
            }
            if (_embRefresh != null) _ = _embRefresh();
        }
    }
}
