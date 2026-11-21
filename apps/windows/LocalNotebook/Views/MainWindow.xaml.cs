using LocalNotebook.Interop;
using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Text.Json;
using Windows.Storage.Pickers;
using WinRT.Interop;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

public sealed partial class MainWindow : Window
{
    private static readonly string[] Protocols = { "chat_completion", "responses", "anthropic", "gemini" };

    private string? _activeNotebookId;
    private string _activeNotebookTitle = "";
    private bool _loadingPicker;
    private bool _refreshing;
    private bool _refreshAgain;

    public MainWindow()
    {
        InitializeComponent();
        App.Core.EventReceived += OnCoreEvent;
        Closed += (_, _) =>
        {
            _activeArtifactPane?.Dispose();
            _math.Dispose();
            App.Core.Dispose();
        };
    }

    // ---------- helpers ----------

    private void ShowStatus(InfoBarSeverity severity, string title, string message)
    {
        StatusBar.Severity = severity;
        StatusBar.Title = title;
        StatusBar.Message = message;
        StatusBar.IsOpen = true;
    }

    /// <summary>Calls the core; any error is shown in the status bar instead of being swallowed.</summary>
    private async Task<JsonElement?> CoreAsync(string method, JsonElement args, string what)
    {
        var (result, err) = await Api.CallAsync(method, args);
        if (err is { } e)
        {
            var advice = Advice(e.Code);
            ShowStatus(InfoBarSeverity.Error, $"{what}失败", advice.Length > 0 ? $"{advice}（{e.Code}）" : $"{e.Message}（{e.Code}）");
            return null;
        }
        return result;
    }

    private static TextBlock Line(string text) => new()
    {
        Text = text,
        TextWrapping = TextWrapping.Wrap,
        IsTextSelectionEnabled = true,
    };

    private async Task<string?> PromptTextAsync(string title, string label, string initial = "", bool multiline = false, string primary = "确定")
    {
        var box = new TextBox
        {
            Header = label,
            Text = initial,
            AcceptsReturn = multiline,
            TextWrapping = multiline ? TextWrapping.Wrap : TextWrapping.NoWrap,
            MinWidth = 420,
            MinHeight = multiline ? 220 : 0,
        };
        var dlg = new ContentDialog { Title = title, Content = box, PrimaryButtonText = primary, CloseButtonText = "取消", DefaultButton = ContentDialogButton.Primary, XamlRoot = Content.XamlRoot };
        return await dlg.ShowAsync() == ContentDialogResult.Primary ? box.Text : null;
    }

    private async Task<bool> ConfirmAsync(string title, string message, string primary = "确定")
    {
        var dlg = new ContentDialog { Title = title, Content = Line(message), PrimaryButtonText = primary, CloseButtonText = "取消", DefaultButton = ContentDialogButton.Close, XamlRoot = Content.XamlRoot };
        return await dlg.ShowAsync() == ContentDialogResult.Primary;
    }

    private async Task InfoAsync(string title, UIElement content)
    {
        var dlg = new ContentDialog { Title = title, Content = content, CloseButtonText = "关闭", XamlRoot = Content.XamlRoot };
        await dlg.ShowAsync();
    }

    private async Task<string?> PickSaveFileAsync(string suggestedName, string typeName, string extension)
    {
        var picker = new FileSavePicker { SuggestedFileName = suggestedName };
        picker.FileTypeChoices.Add(typeName, new List<string> { extension });
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        var file = await picker.PickSaveFileAsync();
        return file?.Path;
    }

    private async Task<string?> PickOpenFileAsync(params string[] extensions)
    {
        var picker = new FileOpenPicker();
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        foreach (var ext in extensions) picker.FileTypeFilter.Add(ext);
        var file = await picker.PickSingleFileAsync();
        return file?.Path;
    }

    // ---------- startup ----------

    public void ReportStartupFailure(string message) =>
        ShowStatus(InfoBarSeverity.Error, "核心启动失败", message);

    public async Task OnCoreReadyAsync()
    {
        var list = await CoreAsync("notebook.list", Args(), "读取笔记本");
        if (list is { } res && Arr(res, "notebooks").FirstOrDefault() is { ValueKind: JsonValueKind.Object } first)
        {
            _activeNotebookId = Str(first, "id");
            _activeNotebookTitle = Str(first, "title");
        }
        else
        {
            var created = await CoreAsync("notebook.create", Args(("title", "未命名笔记本")), "创建笔记本");
            if (created is { } cr)
            {
                _activeNotebookId = Str(cr, "id");
                _activeNotebookTitle = Str(cr, "title");
            }
        }
        NotebookTitleText.Text = _activeNotebookTitle.Length > 0 ? _activeNotebookTitle : "未命名笔记本";
        await RefreshNotebookPickerAsync();
        await RefreshNotebookDataAsync();
        if (!await WarnIfNoModelAsync()) await ShowOnboardingIfFirstRunAsync();
    }

    /// <summary>Returns true when a model is configured.</summary>
    private async Task<bool> WarnIfNoModelAsync()
    {
        var profiles = await CoreAsync("profile.list", Args(), "读取模型配置");
        if (profiles is not { } p) return false;
        var ok = Arr(p, "profiles").Any(prof => Bool(prof, "is_active"));
        if (!ok)
        {
            ShowStatus(InfoBarSeverity.Warning, "尚未配置模型", "请点击右上角“模型协议配置”，填写协议、地址、模型与 API Key。");
        }
        else if (StatusBar.Title == "尚未配置模型")
        {
            StatusBar.IsOpen = false; // configured now; don't leave the stale warning up
        }
        return ok;
    }

    // ---------- notebooks ----------

    private async Task RefreshNotebookPickerAsync()
    {
        var list = await CoreAsync("notebook.list", Args(), "读取笔记本");
        if (list is not { } res) return;
        _loadingPicker = true;
        try
        {
            NotebookPicker.Items.Clear();
            foreach (var nb in Arr(res, "notebooks"))
            {
                var id = Str(nb, "id");
                var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
                row.Children.Add(new PathIcon
                {
                    Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M18,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V4A2,2 0 0,0 18,2M18,20H6V4H8V12L10.5,9.75L13,12V4H18V20Z"),
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentBlueBrush"],
                });
                row.Children.Add(new TextBlock { Text = Str(nb, "title"), VerticalAlignment = VerticalAlignment.Center });
                var item = new ComboBoxItem { Content = row, Tag = id };
                NotebookPicker.Items.Add(item);
                if (id == _activeNotebookId) NotebookPicker.SelectedItem = item;
            }
        }
        finally
        {
            _loadingPicker = false;
        }
    }

    private async void OnNotebookPicked(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingPicker || NotebookPicker.SelectedItem is not ComboBoxItem item || item.Tag is not string id) return;
        if (id == _activeNotebookId) return;
        await SwitchNotebookAsync(id);
    }

    private async Task SwitchNotebookAsync(string id)
    {
        _activeNotebookId = id;
        _streaming = null;
        var nb = await CoreAsync("notebook.get", Args(("notebook_id", id)), "读取笔记本");
        _activeNotebookTitle = nb is { } n ? Str(n, "title") : "";
        NotebookTitleText.Text = _activeNotebookTitle.Length > 0 ? _activeNotebookTitle : "未命名笔记本";
        await RefreshNotebookPickerAsync();
        await RefreshNotebookDataAsync();
    }

    private async void OnNewNotebookClick(object sender, RoutedEventArgs e)
    {
        var res = await CoreAsync("notebook.create", Args(("title", "新笔记本")), "新建笔记本");
        if (res is { } r) await SwitchNotebookAsync(Str(r, "id"));
    }

    private async void OnRenameNotebookClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var name = await PromptTextAsync("重命名笔记本", "名称", _activeNotebookTitle);
        if (string.IsNullOrWhiteSpace(name)) return;
        if (await CoreAsync("notebook.rename", Args(("notebook_id", _activeNotebookId), ("title", name.Trim())), "重命名笔记本") != null)
        {
            _activeNotebookTitle = name.Trim();
            NotebookTitleText.Text = _activeNotebookTitle;
            await RefreshNotebookPickerAsync();
        }
    }

    private async void OnDeleteNotebookClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        if (!await ConfirmAsync("删除笔记本", $"确定删除“{_activeNotebookTitle}”及其全部来源、对话和产物吗？此操作无法撤销。", "删除")) return;
        if (await CoreAsync("notebook.delete", Args(("notebook_id", _activeNotebookId)), "删除笔记本") == null) return;
        var list = await CoreAsync("notebook.list", Args(), "读取笔记本");
        var next = list is { } l ? Arr(l, "notebooks").FirstOrDefault() : default;
        if (next.ValueKind == JsonValueKind.Object)
        {
            await SwitchNotebookAsync(Str(next, "id"));
        }
        else
        {
            var created = await CoreAsync("notebook.create", Args(("title", "未命名笔记本")), "创建笔记本");
            if (created is { } c) await SwitchNotebookAsync(Str(c, "id"));
        }
    }

    private async void OnExportLnaClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var path = await PickSaveFileAsync(_activeNotebookTitle, "Local Notebook 归档", ".lna");
        if (path == null) return;
        if (await CoreAsync("notebook.export", Args(("notebook_id", _activeNotebookId), ("format", "lna"), ("out_path", path)), "导出") != null)
            ShowStatus(InfoBarSeverity.Success, "已导出", path);
    }

    private async void OnExportMarkdownClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var path = await PickSaveFileAsync(_activeNotebookTitle, "Markdown", ".md");
        if (path == null) return;
        if (await CoreAsync("notebook.export", Args(("notebook_id", _activeNotebookId), ("format", "markdown"), ("out_path", path)), "导出") != null)
            ShowStatus(InfoBarSeverity.Success, "已导出", path);
    }

    private async void OnImportLnaClick(object sender, RoutedEventArgs e)
    {
        var path = await PickOpenFileAsync(".lna");
        if (path == null) return;
        var res = await CoreAsync("notebook.import", Args(("in_path", path)), "导入");
        if (res is { } r)
        {
            ShowStatus(InfoBarSeverity.Success, "已导入", "来源正在重新建立索引。");
            await SwitchNotebookAsync(Str(r, "notebook_id"));
        }
    }

    private async void OnExportDiagnosticsClick(object sender, RoutedEventArgs e)
    {
        var path = await PickSaveFileAsync("diagnostics", "诊断包 (JSON)", ".json");
        if (path == null) return;
        if (await CoreAsync("diagnostics.export", Args(("out_path", path)), "导出诊断包") != null)
            ShowStatus(InfoBarSeverity.Success, "诊断包已导出", "只包含数量、状态与错误码，不含文档内容、提示词、回答或密钥。");
    }

    // ---------- refresh ----------

    private async Task RefreshNotebookDataAsync()
    {
        if (_activeNotebookId == null) return;
        // Events arrive in bursts; coalesce so only one refresh runs at a time and the last one wins.
        if (_refreshing)
        {
            _refreshAgain = true;
            return;
        }
        _refreshing = true;
        try
        {
            do
            {
                _refreshAgain = false;
                await RefreshOnceAsync();
            } while (_refreshAgain);
        }
        finally
        {
            _refreshing = false;
        }
    }

    private async Task RefreshOnceAsync()
    {
        var nid = _activeNotebookId!;
        var param = Args(("notebook_id", nid));

        var summary = await CoreAsync("notebook.get", param, "读取笔记本");
        if (summary is { } sm) RenderSummary(sm);

        var src = await CoreAsync("source.list", param, "读取来源");
        var notes = await CoreAsync("note.list", param, "读取笔记");
        RenderSources(src, notes);

        if (_streaming == null)
        {
            var chat = await CoreAsync("chat.history", param, "读取对话");
            if (chat is { } c) RenderChat(c);
        }

        var art = await CoreAsync("studio.list", param, "读取 Studio 产物");
        if (art is { } a) RenderArtifacts(a);
    }

    private void RenderSummary(JsonElement nb)
    {
        var md = Str(nb, "summary_md");
        SummaryText.Text = md.Length > 0 ? md : "请添加来源或直接提问。";
        SuggestedPanel.Children.Clear();
        SuggestedPanel.ColumnDefinitions.Clear();
        var raw = Str(nb, "suggested_questions_json");
        var questions = new List<string>();
        if (nb.TryGetProperty("suggested_questions", out var sq) && sq.ValueKind == JsonValueKind.Array)
        {
            foreach (var q in sq.EnumerateArray()) if (q.ValueKind == JsonValueKind.String) questions.Add(q.GetString() ?? "");
        }
        else if (raw.Length > 0)
        {
            try
            {
                using var doc = JsonDocument.Parse(raw);
                foreach (var q in doc.RootElement.EnumerateArray()) questions.Add(q.GetString() ?? "");
            }
            catch (JsonException) { }
        }
        var topQuestions = questions.Where(q => q.Length > 0).Take(3).ToList();
        for (int i = 0; i < topQuestions.Count; i++)
        {
            SuggestedPanel.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var q = topQuestions[i];
            var btn = new Button
            {
                Content = new TextBlock
                {
                    Text = q,
                    FontSize = 12,
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                    TextWrapping = TextWrapping.Wrap,
                    MaxLines = 2,
                    TextTrimming = TextTrimming.CharacterEllipsis,
                },
                Tag = q,
                Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["PanelBgBrush"],
                BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["StrongBorderBrush"],
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(12),
                Padding = new Thickness(14, 10, 14, 10),
                MinHeight = 44,
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Left,
                VerticalAlignment = VerticalAlignment.Stretch,
            };
            btn.Click += async (s, _) => { if (s is Button b && b.Tag is string text) await SendChatAsync(text); };
            Grid.SetColumn(btn, i);
            SuggestedPanel.Children.Add(btn);
        }
    }

    // ---------- events ----------

    private void OnCoreEvent(ApiEventDto ev)
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            try
            {
                switch (ev.Event)
                {
                    case "source.status":
                    case "studio.status":
                    case "notebook.summary_updated":
                        await RefreshNotebookDataAsync();
                        break;
                    case "embedding.download_progress":
                    case "embedding.download_done":
                        OnEmbeddingEvent(ev);
                        break;
                    default:
                        if (ev.Event.StartsWith("chat.")) await OnChatEventAsync(ev);
                        break;
                }
            }
            catch (Exception ex)
            {
                ShowStatus(InfoBarSeverity.Error, "界面更新失败", ex.Message);
            }
        });
    }
}
