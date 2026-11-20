using LocalNotebook.Interop;
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
        var download = new Button { Content = "下载并启用" };
        var cancel = new Button { Content = "取消下载", Visibility = Visibility.Collapsed };
        var rebuild = new Button { Content = "重建索引" };
        _embProgress = progress; _embStatus = status; _embDownload = download; _embCancel = cancel;

        var panel = new StackPanel { Spacing = 10, MinWidth = 460, MaxWidth = 520 };
        panel.Children.Add(Line("语义检索让提问和来源语言不同、或措辞不同时也能找到相关内容。它需要一个本地嵌入模型；不下载也能用，只是退回关键词检索。"));
        panel.Children.Add(status);
        panel.Children.Add(progress);
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        row.Children.Add(download);
        row.Children.Add(cancel);
        row.Children.Add(rebuild);
        panel.Children.Add(row);
        panel.Children.Add(new TextBlock { Text = "默认模型：multilingual-e5-small（MIT 许可，中英双语）。下载来自 Hugging Face，固定版本并校验 SHA-256；模型在本机运行，不会上传你的内容。", TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.7 });

        async Task RefreshAsync()
        {
            var es = await CoreAsync("embedding.status", Args(), "读取语义检索状态");
            var dm = await CoreAsync("embedding.default_model", Args(), "读取模型信息");
            var dl = await CoreAsync("embedding.download_status", Args(), "读取下载状态");
            if (es is not { } s || dm is not { } d) return;
            var running = dl is { } x && Bool(x, "running");
            var activeModel = Obj(s, "active_model");
            var total = Int(s, "total_chunks");
            var done = Int(s, "embedded_chunks");
            var mb = Int(d, "total_bytes") / 1_000_000;
            if (activeModel.ValueKind == JsonValueKind.Object)
                status.Text = $"当前模型：{Str(activeModel, "model_name")}（{Str(activeModel, "provider")}）。已建立索引的片段：{done}/{total}。";
            else
                status.Text = Bool(d, "installed") ? "模型文件已下载，尚未启用。" : $"尚未启用语义检索。默认模型约 {mb} MB。";
            download.Content = Bool(d, "installed") ? "启用已下载的模型" : $"下载并启用（约 {mb} MB）";
            download.IsEnabled = !running;
            cancel.Visibility = running ? Visibility.Visible : Visibility.Collapsed;
            progress.Visibility = running ? Visibility.Visible : Visibility.Collapsed;
            rebuild.IsEnabled = activeModel.ValueKind == JsonValueKind.Object;
        }

        download.Click += async (_, _) =>
        {
            if (await CoreAsync("embedding.download", Args(), "下载模型") != null)
            {
                progress.Value = 0;
                await RefreshAsync();
            }
        };
        cancel.Click += async (_, _) => { await CoreAsync("embedding.download_cancel", Args(), "取消下载"); };
        rebuild.Click += async (_, _) =>
        {
            if (!await ConfirmAsync("重建索引", "将用当前模型重新计算所有来源的向量，期间检索可能暂时只用关键词。继续吗？", "重建")) return;
            var r = await CoreAsync("embedding.rebuild", Args(("force", true)), "重建索引");
            if (r is { } rr) status.Text = $"已排队重建 {Int(rr, "sources_queued")} 个来源。";
        };

        await RefreshAsync();
        var dlg = new ContentDialog { Title = "语义检索", Content = panel, CloseButtonText = "关闭", XamlRoot = Content.XamlRoot };
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
                _embStatus.Text = $"正在下载 {Str(ev.Data, "file")}（{Int(ev.Data, "file_index") + 1}/{Int(ev.Data, "file_count")}）：{Int(ev.Data, "overall_done") / 1_000_000} / {total / 1_000_000} MB";
        }
        else if (ev.Event == "embedding.download_done")
        {
            if (Bool(ev.Data, "ok"))
                ShowStatus(InfoBarSeverity.Success, "语义检索已启用", "模型已下载，正在为已有来源建立索引。");
            else
            {
                var err = Obj(ev.Data, "error");
                if (Str(err, "code") != "CANCELLED")
                    ShowStatus(InfoBarSeverity.Error, "模型下载失败", Str(err, "message"));
            }
            if (_embRefresh != null) _ = _embRefresh();
        }
    }
}
