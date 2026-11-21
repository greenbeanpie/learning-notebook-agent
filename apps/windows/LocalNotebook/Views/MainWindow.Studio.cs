using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

public sealed partial class MainWindow
{
    private static string KindName(string kind) => kind switch
    {
        "audio" => "音频概览",
        "slides" => "演示文稿",
        "mindmap" => "思维导图",
        "report" => "报告",
        "flashcards" => "闪卡",
        "quiz" => "测验",
        "infographic" => "信息图",
        _ => kind,
    };

    private static string ArtifactStatusLabel(string status, string stage) => status switch
    {
        "PENDING" => "排队中",
        "RUNNING" => stage switch { "COLLECT" => "收集资料", "PLAN" => "规划大纲", "GENERATE" => "生成中", "RENDER" => "渲染中", _ => "处理中" },
        "DRAFT_REVIEW" => "等待审核大纲",
        "READY" => "已完成",
        "FAILED" => "失败",
        "CANCELLED" => "已取消",
        _ => status,
    };

    private void RenderArtifacts(JsonElement art)
    {
        ArtifactsList.Items.Clear();
        var any = false;
        foreach (var a in Arr(art, "artifacts").Reverse())
        {
            any = true;
            ArtifactsList.Items.Add(ArtifactRow(a));
        }
        if (!any) ArtifactsList.Items.Add(Line("选择上方的类型，把已勾选的来源生成为测验、闪卡、导图、报告等。"));
    }

    private UIElement ArtifactRow(JsonElement a)
    {
        var id = Str(a, "id");
        var status = Str(a, "status");
        var title = Str(a, "title");
        var kind = Str(a, "artifact_type");
        var card = new Border
        {
            Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"],
            BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(12),
            MinHeight = 56,
            Padding = new Thickness(12, 12, 10, 12),
            Margin = new Thickness(0, 0, 0, 10),
        };
        var col = new StackPanel { Spacing = 6 };
        var head = new Grid { ColumnSpacing = 8 };
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var kindIconData = kind switch
        {
            "audio" => ("#A8C7FA", "M12,14C13.66,14 15,12.66 15,11V5C15,3.34 13.66,2 12,2C10.34,2 9,3.34 9,5V11C9,12.66 10.34,14 12,14M17,11C17,13.76 14.76,16 12,16C9.24,16 7,13.76 7,11H5C5,14.53 7.61,17.43 11,17.92V21H13V17.92C16.39,17.43 19,14.53 19,11H17Z"),
            "slides" => ("#FDD663", "M19,3H5C3.89,3 3,3.89 3,5V19C3,20.11 3.89,21 5,21H19C20.11,21 21,20.11 21,19V5C21,3.89 20.11,3 19,3M19,19H5V5H19V19M7,10H9V17H7V10M11,7H13V17H11V7M15,13H17V17H15V13Z"),
            "mindmap" => ("#C58AF9", "M12,2A3,3 0 0,1 15,5C15,6.28 14.19,7.38 13.06,7.81L14.5,10.5H16A3,3 0 0,1 19,13.5A3,3 0 0,1 16,16.5C14.68,16.5 13.56,15.66 13.16,14.5H10.84C10.44,15.66 9.32,16.5 8,16.5A3,3 0 0,1 5,13.5A3,3 0 0,1 8,10.5H9.5L10.94,7.81C9.81,7.38 9,6.28 9,5A3,3 0 0,1 12,2Z"),
            "report" => ("#81C995", "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20M8,12H16V14H8V12M8,16H13V18H8V16Z"),
            "flashcards" => ("#F6AEA9", "M4,6H2V20A2,2 0 0,0 4,22H18V20H4V6M20,2H8A2,2 0 0,0 6,4V16A2,2 0 0,0 8,18H20A2,2 0 0,0 22,16V4A2,2 0 0,0 20,2M20,16H8V4H20V16Z"),
            "quiz" => ("#78D9EC", "M12,2A10,10 0 0,1 22,12A10,10 0 0,1 12,22A10,10 0 0,1 2,12A10,10 0 0,1 12,2M11,16.5L18,9.5L16.59,8.09L11,13.67L7.91,10.59L6.5,12L11,16.5Z"),
            _ => ("#FCAD70", "M3,3V21H21V19H5V3H3M9,17H7V10H9V17M13,17H11V7H13V17M17,17H15V13H17V17Z"),
        };
        var row = new Grid { ColumnSpacing = 10 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        var iconColor = Windows.UI.Color.FromArgb(
            255,
            Convert.ToByte(kindIconData.Item1.Substring(1, 2), 16),
            Convert.ToByte(kindIconData.Item1.Substring(3, 2), 16),
            Convert.ToByte(kindIconData.Item1.Substring(5, 2), 16));
        var kindIcon = new PathIcon
        {
            Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), kindIconData.Item2),
            Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(iconColor),
            VerticalAlignment = VerticalAlignment.Center,
        };
        Grid.SetColumn(kindIcon, 0);
        row.Children.Add(kindIcon);

        var titleStack = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        titleStack.Children.Add(new TextBlock { Text = $"{title}", FontSize = 13, FontWeight = Microsoft.UI.Text.FontWeights.Medium, TextWrapping = TextWrapping.Wrap });
        var stage = Str(a, "stage");
        titleStack.Children.Add(new TextBlock { Text = $"{KindName(kind)} · {ArtifactStatusLabel(status, stage)}", FontSize = 11, Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"] });
        Grid.SetColumn(titleStack, 1);
        row.Children.Add(titleStack);
        head.Children.Add(row);

        var menu = new MenuFlyout { Placement = Microsoft.UI.Xaml.Controls.Primitives.FlyoutPlacementMode.BottomEdgeAlignedRight };
        void Add(string label, Func<Task> act)
        {
            var item = new MenuFlyoutItem { Text = label };
            item.Click += async (_, _) => { await act(); await RefreshNotebookDataAsync(); };
            menu.Items.Add(item);
        }
        Add("重命名", async () =>
        {
            var name = await PromptTextAsync("重命名产物", "名称", title);
            if (!string.IsNullOrWhiteSpace(name)) await CoreAsync("studio.rename", Args(("artifact_id", id), ("title", name.Trim())), "重命名");
        });
        if (status is "PENDING" or "RUNNING") Add("取消", async () => { await CoreAsync("studio.cancel", Args(("artifact_id", id)), "取消"); });
        if (status is "FAILED" or "CANCELLED") Add("重试", async () => { await CoreAsync("studio.retry", Args(("artifact_id", id)), "重试"); });
        if (status == "READY") Add("重新生成", async () => { await CoreAsync("studio.retry", Args(("artifact_id", id)), "重新生成"); });
        Add("删除", async () =>
        {
            if (await ConfirmAsync("删除产物", $"确定删除“{title}”吗？", "删除")) await CoreAsync("studio.delete", Args(("artifact_id", id)), "删除");
        });
        var more = new Button
        {
            Content = new PathIcon
            {
                Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M6,10A2,2 0 0,0 4,12A2,2 0 0,0 6,14A2,2 0 0,0 8,12A2,2 0 0,0 6,10M12,10A2,2 0 0,0 10,12A2,2 0 0,0 12,14A2,2 0 0,0 14,12A2,2 0 0,0 12,10M18,10A2,2 0 0,0 16,12A2,2 0 0,0 18,14A2,2 0 0,0 20,12A2,2 0 0,0 18,10Z"),
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
            },
            Flyout = menu,
            MinWidth = 38,
            Width = 38,
            MinHeight = 38,
            Height = 38,
            Padding = new Thickness(0),
            CornerRadius = new CornerRadius(19),
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
            VerticalAlignment = VerticalAlignment.Center,
        };
        more.Tapped += (_, e) => e.Handled = true;
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(more, "产物操作");
        Grid.SetColumn(more, 1);
        head.Children.Add(more);
        col.Children.Add(head);

        if (status is "PENDING" or "RUNNING") col.Children.Add(new ProgressBar { Value = Num(a, "progress") * 100, Maximum = 100 });
        var err = Str(a, "error_code");
        if (status == "FAILED")
        {
            var advice = Advice(err);
            col.Children.Add(new TextBlock { Text = advice.Length > 0 ? advice : Str(a, "error_detail"), TextWrapping = TextWrapping.Wrap, FontSize = 12, Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 160, 140)) });
        }
        if (status == "READY" && Bool(a, "sources_stale"))
            col.Children.Add(new TextBlock { Text = "来源已更新，可重新生成", FontSize = 12, Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 200, 120)) });
        if (status == "DRAFT_REVIEW")
        {
            var open = new Button
            {
                Content = "审核大纲",
                HorizontalAlignment = HorizontalAlignment.Left,
                Style = (Style)Application.Current.Resources["PillButtonStyle"],
                Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["HoverBgBrush"],
                BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["StrongBorderBrush"],
                CornerRadius = new CornerRadius(16),
                Padding = new Thickness(14, 4, 14, 4),
                FontSize = 12,
            };
            open.Click += (_, _) => OpenArtifact(id);
            col.Children.Add(open);
        }
        if (status is "READY" or "DRAFT_REVIEW")
        {
            card.Tapped += (_, _) => OpenArtifact(id);
        }
        card.Child = col;
        return card;
    }

    private ArtifactViewerPane? _activeArtifactPane;
    private bool _studioFullscreen;

    private void OpenArtifact(string id)
    {
        _activeArtifactPane?.Dispose();
        _studioFullscreen = false;
        var pane = new ArtifactViewerPane(
            id,
            this,
            onClose: CloseArtifactInline,
            onToggleFullscreen: ToggleStudioFullscreen);
        _activeArtifactPane = pane;
        StudioDetailHost.Content = pane;
        StudioHomeView.Visibility = Visibility.Collapsed;
        StudioDetailHost.Visibility = Visibility.Visible;
        ApplyStudioLayout(opened: true, fullscreen: false);
    }

    private void CloseArtifactInline()
    {
        _activeArtifactPane?.Dispose();
        _activeArtifactPane = null;
        _studioFullscreen = false;
        StudioDetailHost.Content = null;
        StudioDetailHost.Visibility = Visibility.Collapsed;
        StudioHomeView.Visibility = Visibility.Visible;
        ApplyStudioLayout(opened: false, fullscreen: false);
    }

    private void ToggleStudioFullscreen()
    {
        _studioFullscreen = !_studioFullscreen;
        _activeArtifactPane?.SetFullscreenIcon(_studioFullscreen);
        ApplyStudioLayout(opened: true, fullscreen: _studioFullscreen);
    }

    private void ApplyStudioLayout(bool opened, bool fullscreen)
    {
        if (fullscreen)
        {
            SourcesPaneBorder.Visibility = Visibility.Collapsed;
            ChatPaneBorder.Visibility = Visibility.Collapsed;
            SourcesCol.Width = new GridLength(0);
            ChatCol.Width = new GridLength(0);
            StudioCol.Width = new GridLength(1, GridUnitType.Star);
        }
        else if (opened)
        {
            SourcesPaneBorder.Visibility = Visibility.Visible;
            ChatPaneBorder.Visibility = Visibility.Visible;
            SourcesCol.Width = new GridLength(260);
            ChatCol.Width = new GridLength(1, GridUnitType.Star);
            StudioCol.Width = new GridLength(1.65, GridUnitType.Star);
        }
        else
        {
            SourcesPaneBorder.Visibility = Visibility.Visible;
            ChatPaneBorder.Visibility = Visibility.Visible;
            SourcesCol.Width = new GridLength(320);
            ChatCol.Width = new GridLength(1, GridUnitType.Star);
            StudioCol.Width = new GridLength(340);
        }
    }

    // ---------- creating ----------

    private async void OnStudioTileClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null || sender is not Button btn || btn.Tag is not string kind) return;
        var cfg = await ConfigureArtifactAsync(kind);
        if (cfg is not { } config) return;
        var res = await CoreAsync("studio.create", Args(("notebook_id", _activeNotebookId), ("artifact_type", kind), ("config", config)), $"生成{KindName(kind)}");
        if (res != null) ShowStatus(InfoBarSeverity.Success, "已提交", $"{KindName(kind)}已加入生成队列，完成后点击“打开”查看。");
        await RefreshNotebookDataAsync();
    }

    private async Task<JsonElement?> ConfigureArtifactAsync(string kind)
    {
        var panel = new StackPanel { Spacing = 10, MinWidth = 440 };
        var topic = new TextBox { Header = "主题或要求（可选）", PlaceholderText = "例如：只关注第三章的基与维数" };
        var lang = new ComboBox { Header = "语言", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var (label, tag) in new[] { ("跟随来源", ""), ("中文", "zh-CN"), ("English", "en") })
            lang.Items.Add(new ComboBoxItem { Content = label, Tag = tag });
        lang.SelectedIndex = 0;
        panel.Children.Add(topic);
        panel.Children.Add(lang);

        var count = new NumberBox { Header = kind == "flashcards" ? "卡片数量" : "题目数量", Value = kind == "flashcards" ? 20 : 10, Minimum = 3, Maximum = 50, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        var difficulty = new ComboBox { Header = "难度", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var (label, tag) in new[] { ("混合", "mixed"), ("简单", "easy"), ("中等", "medium"), ("困难", "hard") }) difficulty.Items.Add(new ComboBoxItem { Content = label, Tag = tag });
        difficulty.SelectedIndex = 0;
        var typeBoxes = new List<CheckBox>();
        ComboBox? variant = null;
        CheckBox? review = null;
        NumberBox? slidePages = null;

        switch (kind)
        {
            case "quiz":
                panel.Children.Add(count);
                panel.Children.Add(difficulty);
                foreach (var (label, tag) in new[] { ("单选", "single"), ("多选", "multi"), ("填空", "fill"), ("简答", "short") })
                {
                    var cb = new CheckBox { Content = label, Tag = tag, IsChecked = tag is "single" or "multi" };
                    typeBoxes.Add(cb);
                    panel.Children.Add(cb);
                }
                break;
            case "flashcards":
                panel.Children.Add(count);
                break;
            case "report":
                variant = new ComboBox { Header = "形式", HorizontalAlignment = HorizontalAlignment.Stretch };
                variant.Items.Add(new ComboBoxItem { Content = "文档", Tag = "document" });
                variant.Items.Add(new ComboBoxItem { Content = "互动式", Tag = "interactive" });
                variant.SelectedIndex = 0;
                panel.Children.Add(variant);
                break;
            case "slides":
                slidePages = new NumberBox { Header = "页数（简短 8 页，默认 15 页）", Value = 15, Minimum = 4, Maximum = 40, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
                review = new CheckBox { Content = "先审核大纲再生成", IsChecked = true };
                panel.Children.Add(slidePages);
                panel.Children.Add(review);
                break;
            case "infographic":
                variant = new ComboBox { Header = "方向", HorizontalAlignment = HorizontalAlignment.Stretch };
                foreach (var (label, tag) in new[] { ("横版", "landscape"), ("竖版", "portrait"), ("方形", "square") }) variant.Items.Add(new ComboBoxItem { Content = label, Tag = tag });
                variant.SelectedIndex = 0;
                panel.Children.Add(variant);
                break;
            case "audio":
                variant = new ComboBox { Header = "形式", HorizontalAlignment = HorizontalAlignment.Stretch };
                foreach (var (label, tag) in new[] { ("深入探究（双人）", "deep_dive"), ("辩论（双人）", "debate"), ("摘要（单人）", "brief"), ("评论（单人）", "critique") }) variant.Items.Add(new ComboBoxItem { Content = label, Tag = tag });
                variant.SelectedIndex = 0;
                panel.Children.Add(variant);
                panel.Children.Add(new TextBlock { Text = "音频使用系统自带的语音合成，较长的内容需要一些时间。", FontSize = 12, Opacity = 0.7, TextWrapping = TextWrapping.Wrap });
                break;
        }

        var estimate = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.85, Text = "正在估算用量…" };
        var confirmBox = new CheckBox { Content = "我已知晓用量较大，仍要继续", Visibility = Visibility.Collapsed };
        panel.Children.Add(estimate);
        panel.Children.Add(confirmBox);

        var needsConfirm = false;
        var dlg = new ContentDialog { Title = $"生成{KindName(kind)}", Content = new ScrollViewer { Content = panel, MaxHeight = 560 }, PrimaryButtonText = "生成", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Primary, XamlRoot = Content.XamlRoot };
        dlg.Opened += async (_, _) =>
        {
            var est = await Api.CallAsync("studio.estimate", Args(("notebook_id", _activeNotebookId), ("artifact_type", kind)));
            if (est.Result is { } r)
            {
                var cost = r.TryGetProperty("estimated_cost_usd", out var c) && c.ValueKind == JsonValueKind.Number ? $"，约 ${c.GetDouble():0.000}" : "（未配置单价，不显示金额）";
                estimate.Text = $"预计输入约 {Int(r, "estimated_input_tokens"):N0} tokens，输出约 {Int(r, "estimated_output_tokens"):N0} tokens{cost}。以上为估算值，实际用量以服务商为准。";
                needsConfirm = Bool(r, "requires_confirmation");
                if (needsConfirm)
                {
                    confirmBox.Visibility = Visibility.Visible;
                    dlg.IsPrimaryButtonEnabled = false;
                    confirmBox.Checked += (_, _) => dlg.IsPrimaryButtonEnabled = true;
                    confirmBox.Unchecked += (_, _) => dlg.IsPrimaryButtonEnabled = false;
                }
            }
            else
            {
                estimate.Text = est.Error is { } er ? $"无法估算：{(Advice(er.Code).Length > 0 ? Advice(er.Code) : er.Message)}" : "";
            }
        };
        if (await dlg.ShowAsync() != ContentDialogResult.Primary) return null;

        var kv = new List<(string, object?)>();
        if (topic.Text.Trim().Length > 0) kv.Add(("topic", topic.Text.Trim()));
        if ((lang.SelectedItem as ComboBoxItem)?.Tag is string l && l.Length > 0) kv.Add(("lang", l));
        if (needsConfirm) kv.Add(("confirmed", true));
        switch (kind)
        {
            case "quiz":
                kv.Add(("count", (int)count.Value));
                kv.Add(("difficulty", (difficulty.SelectedItem as ComboBoxItem)?.Tag as string ?? "mixed"));
                var types = typeBoxes.Where(c => c.IsChecked == true).Select(c => (string)c.Tag).ToArray();
                if (types.Length > 0) kv.Add(("types", types));
                break;
            case "flashcards":
                kv.Add(("count", (int)count.Value));
                break;
            case "report":
                kv.Add(("format", (variant!.SelectedItem as ComboBoxItem)?.Tag as string ?? "document"));
                break;
            case "slides":
                kv.Add(("pages", (int)slidePages!.Value));
                kv.Add(("reviewOutline", review!.IsChecked == true));
                break;
            case "infographic":
                kv.Add(("orientation", (variant!.SelectedItem as ComboBoxItem)?.Tag as string ?? "landscape"));
                break;
            case "audio":
                kv.Add(("format", (variant!.SelectedItem as ComboBoxItem)?.Tag as string ?? "deep_dive"));
                break;
        }
        return Args(kv.ToArray());
    }
}
