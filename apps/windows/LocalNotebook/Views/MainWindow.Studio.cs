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
        "audio" => L10n.T("studio.audio"),
        "slides" => L10n.T("studio.slides"),
        "mindmap" => L10n.T("studio.mindmap"),
        "report" => L10n.T("studio.report"),
        "flashcards" => L10n.T("studio.flashcards"),
        "quiz" => L10n.T("studio.quiz"),
        "infographic" => L10n.T("studio.infographic"),
        "knowledge_graph" => L10n.T("kg.title"),
        _ => kind,
    };

    private static string ArtifactStatusLabel(string status, string stage) => status switch
    {
        "PENDING" => L10n.T("studio.status_pending"),
        "RUNNING" => stage switch
        {
            "COLLECT" => L10n.T("studio.status_collect"),
            "PLAN" => L10n.T("studio.status_plan"),
            "GENERATE" => L10n.T("studio.status_generate"),
            "RENDER" => L10n.T("studio.status_render"),
            _ => L10n.T("studio.status_processing"),
        },
        "DRAFT_REVIEW" => L10n.T("studio.status_draft"),
        "READY" => L10n.T("studio.status_ready"),
        "FAILED" => L10n.T("studio.status_failed"),
        "CANCELLED" => L10n.T("studio.status_cancelled"),
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
        var title = L10n.LocalizeDefaultArtifactTitle(Str(a, "title"));
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
        var kindIcon = new Viewbox
        {
            Width = 18,
            Height = 18,
            VerticalAlignment = VerticalAlignment.Center,
            HorizontalAlignment = HorizontalAlignment.Center,
            Child = new PathIcon
            {
                Width = 24,
                Height = 24,
                Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), kindIconData.Item2),
                Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(iconColor),
            },
        };
        Grid.SetColumn(kindIcon, 0);
        row.Children.Add(kindIcon);

        var titleStack = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        titleStack.Children.Add(new TextBlock { Text = $"{title}", FontSize = 13, FontWeight = Microsoft.UI.Text.FontWeights.Medium, TextWrapping = TextWrapping.Wrap });
        var stage = Str(a, "stage");
        var hintSuffix = status switch
        {
            "PENDING" or "RUNNING" => " · 点击查看思考过程",
            "FAILED" => " · 点击查看错误与日志",
            _ => "",
        };
        titleStack.Children.Add(new TextBlock { Text = $"{KindName(kind)} · {ArtifactStatusLabel(status, stage)}{hintSuffix}", FontSize = 11, Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"] });
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
        if (status == "READY")
        {
            var existingCfg = a.TryGetProperty("config", out var cfgEl) && cfgEl.ValueKind == JsonValueKind.Object ? cfgEl : (JsonElement?)null;
            Add("按当前来源与要求更新", async () =>
            {
                var updated = await ConfigureArtifactAsync(kind, existingCfg, isUpdate: true);
                if (updated is not { } cfg) return;
                await CoreAsync("studio.regenerate", Args(("artifact_id", id), ("config", cfg), ("use_selected_sources", true)), "更新产物");
            });
        }
        Add("删除", async () =>
        {
            if (await ConfirmAsync("删除产物", $"确定删除“{title}”吗？", "删除")) await CoreAsync("studio.delete", Args(("artifact_id", id)), "删除");
        });
        var more = new Button
        {
            Content = new Viewbox
            {
                Width = 18,
                Height = 18,
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
                Child = new PathIcon
                {
                    Width = 24,
                    Height = 24,
                    Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M6,10A2,2 0 0,0 4,12A2,2 0 0,0 6,14A2,2 0 0,0 8,12A2,2 0 0,0 6,10M12,10A2,2 0 0,0 10,12A2,2 0 0,0 12,14A2,2 0 0,0 14,12A2,2 0 0,0 12,10M18,10A2,2 0 0,0 16,12A2,2 0 0,0 18,14A2,2 0 0,0 20,12A2,2 0 0,0 18,10Z"),
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                },
            },
            Flyout = menu,
            MinWidth = 36,
            Width = 36,
            MinHeight = 36,
            Height = 36,
            Padding = new Thickness(0),
            CornerRadius = new CornerRadius(18),
            HorizontalContentAlignment = HorizontalAlignment.Center,
            VerticalContentAlignment = VerticalAlignment.Center,
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
        card.Tapped += (_, _) => OpenArtifact(id);
        card.Child = col;
        return card;
    }

    private ArtifactViewerPane? _activeArtifactPane;
    private bool _studioFullscreen;

    private void OpenArtifact(string id)
    {
        var old = _activeArtifactPane;
        _activeArtifactPane = null;
        old?.Dispose();
        _studioFullscreen = false;
        if (_sourceFullscreen)
        {
            _sourceFullscreen = false;
            if (_sourceFullscreenIcon != null)
            {
                _sourceFullscreenIcon.Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(
                    typeof(Microsoft.UI.Xaml.Media.Geometry), SourceFullscreenIconPath);
            }
        }
        var pane = new ArtifactViewerPane(
            id,
            this,
            onClose: CloseArtifactInline,
            onToggleFullscreen: ToggleStudioFullscreen,
            onQuizConcept: concept => _ = CreateConceptLinkArtifactAsync("quiz", concept),
            onExpandMindmap: concept => _ = CreateConceptLinkArtifactAsync("mindmap", concept),
            onAskConcept: concept => _ = SendChatAsync(L10n.IsEnglish ? $"Explain the concept '{concept}' in detail with key theorems and examples." : $"请详细讲解知识点「{concept}」的核心定义、推导思路与典型应用。"));
        _activeArtifactPane = pane;
        StudioDetailHost.Content = pane.Root;
        StudioHomeView.Visibility = Visibility.Collapsed;
        StudioDetailHost.Visibility = Visibility.Visible;
        ApplyPaneLayout();
    }

    private void CloseArtifactInline()
    {
        var old = _activeArtifactPane;
        _activeArtifactPane = null;
        _studioFullscreen = false;
        old?.Dispose();
        StudioDetailHost.Visibility = Visibility.Collapsed;
        StudioDetailHost.Content = null;
        StudioHomeView.Visibility = Visibility.Visible;
        ApplyPaneLayout();
    }

    private DispatcherTimer? _paneAnimTimer;

    private void ToggleStudioFullscreen()
    {
        _studioFullscreen = !_studioFullscreen;
        if (_studioFullscreen)
        {
            _sourceFullscreen = false;
            if (_sourceFullscreenIcon != null)
            {
                _sourceFullscreenIcon.Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(
                    typeof(Microsoft.UI.Xaml.Media.Geometry), SourceFullscreenIconPath);
            }
        }
        _activeArtifactPane?.SetFullscreenIcon(_studioFullscreen);
        AnimatePaneLayout(StudioPaneBorder, _studioFullscreen);
    }

    /// <summary>Smoothly animates pane width expansion or shrinking (~190ms cubic ease-out) when toggling fullscreen.</summary>
    private void AnimatePaneLayout(Border targetPane, bool expanding)
    {
        if (!L10n.Prefs.FullscreenAnimation || RootGrid == null || RootGrid.ActualWidth <= 100)
        {
            ApplyPaneLayout();
            return;
        }

        _paneAnimTimer?.Stop();
        var startS = SourcesPaneBorder.Visibility == Visibility.Visible ? SourcesPaneBorder.ActualWidth : 0.0;
        var startC = ChatPaneBorder.Visibility == Visibility.Visible ? ChatPaneBorder.ActualWidth : 0.0;
        var startT = StudioPaneBorder.Visibility == Visibility.Visible ? StudioPaneBorder.ActualWidth : 0.0;

        var totalW = Math.Max(300.0, RootGrid.ActualWidth - 56.0);
        double targetS, targetC, targetT;
        var studioOpened = _activeArtifactPane != null;

        if (_sourceFullscreen && _sourcePreviewOpen)
        {
            targetS = totalW; targetC = 0; targetT = 0;
        }
        else if (_studioFullscreen && studioOpened)
        {
            targetS = 0; targetC = 0; targetT = totalW;
        }
        else if (_sourcePreviewOpen && studioOpened)
        {
            targetS = totalW * (1.0 / 3.2); targetC = totalW * (1.0 / 3.2); targetT = totalW * (1.2 / 3.2);
        }
        else if (_sourcePreviewOpen)
        {
            targetT = 300;
            var rem = Math.Max(100, totalW - targetT);
            targetS = rem * 0.5; targetC = rem * 0.5;
        }
        else if (studioOpened)
        {
            targetS = 260;
            var rem = Math.Max(100, totalW - targetS);
            targetC = rem * (1.0 / 2.65); targetT = rem * (1.65 / 2.65);
        }
        else
        {
            targetS = 320; targetT = 340; targetC = Math.Max(100, totalW - targetS - targetT);
        }

        SourcesPaneBorder.Visibility = Visibility.Visible;
        ChatPaneBorder.Visibility = Visibility.Visible;
        StudioPaneBorder.Visibility = Visibility.Visible;

        var scale = new Microsoft.UI.Xaml.Media.ScaleTransform { ScaleX = expanding ? 0.96 : 1.02, ScaleY = expanding ? 0.96 : 1.02 };
        targetPane.RenderTransformOrigin = new Windows.Foundation.Point(0.5, 0.5);
        targetPane.RenderTransform = scale;

        var started = DateTime.UtcNow;
        const double durationMs = 190.0;
        var timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(16) };
        _paneAnimTimer = timer;
        timer.Tick += (_, _) =>
        {
            var elapsed = (DateTime.UtcNow - started).TotalMilliseconds;
            var rawT = Math.Clamp(elapsed / durationMs, 0.0, 1.0);
            var ease = 1.0 - Math.Pow(1.0 - rawT, 3.0);

            SourcesCol.Width = new GridLength(Math.Max(0, startS + (targetS - startS) * ease), GridUnitType.Pixel);
            ChatCol.Width = new GridLength(Math.Max(0, startC + (targetC - startC) * ease), GridUnitType.Pixel);
            StudioCol.Width = new GridLength(Math.Max(0, startT + (targetT - startT) * ease), GridUnitType.Pixel);

            var curScale = (expanding ? 0.96 : 1.02) + (1.0 - (expanding ? 0.96 : 1.02)) * ease;
            scale.ScaleX = curScale;
            scale.ScaleY = curScale;

            if (rawT >= 1.0)
            {
                timer.Stop();
                targetPane.RenderTransform = null;
                ApplyPaneLayout();
            }
        };
        timer.Start();
    }

    private void ApplyPaneLayout()
    {
        var studioOpened = _activeArtifactPane != null;
        if (_sourceFullscreen && _sourcePreviewOpen)
        {
            SourcesPaneBorder.Visibility = Visibility.Visible;
            ChatPaneBorder.Visibility = Visibility.Collapsed;
            StudioPaneBorder.Visibility = Visibility.Collapsed;
            SourcesCol.Width = new GridLength(1, GridUnitType.Star);
            ChatCol.Width = new GridLength(0);
            StudioCol.Width = new GridLength(0);
        }
        else if (_studioFullscreen && studioOpened)
        {
            SourcesPaneBorder.Visibility = Visibility.Collapsed;
            ChatPaneBorder.Visibility = Visibility.Collapsed;
            StudioPaneBorder.Visibility = Visibility.Visible;
            SourcesCol.Width = new GridLength(0);
            ChatCol.Width = new GridLength(0);
            StudioCol.Width = new GridLength(1, GridUnitType.Star);
        }
        else
        {
            SourcesPaneBorder.Visibility = Visibility.Visible;
            ChatPaneBorder.Visibility = Visibility.Visible;
            StudioPaneBorder.Visibility = Visibility.Visible;
            if (_sourcePreviewOpen && studioOpened)
            {
                SourcesCol.Width = new GridLength(1, GridUnitType.Star);
                ChatCol.Width = new GridLength(1, GridUnitType.Star);
                StudioCol.Width = new GridLength(1.2, GridUnitType.Star);
            }
            else if (_sourcePreviewOpen)
            {
                SourcesCol.Width = new GridLength(1.2, GridUnitType.Star);
                ChatCol.Width = new GridLength(1.2, GridUnitType.Star);
                StudioCol.Width = new GridLength(300);
            }
            else if (studioOpened)
            {
                SourcesCol.Width = new GridLength(260);
                ChatCol.Width = new GridLength(1, GridUnitType.Star);
                StudioCol.Width = new GridLength(1.65, GridUnitType.Star);
            }
            else
            {
                SourcesCol.Width = new GridLength(320);
                ChatCol.Width = new GridLength(1, GridUnitType.Star);
                StudioCol.Width = new GridLength(340);
            }
        }
    }

    // ---------- creating ----------

    private async Task CreateKnowledgeGraphAsync(string sourceId, string sourceTitle)
    {
        if (_activeNotebookId == null) return;
        var topic = L10n.IsEnglish
            ? $"Build a structured Knowledge Graph for '{sourceTitle}' covering core concepts, prerequisites, and relationships."
            : $"基于《{sourceTitle}》构建知识点图谱，梳理核心概念、前置依赖与定理体系。";
        var cfg = Args(("mode", "knowledge_graph"), ("topic", topic), ("source_id", sourceId), ("lang", L10n.TargetLangCode));
        var title = L10n.IsEnglish ? $"{sourceTitle} - Knowledge Graph" : $"{sourceTitle} · 知识点图谱";
        var res = await CoreAsync("studio.create", Args(("notebook_id", _activeNotebookId), ("artifact_type", "mindmap"), ("title", title), ("config", cfg)), L10n.T("kg.title"));
        if (res != null) ShowStatus(InfoBarSeverity.Success, L10n.T("kg.title"), L10n.IsEnglish ? "Building Knowledge Graph in background." : "已开始在后台构建知识点图谱。");
        await RefreshNotebookDataAsync();
    }

    private async Task CreateConceptLinkArtifactAsync(string artifactType, string concept)
    {
        if (_activeNotebookId == null) return;
        var cfg = artifactType == "quiz"
            ? Args(("topic", L10n.IsEnglish ? $"Focused quiz on concept: {concept}" : $"针对知识点「{concept}」的专项测验"), ("count", 5), ("difficulty", "mixed"))
            : Args(("topic", L10n.IsEnglish ? $"Detailed mind map expanding concept: {concept}" : $"深入展开知识点「{concept}」的详细思维导图"));
        var title = artifactType == "quiz"
            ? (L10n.IsEnglish ? $"Quiz: {concept}" : $"专项测验：{concept}")
            : (L10n.IsEnglish ? $"Mind Map: {concept}" : $"思维导图：{concept}");
        var res = await CoreAsync("studio.create", Args(("notebook_id", _activeNotebookId), ("artifact_type", artifactType), ("title", title), ("config", cfg)), KindName(artifactType));
        if (res != null) ShowStatus(InfoBarSeverity.Success, title, L10n.IsEnglish ? "Started generating in background." : "已加入后台生成队列。");
        await RefreshNotebookDataAsync();
    }

    private async void OnStudioTileClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null || sender is not Button btn || btn.Tag is not string kind) return;
        var cfg = await ConfigureArtifactAsync(kind);
        if (cfg is not { } config) return;
        var actualType = kind == "knowledge_graph" ? "mindmap" : kind;
        var finalConfig = kind == "knowledge_graph"
            ? MergeKnowledgeGraphMode(config)
            : config;
        var res = await CoreAsync("studio.create", Args(("notebook_id", _activeNotebookId), ("artifact_type", actualType), ("config", finalConfig)), KindName(kind));
        if (res != null) ShowStatus(InfoBarSeverity.Success, KindName(kind), L10n.IsEnglish ? "Submitted to background queue." : "已加入后台生成队列。");
        await RefreshNotebookDataAsync();
    }

    private static JsonElement MergeKnowledgeGraphMode(JsonElement cfg)
    {
        var kv = new List<(string, object?)> { ("mode", "knowledge_graph") };
        if (cfg.ValueKind == JsonValueKind.Object)
        {
            foreach (var p in cfg.EnumerateObject())
            {
                if (p.Name != "mode") kv.Add((p.Name, p.Value));
            }
        }
        return Args(kv.ToArray());
    }

    private async Task<JsonElement?> ConfigureArtifactAsync(string kind, JsonElement? initialConfig = null, bool isUpdate = false)
    {
        var panel = new StackPanel { Spacing = 10, MinWidth = 440 };
        if (isUpdate)
        {
            panel.Children.Add(new TextBlock
            {
                Text = "将按以下要求与当前已勾选的来源重新生成此产物。",
                FontSize = 12,
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                TextWrapping = TextWrapping.Wrap,
            });
        }
        var topic = new TextBox
        {
            Header = "主题或要求（可选）",
            PlaceholderText = "例如：只关注第三章的基与维数",
            Text = initialConfig is { } ic0 ? Str(ic0, "topic") : "",
        };
        var lang = new ComboBox { Header = "语言", HorizontalAlignment = HorizontalAlignment.Stretch };
        var initLang = initialConfig is { } ic1 ? Str(ic1, "lang") : "";
        var langTags = new[] { ("跟随来源", ""), ("中文", "zh-CN"), ("English", "en") };
        foreach (var (label, tag) in langTags)
            lang.Items.Add(new ComboBoxItem { Content = label, Tag = tag });
        lang.SelectedIndex = Math.Max(0, Array.FindIndex(langTags, x => x.Item2 == initLang));
        panel.Children.Add(topic);
        panel.Children.Add(lang);

        var count = new NumberBox { Header = kind == "flashcards" ? "卡片数量" : "题目数量", Value = kind == "flashcards" ? 10 : 5, Minimum = 3, Maximum = 50, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
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
                slidePages = new NumberBox { Header = "页数（默认 8 页，建议 6~12 页）", Value = 8, Minimum = 4, Maximum = 40, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
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
        var dlg = new ContentDialog { Title = isUpdate ? $"更新{KindName(kind)}" : $"生成{KindName(kind)}", Content = new ScrollViewer { Content = panel, MaxHeight = 560 }, PrimaryButtonText = isUpdate ? "更新生成" : "生成", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Primary, XamlRoot = Content.XamlRoot };
        dlg.Opened += async (_, _) =>
        {
            var estType = kind == "knowledge_graph" ? "mindmap" : kind;
            var estCfg = kind == "knowledge_graph" ? Args(("mode", "knowledge_graph")) : Args();
            var est = await Api.CallAsync("studio.estimate", Args(("notebook_id", _activeNotebookId), ("artifact_type", estType), ("config", estCfg)));
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
