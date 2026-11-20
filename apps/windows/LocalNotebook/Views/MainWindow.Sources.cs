using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

public sealed partial class MainWindow
{
    private static string StatusLabel(string status) => status switch
    {
        "QUEUED" => "排队中",
        "PARSING" => "解析中",
        "CHUNKING" => "分块中",
        "EMBEDDING" => "建立索引",
        "READY" => "就绪",
        "FAILED" => "失败",
        _ => status,
    };

    private void RenderSources(JsonElement? src, JsonElement? notes)
    {
        SourcesList.Items.Clear();
        if (src is { } sr)
        {
            var any = false;
            foreach (var s in Arr(sr, "sources"))
            {
                any = true;
                SourcesList.Items.Add(SourceRow(s));
            }
            if (!any)
            {
                SourcesList.Items.Add(Line("还没有来源。点击上方“+ 添加来源”导入文件、网页或粘贴文本。"));
            }
        }
        if (notes is { } nr)
        {
            var list = Arr(nr, "notes").ToList();
            if (list.Count > 0)
            {
                SourcesList.Items.Add(new TextBlock { Text = "笔记", FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Margin = new Thickness(0, 10, 0, 2), Opacity = 0.8 });
                foreach (var n in list)
                {
                    var b = new HyperlinkButton { Content = $"📝 {Str(n, "title")}", Tag = Str(n, "id"), Padding = new Thickness(0, 2, 0, 2) };
                    b.Click += async (s, _) => { if (s is HyperlinkButton hb && hb.Tag is string id) await EditNoteAsync(id); };
                    SourcesList.Items.Add(b);
                }
            }
        }
    }

    private UIElement SourceRow(JsonElement s)
    {
        var id = Str(s, "id");
        var status = Str(s, "status");
        var code = Str(s, "error_code");
        var title = Str(s, "title");
        var text = $"{title} [{StatusLabel(status)}]";
        if (status == "FAILED")
        {
            var advice = Advice(code);
            var detail = advice.Length > 0 ? advice : Str(s, "error_detail");
            if (detail.Length > 0) text += $" — {detail}";
        }
        else if (code == "EMBEDDING_INCOMPLETE")
        {
            text += " — 语义索引未完成，仅关键词检索";
        }
        var cb = new CheckBox
        {
            Content = new TextBlock { Text = text, TextWrapping = TextWrapping.Wrap },
            IsChecked = Bool(s, "is_selected"),
            Tag = id,
        };
        cb.Checked += OnSourceCheckChanged;
        cb.Unchecked += OnSourceCheckChanged;

        var menu = new MenuFlyout();
        void Add(string label, RoutedEventHandler h)
        {
            var item = new MenuFlyoutItem { Text = label, Tag = (id, title) };
            item.Click += h;
            menu.Items.Add(item);
        }
        if (status == "READY") Add("预览", OnPreviewSourceClick);
        Add("重命名", OnRenameSourceClick);
        if (status == "FAILED") Add("重试导入", OnRetrySourceClick);
        Add("删除来源", OnDeleteSourceClick);
        // A visible "⋯" button opens the menu: right-click alone is hard to discover.
        var more = new Button { Content = "⋯", Flyout = menu, Padding = new Thickness(8, 2, 8, 2), VerticalAlignment = VerticalAlignment.Top };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(more, "更多操作");
        var row = new Grid { ColumnSpacing = 4 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(more, 1);
        row.Children.Add(cb);
        row.Children.Add(more);
        return row;
    }

    private static (string Id, string Title) Tagged(object sender) =>
        sender is MenuFlyoutItem { Tag: ValueTuple<string, string> t } ? t : ("", "");

    private async void OnAddSourceClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var path = await PickOpenFileAsync(".pdf", ".md", ".txt");
        if (path == null) return;
        await CoreAsync("source.add_files", Args(("notebook_id", _activeNotebookId), ("paths", new[] { path })), "添加来源");
        await RefreshNotebookDataAsync();
    }

    private async void OnAddUrlClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var url = await PromptTextAsync("添加网页", "网页地址（http 或 https）", "https://", false, "导入");
        if (string.IsNullOrWhiteSpace(url)) return;
        ShowStatus(InfoBarSeverity.Informational, "正在抓取网页", url.Trim());
        if (await CoreAsync("source.add_url", Args(("notebook_id", _activeNotebookId), ("url", url.Trim())), "添加网页") != null)
        {
            StatusBar.IsOpen = false;
            await RefreshNotebookDataAsync();
        }
    }

    private async void OnAddPasteClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var titleBox = new TextBox { Header = "标题", Text = "粘贴的文本", MinWidth = 460 };
        var textBox = new TextBox { Header = "内容", AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 240, MinWidth = 460 };
        var panel = new StackPanel { Spacing = 10 };
        panel.Children.Add(titleBox);
        panel.Children.Add(textBox);
        var dlg = new ContentDialog { Title = "粘贴文本", Content = panel, PrimaryButtonText = "添加", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Primary, XamlRoot = Content.XamlRoot };
        if (await dlg.ShowAsync() != ContentDialogResult.Primary || string.IsNullOrWhiteSpace(textBox.Text)) return;
        var res = await CoreAsync("source.add_text", Args(("notebook_id", _activeNotebookId), ("title", titleBox.Text.Trim()), ("text", textBox.Text)), "添加文本");
        if (res is { } r && Bool(r, "duplicate")) ShowStatus(InfoBarSeverity.Informational, "内容已存在", "这段文本与已有来源相同，没有重复添加。");
        await RefreshNotebookDataAsync();
    }

    private async void OnSourceCheckChanged(object sender, RoutedEventArgs e)
    {
        if (sender is not CheckBox cb || cb.Tag is not string id) return;
        await CoreAsync("source.set_selected", Args(("source_id", id), ("selected", cb.IsChecked == true)), "更新来源选择");
    }

    private async void OnRetrySourceClick(object sender, RoutedEventArgs e)
    {
        var (id, _) = Tagged(sender);
        if (id.Length == 0) return;
        await CoreAsync("source.retry", Args(("source_id", id)), "重试导入");
        await RefreshNotebookDataAsync();
    }

    private async void OnDeleteSourceClick(object sender, RoutedEventArgs e)
    {
        var (id, title) = Tagged(sender);
        if (id.Length == 0) return;
        if (!await ConfirmAsync("删除来源", $"确定删除“{title}”吗？它的内容、索引和相关引用来源会被移除。", "删除")) return;
        await CoreAsync("source.remove", Args(("source_id", id)), "删除来源");
        await RefreshNotebookDataAsync();
    }

    private async void OnRenameSourceClick(object sender, RoutedEventArgs e)
    {
        var (id, title) = Tagged(sender);
        if (id.Length == 0) return;
        var name = await PromptTextAsync("重命名来源", "名称", title);
        if (string.IsNullOrWhiteSpace(name)) return;
        await CoreAsync("source.rename", Args(("source_id", id), ("title", name.Trim())), "重命名来源");
        await RefreshNotebookDataAsync();
    }

    private async void OnPreviewSourceClick(object sender, RoutedEventArgs e)
    {
        var (id, _) = Tagged(sender);
        if (id.Length > 0) await ShowSourcePreviewAsync(id, null, null);
    }

    /// <summary>Lists a source's chunks; when <paramref name="focusText"/> or a page is given, that part is scrolled into view.</summary>
    private async Task ShowSourcePreviewAsync(string sourceId, int? page, string? focusText)
    {
        var res = await CoreAsync("source.preview", Args(("source_id", sourceId)), "读取来源预览");
        if (res is not { } r) return;
        var panel = new StackPanel { Spacing = 10, MinWidth = 520, MaxWidth = 700 };
        FrameworkElement? focus = null;
        var needle = (focusText ?? "").Trim();
        if (needle.Length > 60) needle = needle[..60];
        foreach (var c in Arr(r, "chunks"))
        {
            var content = Str(c, "content");
            var p = (int)Int(c, "page_start");
            var header = new TextBlock { FontSize = 11, Opacity = 0.6, Text = $"片段 {Int(c, "chunk_index") + 1}{(p > 0 ? $" · 第 {p} 页" : "")}{(Str(c, "section_path").Length > 0 ? $" · {Str(c, "section_path")}" : "")}" };
            var body = new Border { Padding = new Thickness(8), CornerRadius = new CornerRadius(6), Child = Line(content) };
            var hit = (needle.Length > 0 && content.Contains(needle)) || (needle.Length == 0 && page is { } pg && p == pg);
            if (hit)
            {
                body.Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 40, 60, 90));
                focus ??= body;
            }
            panel.Children.Add(header);
            panel.Children.Add(body);
        }
        if (panel.Children.Count == 0) panel.Children.Add(Line("这个来源还没有可预览的内容。"));
        var scroll = new ScrollViewer { Content = panel, MaxHeight = 560 };
        var dlg = new ContentDialog { Title = "来源预览", Content = scroll, CloseButtonText = "关闭", XamlRoot = Content.XamlRoot };
        dlg.Opened += (_, _) => focus?.StartBringIntoView();
        await dlg.ShowAsync();
    }

    // ---------- notes ----------

    private async void OnCreateNoteClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var res = await CoreAsync("note.save", Args(("notebook_id", _activeNotebookId), ("title", "新笔记"), ("body_md", "")), "创建笔记");
        await RefreshNotebookDataAsync();
        if (res is { } r) await EditNoteAsync(Str(r, "id"));
    }

    private async Task EditNoteAsync(string noteId)
    {
        var note = await CoreAsync("note.get", Args(("note_id", noteId)), "读取笔记");
        if (note is not { } n || _activeNotebookId == null) return;
        var titleBox = new TextBox { Header = "标题", Text = Str(n, "title"), MinWidth = 520 };
        var bodyBox = new TextBox { Header = "内容（Markdown）", Text = Str(n, "body_md"), AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 260 };
        var panel = new StackPanel { Spacing = 10 };
        panel.Children.Add(titleBox);
        panel.Children.Add(bodyBox);
        var origin = Str(n, "origin");
        if (origin == "chat") panel.Children.Add(new TextBlock { Text = "这条笔记来自一条回答，保存了当时的引用。", Opacity = 0.6, FontSize = 12 });
        var del = new Button { Content = "删除笔记" };
        panel.Children.Add(del);
        var dlg = new ContentDialog
        {
            Title = "笔记",
            Content = panel,
            PrimaryButtonText = "保存",
            SecondaryButtonText = "转为来源",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = Content.XamlRoot,
        };
        var deleted = false;
        del.Click += async (_, _) =>
        {
            if (!await ConfirmAsync("删除笔记", "确定删除这条笔记吗？", "删除")) return;
            deleted = true;
            dlg.Hide();
        };
        var result = await dlg.ShowAsync();
        if (deleted)
        {
            await CoreAsync("note.delete", Args(("note_id", noteId)), "删除笔记");
        }
        else if (result == ContentDialogResult.Primary)
        {
            await CoreAsync("note.save", Args(("notebook_id", _activeNotebookId), ("id", noteId), ("title", titleBox.Text), ("body_md", bodyBox.Text)), "保存笔记");
        }
        else if (result == ContentDialogResult.Secondary)
        {
            await CoreAsync("note.save", Args(("notebook_id", _activeNotebookId), ("id", noteId), ("title", titleBox.Text), ("body_md", bodyBox.Text)), "保存笔记");
            if (await CoreAsync("note.to_source", Args(("note_id", noteId)), "转为来源") != null)
                ShowStatus(InfoBarSeverity.Success, "已转为来源", "这条笔记现在可以像其他来源一样被勾选和引用。");
        }
        await RefreshNotebookDataAsync();
    }
}
