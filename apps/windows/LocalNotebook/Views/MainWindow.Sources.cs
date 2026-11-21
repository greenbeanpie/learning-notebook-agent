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

    private bool _updatingSourceChecks;

    private static (string Hex, string PathData) SourceKindIcon(string kind, string title)
    {
        var k = kind.ToLowerInvariant();
        if (title.EndsWith(".pdf", StringComparison.OrdinalIgnoreCase)) k = "pdf";
        else if (title.EndsWith(".md", StringComparison.OrdinalIgnoreCase) || title.EndsWith(".markdown", StringComparison.OrdinalIgnoreCase)) k = "md";
        else if (string.IsNullOrEmpty(k) && title.EndsWith(".txt", StringComparison.OrdinalIgnoreCase)) k = "txt";
        return k switch
        {
            "pdf" => ("#F28B82", "M19,3H5C3.9,3 3,3.9 3,5V19C3,20.1 3.9,21 5,21H19C20.1,21 21,20.1 21,19V5C21,3.9 20.1,3 19,3M9.5,11.5C9.5,12.3 8.8,13 8,13H7V15H5.5V9H8C8.8,9 9.5,9.7 9.5,10.5V11.5M14.5,13.5C14.5,14.3 13.8,15 13,15H10.5V9H13C13.8,9 14.5,9.7 14.5,10.5V13.5M18.5,10.5H17V11.5H18.5V13H17V15H15.5V9H18.5V10.5M12,10.5H13V13.5H12V10.5M7,10.5H8V11.5H7V10.5Z"),
            "md" or "markdown" => ("#8AB4F8", "M20.56,18H3.44C2.65,18 2,17.37 2,16.59V7.41C2,6.63 2.65,6 3.44,6H20.56C21.35,6 22,6.63 22,7.41V16.59C22,17.37 21.35,18 20.56,18M6.81,15.19V11.53L8.73,13.88L10.65,11.53V15.19H12.58V8.81H10.65L8.73,11.16L6.81,8.81H4.89V15.19H6.81M19.69,12H17.77V8.81H15.85V12H13.92L16.81,15.28L19.69,12Z"),
            "txt" => ("#81C995", "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20M8,12H16V14H8V12M8,16H13V18H8V16Z"),
            "web" => ("#78D9EC", "M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2M11,19.93C7.05,19.44 4,16.08 4,12C4,11.38 4.08,10.79 4.21,10.21L9,15V16A2,2 0 0,0 11,18M17.9,17.39C17.64,16.58 16.9,16 16,16H15V13A1,1 0 0,0 14,12H8V10H10A1,1 0 0,0 11,9V7H13A2,2 0 0,0 15,5V4.59C17.93,5.78 20,8.65 20,12C20,14.08 19.2,15.97 17.9,17.39Z"),
            "note" => ("#FDD663", "M14,10H19.5L14,4.5V10M5,3H15L21,9V19A2,2 0 0,1 19,21H5C3.89,21 3,20.1 3,19V5C3,3.89 3.89,3 5,3M5,5V19H19V12H12V5H5Z"),
            _ => ("#C4C7C5", "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20Z"),
        };
    }

    private static Microsoft.UI.Xaml.Media.SolidColorBrush BrushFromHex(string hex) => new(Windows.UI.Color.FromArgb(
        255,
        Convert.ToByte(hex.Substring(1, 2), 16),
        Convert.ToByte(hex.Substring(3, 2), 16),
        Convert.ToByte(hex.Substring(5, 2), 16)));

    private void RenderSources(JsonElement? src, JsonElement? notes)
    {
        SourcesList.Items.Clear();
        if (src is { } sr)
        {
            var sourceItems = Arr(sr, "sources").ToList();
            if (sourceItems.Count > 0)
            {
                var allIds = sourceItems.Select(x => Str(x, "id")).ToList();
                var allSelected = sourceItems.All(x => Bool(x, "is_selected"));
                var selectAllRow = new Grid
                {
                    ColumnSpacing = 8,
                    MinHeight = 38,
                    Padding = new Thickness(8, 2, 4, 2),
                    Margin = new Thickness(0, 0, 0, 4),
                };
                selectAllRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                selectAllRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                selectAllRow.Children.Add(new TextBlock
                {
                    Text = "全选来源",
                    FontSize = 12,
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
                    VerticalAlignment = VerticalAlignment.Center,
                });
                var selectAllCb = new CheckBox
                {
                    IsChecked = allSelected,
                    MinWidth = 36,
                    Width = 36,
                    MinHeight = 36,
                    Height = 36,
                    Padding = new Thickness(0),
                    HorizontalContentAlignment = HorizontalAlignment.Center,
                    VerticalContentAlignment = VerticalAlignment.Center,
                    VerticalAlignment = VerticalAlignment.Center,
                };
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(selectAllCb, "全选来源");
                ToolTipService.SetToolTip(selectAllCb, "全选或取消全选所有来源");
                selectAllCb.Checked += async (_, _) => await SetAllSourcesSelectedAsync(allIds, true);
                selectAllCb.Unchecked += async (_, _) => await SetAllSourcesSelectedAsync(allIds, false);
                Grid.SetColumn(selectAllCb, 1);
                selectAllRow.Children.Add(selectAllCb);
                SourcesList.Items.Add(selectAllRow);

                foreach (var s in sourceItems)
                {
                    SourcesList.Items.Add(SourceRow(s));
                }
            }
            else
            {
                var empty = Line("还没有来源。点击上方“+ 添加来源”导入文件、网页或粘贴文本。");
                empty.Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"];
                empty.Margin = new Thickness(4, 6, 4, 6);
                SourcesList.Items.Add(empty);
            }
        }
        if (notes is { } nr)
        {
            var list = Arr(nr, "notes").ToList();
            if (list.Count > 0)
            {
                SourcesList.Items.Add(new TextBlock
                {
                    Text = "笔记",
                    FontSize = 13,
                    FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                    Margin = new Thickness(8, 16, 4, 6),
                });
                foreach (var n in list)
                {
                    var rowContent = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, VerticalAlignment = VerticalAlignment.Center };
                    rowContent.Children.Add(new PathIcon
                    {
                        Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M14,10H19.5L14,4.5V10M5,3H15L21,9V19A2,2 0 0,1 19,21H5C3.89,21 3,20.1 3,19V5C3,3.89 3.89,3 5,3M5,5V19H19V12H12V5H5Z"),
                        Foreground = BrushFromHex("#FDD663"),
                        VerticalAlignment = VerticalAlignment.Center,
                    });
                    rowContent.Children.Add(new TextBlock
                    {
                        Text = Str(n, "title"),
                        FontSize = 13,
                        Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"],
                        TextTrimming = TextTrimming.CharacterEllipsis,
                        MaxLines = 1,
                        VerticalAlignment = VerticalAlignment.Center,
                    });
                    var b = new Button
                    {
                        Content = rowContent,
                        Tag = Str(n, "id"),
                        MinHeight = 42,
                        Padding = new Thickness(8, 8, 8, 8),
                        Margin = new Thickness(0, 2, 0, 2),
                        CornerRadius = new CornerRadius(10),
                        Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
                        BorderThickness = new Thickness(0),
                        HorizontalAlignment = HorizontalAlignment.Stretch,
                        HorizontalContentAlignment = HorizontalAlignment.Left,
                    };
                    b.Click += async (s, _) => { if (s is Button btn && btn.Tag is string id) await EditNoteAsync(id); };
                    SourcesList.Items.Add(b);
                }
            }
        }
    }

    private async Task SetAllSourcesSelectedAsync(List<string> ids, bool selected)
    {
        if (_updatingSourceChecks) return;
        _updatingSourceChecks = true;
        try
        {
            foreach (var id in ids)
            {
                await CoreAsync("source.set_selected", Args(("source_id", id), ("selected", selected)), "更新来源选择");
            }
            await RefreshNotebookDataAsync();
        }
        finally
        {
            _updatingSourceChecks = false;
        }
    }

    private UIElement SourceRow(JsonElement s)
    {
        var id = Str(s, "id");
        var status = Str(s, "status");
        var code = Str(s, "error_code");
        var title = Str(s, "title");
        var kind = Str(s, "kind");

        string? subtext = null;
        var subColor = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"];
        if (status == "FAILED")
        {
            var advice = Advice(code);
            var detail = advice.Length > 0 ? advice : Str(s, "error_detail");
            subtext = detail.Length > 0 ? $"失败 — {detail}" : "失败";
            subColor = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 160, 140));
        }
        else if (status != "READY")
        {
            subtext = StatusLabel(status);
        }
        else if (code == "EMBEDDING_INCOMPLETE")
        {
            subtext = "仅关键词检索";
            subColor = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 200, 120));
        }

        // 1. Icon (PDF / Markdown / TXT / Web / Note)
        var (iconHex, iconPath) = SourceKindIcon(kind, title);
        var typeIcon = new PathIcon
        {
            Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), iconPath),
            Foreground = BrushFromHex(iconHex),
            VerticalAlignment = VerticalAlignment.Center,
            HorizontalAlignment = HorizontalAlignment.Center,
        };
        var iconHost = new Viewbox
        {
            Width = 18,
            Height = 18,
            VerticalAlignment = VerticalAlignment.Center,
            Child = typeIcon,
        };

        // 2. Filename (+ subtle status line only when not READY)
        var titleStack = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        var titleBlock = new TextBlock
        {
            Text = title,
            FontSize = 13,
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"],
            TextTrimming = TextTrimming.CharacterEllipsis,
            MaxLines = 1,
        };
        titleStack.Children.Add(titleBlock);
        if (subtext != null)
        {
            titleStack.Children.Add(new TextBlock
            {
                Text = subtext,
                FontSize = 11,
                Foreground = subColor,
                TextTrimming = TextTrimming.CharacterEllipsis,
                MaxLines = 1,
            });
        }
        ToolTipService.SetToolTip(titleStack, subtext != null ? $"{title}\n{subtext}" : $"{title}（{StatusLabel(status)}）");
        if (status == "READY")
        {
            titleStack.Tapped += async (_, _) => await ShowSourcePreviewAsync(id, null, null);
        }

        // 3. Actions (three dots: preview, rename, retry, delete)
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

        var more = new Button
        {
            Content = new PathIcon
            {
                Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M6,10A2,2 0 0,0 4,12A2,2 0 0,0 6,14A2,2 0 0,0 8,12A2,2 0 0,0 6,10M12,10A2,2 0 0,0 10,12A2,2 0 0,0 12,14A2,2 0 0,0 14,12A2,2 0 0,0 12,10M18,10A2,2 0 0,0 16,12A2,2 0 0,0 18,14A2,2 0 0,0 20,12A2,2 0 0,0 18,10Z"),
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
            },
            Flyout = menu,
            MinWidth = 36,
            Width = 36,
            MinHeight = 36,
            Height = 36,
            Padding = new Thickness(0),
            Margin = new Thickness(0, 0, 4, 0),
            CornerRadius = new CornerRadius(18),
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
            VerticalAlignment = VerticalAlignment.Center,
        };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(more, $"更多操作 {title}");
        ToolTipService.SetToolTip(more, "更多操作（重命名、删除等）");

        // 4. Select whether to cite (CheckBox)
        var cb = new CheckBox
        {
            IsChecked = Bool(s, "is_selected"),
            Tag = id,
            MinWidth = 36,
            Width = 36,
            MinHeight = 36,
            Height = 36,
            Padding = new Thickness(0),
            HorizontalContentAlignment = HorizontalAlignment.Center,
            VerticalContentAlignment = VerticalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center,
        };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(cb, $"引用来源 {title}");
        ToolTipService.SetToolTip(cb, "选择是否引用此来源");
        cb.Checked += OnSourceCheckChanged;
        cb.Unchecked += OnSourceCheckChanged;

        var row = new Grid
        {
            ColumnSpacing = 8,
            MinHeight = 44,
        };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        Grid.SetColumn(iconHost, 0);
        Grid.SetColumn(titleStack, 1);
        Grid.SetColumn(more, 2);
        Grid.SetColumn(cb, 3);

        row.Children.Add(iconHost);
        row.Children.Add(titleStack);
        row.Children.Add(more);
        row.Children.Add(cb);

        return new Border
        {
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
            CornerRadius = new CornerRadius(10),
            Padding = new Thickness(8, 4, 4, 4),
            Margin = new Thickness(0, 2, 0, 2),
            Child = row,
        };
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
        if (_updatingSourceChecks) return;
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
