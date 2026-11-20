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
        var card = new Border { Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"], CornerRadius = new CornerRadius(10), Padding = new Thickness(10), Margin = new Thickness(0, 0, 0, 6) };
        var col = new StackPanel { Spacing = 4 };
        var head = new Grid { ColumnSpacing = 6 };
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        head.Children.Add(new TextBlock { Text = $"{title}", FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap });

        var menu = new MenuFlyout();
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
        var more = new Button { Content = "⋯", Flyout = menu, Padding = new Thickness(8, 0, 8, 0) };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(more, "产物操作");
        Grid.SetColumn(more, 1);
        head.Children.Add(more);
        col.Children.Add(head);

        var stage = Str(a, "stage");
        col.Children.Add(new TextBlock { Text = $"{KindName(kind)} · {ArtifactStatusLabel(status, stage)}", FontSize = 12, Opacity = 0.7 });
        if (status is "PENDING" or "RUNNING") col.Children.Add(new ProgressBar { Value = Num(a, "progress") * 100, Maximum = 100 });
        var err = Str(a, "error_code");
        if (status == "FAILED")
        {
            var advice = Advice(err);
            col.Children.Add(new TextBlock { Text = advice.Length > 0 ? advice : Str(a, "error_detail"), TextWrapping = TextWrapping.Wrap, FontSize = 12, Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 160, 140)) });
        }
        if (status == "READY" && Bool(a, "sources_stale"))
            col.Children.Add(new TextBlock { Text = "来源已更新，可重新生成", FontSize = 12, Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 200, 120)) });
        if (status is "READY" or "DRAFT_REVIEW")
        {
            var open = new Button { Content = status == "READY" ? "打开" : "审核大纲", HorizontalAlignment = HorizontalAlignment.Left };
            open.Click += (_, _) => OpenArtifact(id);
            col.Children.Add(open);
        }
        card.Child = col;
        return card;
    }

    private readonly Dictionary<string, ArtifactWindow> _artifactWindows = new();

    private void OpenArtifact(string id)
    {
        if (_artifactWindows.TryGetValue(id, out var existing))
        {
            existing.Activate();
            return;
        }
        var w = new ArtifactWindow(id);
        _artifactWindows[id] = w;
        w.Closed += (_, _) => _artifactWindows.Remove(id);
        w.Activate();
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
