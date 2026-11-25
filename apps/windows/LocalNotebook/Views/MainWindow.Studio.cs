using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

public sealed partial class MainWindow
{
    private readonly Dictionary<string, string> _studioLiveStep = new();
    private readonly Dictionary<string, TextBlock> _studioLiveStepLabels = new();

    private void OnStudioThoughtPreview(JsonElement data)
    {
        var aid = Str(data, "artifact_id");
        var title = Str(data, "title");
        if (string.IsNullOrEmpty(aid) || string.IsNullOrEmpty(title)) return;
        _studioLiveStep[aid] = title;
        if (_studioLiveStepLabels.TryGetValue(aid, out var tb))
        {
            tb.Text = title;
            tb.Visibility = Visibility.Visible;
        }
    }

    private static string KindName(string kind) => kind switch
    {
        "audio" => L10n.T("studio.audio"),
        "slides" => L10n.T("studio.slides"),
        "mindmap" => L10n.T("studio.mindmap"),
        "report" => L10n.T("studio.report"),
        "flashcards" => L10n.T("studio.flashcards"),
        "quiz" => L10n.T("studio.quiz"),
        "infographic" => L10n.T("studio.infographic"),
        "lecture" => L10n.T("studio.lecture"),
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
        _studioLiveStepLabels.Clear();
        _ageUpdaters.Clear();
        var any = false;
        // the core lists newest first; keep that order
        foreach (var a in Arr(art, "artifacts"))
        {
            if (Str(Obj(a, "config"), "mode") == "knowledge_graph" && Str(a, "status") is not ("PENDING" or "RUNNING")) continue;
            any = true;
            ArtifactsList.Items.Add(ArtifactRow(a));
        }
        if (!any) ArtifactsList.Items.Add(Line(L10n.T("studio.empty_hint")));
        if (_ageUpdaters.Count > 0)
        {
            _ageTimer ??= CreateAgeTimer();
            _ageTimer.Start();
        }
        else
        {
            _ageTimer?.Stop();
        }
    }

    private readonly List<Action> _ageUpdaters = new();
    private Microsoft.UI.Xaml.DispatcherTimer? _ageTimer;

    private Microsoft.UI.Xaml.DispatcherTimer CreateAgeTimer()
    {
        var t = new Microsoft.UI.Xaml.DispatcherTimer { Interval = TimeSpan.FromSeconds(30) };
        t.Tick += (_, _) => { foreach (var u in _ageUpdaters) u(); };
        return t;
    }

    /// <summary>"3 分钟前" style age of a timestamp (unix ms).</summary>
    internal static string TimeAgo(long createdMs)
    {
        if (createdMs <= 0) return "";
        var secs = Math.Max(0, (DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() - createdMs) / 1000);
        if (secs < 60) return L10n.T("time.just_now");
        var mins = secs / 60;
        if (mins < 60) return L10n.Fmt("time.minutes", mins);
        var hours = mins / 60;
        if (hours < 24) return L10n.Fmt("time.hours", hours);
        var days = hours / 24;
        if (days < 30) return L10n.Fmt("time.days", days);
        if (days < 365) return L10n.Fmt("time.months", days / 30);
        return L10n.Fmt("time.years", days / 365);
    }

    private UIElement ArtifactRow(JsonElement a)
    {
        var id = Str(a, "id");
        var status = Str(a, "status");
        var rawTitle = Str(a, "title");
        var kind = Str(a, "artifact_type");
        var title = rawTitle.Length > 0 ? rawTitle : KindName(kind);
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
            "lecture" => ("#F28B82", "M20,17A2,2 0 0,0 22,15V4A2,2 0 0,0 20,2H9.46C9.81,2.61 10,3.3 10,4H20V15H11V17M15,7V9H9V22H7V16H5V22H3V14H1.5V9A2,2 0 0,1 3.5,7H15M8,4A2,2 0 0,1 6,6A2,2 0 0,1 4,4A2,2 0 0,1 6,2A2,2 0 0,1 8,4Z"),
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
            "PENDING" or "RUNNING" => L10n.T("studio.click_thoughts"),
            "FAILED" => L10n.T("studio.click_error_log"),
            _ => "",
        };
        var createdAt = Int(a, "created_at");
        var statusLine = new TextBlock { FontSize = 11, Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"], TextWrapping = TextWrapping.Wrap };
        void SetStatusLine()
        {
            var age = TimeAgo(createdAt);
            statusLine.Text = $"{KindName(kind)} · {ArtifactStatusLabel(status, stage)}{(age.Length > 0 ? " · " + age : "")}{hintSuffix}";
        }
        SetStatusLine();
        _ageUpdaters.Add(SetStatusLine);
        titleStack.Children.Add(statusLine);
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
        Add(L10n.T("studio.rename"), async () =>
        {
            var name = await PromptTextAsync(L10n.T("studio.rename_title"), L10n.T("common.name"), title);
            if (!string.IsNullOrWhiteSpace(name)) await CoreAsync("studio.rename", Args(("artifact_id", id), ("title", name.Trim())), L10n.T("studio.rename"));
        });
        if (status is "PENDING" or "RUNNING") Add(L10n.T("studio.cancel"), async () => { await CoreAsync("studio.cancel", Args(("artifact_id", id)), L10n.T("studio.cancel")); });
        if (status is "FAILED" or "CANCELLED") Add(L10n.T("studio.retry"), async () => { await CoreAsync("studio.retry", Args(("artifact_id", id)), L10n.T("studio.retry")); });
        if (status == "READY")
        {
            var existingCfg = a.TryGetProperty("config", out var cfgEl) && cfgEl.ValueKind == JsonValueKind.Object ? cfgEl : (JsonElement?)null;
            Add(L10n.T("studio.update_from_sources"), async () =>
            {
                var updated = await ConfigureArtifactAsync(kind, existingCfg, isUpdate: true);
                if (updated is not { } cfg) return;
                await CoreAsync("studio.regenerate", WithImages(Args(("artifact_id", id), ("config", cfg), ("use_selected_sources", true))), L10n.T("studio.op_update"));
            });
        }
        Add(L10n.T("common.delete"), async () =>
        {
            if (await ConfirmAsync(L10n.T("studio.delete_title"), L10n.Fmt("studio.delete_confirm", title), L10n.T("common.delete")))
                await CoreAsync("studio.delete", Args(("artifact_id", id)), L10n.T("common.delete"));
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
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(more, L10n.T("studio.more_actions"));
        Grid.SetColumn(more, 1);
        head.Children.Add(more);
        col.Children.Add(head);

        if (status is "PENDING" or "RUNNING")
        {
            col.Children.Add(new ProgressBar { Value = Num(a, "progress") * 100, Maximum = 100 });
            _studioLiveStep.TryGetValue(id, out var liveStep);
            var stepTb = new TextBlock
            {
                Text = liveStep ?? "",
                FontSize = 11,
                TextWrapping = TextWrapping.Wrap,
                MaxLines = 2,
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                Visibility = string.IsNullOrEmpty(liveStep) ? Visibility.Collapsed : Visibility.Visible,
            };
            _studioLiveStepLabels[id] = stepTb;
            col.Children.Add(stepTb);
        }
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
                Content = L10n.T("studio.review_outline"),
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
            onRequireFullscreen: ForceStudioFullscreen,
            onQuizConcept: concept => _ = CreateConceptLinkArtifactAsync("quiz", concept),
            onExpandMindmap: concept => _ = CreateConceptLinkArtifactAsync("mindmap", concept),
            onAskConcept: concept => _ = SendChatAsync(L10n.Fmt("kg.explain_prompt", concept)),
            math: _math,
            mathHost: MathRenderHost);
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

    private Storyboard? _paneStoryboard;

    private void ForceStudioFullscreen()
    {
        if (_studioFullscreen) return;
        _studioFullscreen = true;
        if (_sourceFullscreen)
        {
            _sourceFullscreen = false;
            if (_sourceFullscreenIcon != null)
            {
                _sourceFullscreenIcon.Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(
                    typeof(Microsoft.UI.Xaml.Media.Geometry), SourceFullscreenIconPath);
            }
        }
        _activeArtifactPane?.SetFullscreenIcon(true);
        _paneStoryboard?.Stop();
        ApplyPaneLayout();
    }

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

    /// <summary>Applies target pane layout in a single pass and runs a GPU-composited opacity transition to avoid Grid reflow jitter.</summary>
    private void AnimatePaneLayout(Border targetPane, bool expanding)
    {
        _paneStoryboard?.Stop();
        _paneStoryboard = null;
        targetPane.RenderTransform = null;

        ApplyPaneLayout();

        if (!L10n.Prefs.FullscreenAnimation || targetPane.Visibility != Visibility.Visible)
        {
            targetPane.Opacity = 1.0;
            return;
        }

        targetPane.Opacity = 0.85;
        var anim = new DoubleAnimation
        {
            From = 0.85,
            To = 1.0,
            Duration = new Duration(TimeSpan.FromMilliseconds(140)),
            EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut },
        };
        Storyboard.SetTarget(anim, targetPane);
        Storyboard.SetTargetProperty(anim, "Opacity");
        var sb = new Storyboard();
        sb.Children.Add(anim);
        sb.Completed += (_, _) =>
        {
            if (ReferenceEquals(_paneStoryboard, sb))
                _paneStoryboard = null;
            targetPane.Opacity = 1.0;
        };
        _paneStoryboard = sb;
        sb.Begin();
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
        var topic = L10n.Fmt("kg.default_topic", sourceTitle);
        var cfg = Args(("mode", "knowledge_graph"), ("topic", topic), ("source_id", sourceId), ("lang", L10n.TargetLangCode));
        var title = L10n.Fmt("kg.title_with_source", sourceTitle);
        var res = await CoreAsync("studio.create", Args(("notebook_id", _activeNotebookId), ("artifact_type", "mindmap"), ("title", title), ("config", cfg)), L10n.T("kg.title"));
        if (res != null) ShowStatus(InfoBarSeverity.Success, L10n.T("kg.title"), L10n.T("kg.building_msg"));
        await RefreshNotebookDataAsync();
    }

    private async Task CreateConceptLinkArtifactAsync(string artifactType, string concept)
    {
        if (_activeNotebookId == null) return;
        var cfg = artifactType == "quiz"
            ? Args(("topic", L10n.Fmt("kg.quiz_topic", concept)), ("quantity", "standard"), ("count", 8), ("difficulty", "auto"))
            : Args(("topic", L10n.Fmt("kg.mindmap_topic", concept)));
        var title = artifactType == "quiz"
            ? L10n.Fmt("kg.quiz_title", concept)
            : L10n.Fmt("kg.mindmap_title", concept);
        var res = await CoreAsync("studio.create", Args(("notebook_id", _activeNotebookId), ("artifact_type", artifactType), ("title", title), ("config", cfg)), KindName(artifactType));
        if (res != null) ShowStatus(InfoBarSeverity.Success, title, L10n.T("studio.queued_msg"));
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
        var res = await CoreAsync("studio.create", WithImages(Args(("notebook_id", _activeNotebookId), ("artifact_type", actualType), ("config", finalConfig))), KindName(kind));
        if (res != null) ShowStatus(InfoBarSeverity.Success, KindName(kind), L10n.T("studio.queued_msg"));
        await RefreshNotebookDataAsync();
    }

    /// <summary>Adds the dialog's pictures (if any) to a request, then forgets them.</summary>
    private JsonElement WithImages(JsonElement args)
    {
        var atts = _cfgImages;
        _cfgImages = null;
        return ImageAttachments.Merge(args, atts);
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

    private static (StackPanel Container, Func<string> GetSelected) BuildSegmentedOption(
        string header,
        (string Label, string Tag)[] options,
        string defaultTag)
    {
        var selected = options.Any(o => o.Tag == defaultTag) ? defaultTag : options[0].Tag;
        var wrapper = new StackPanel { Spacing = 6 };
        wrapper.Children.Add(new TextBlock
        {
            Text = header,
            FontSize = 13,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
        });
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 2 };
        var buttons = new List<(Button Btn, FontIcon Check, TextBlock Text, string Tag)>();
        var activeBg = (Brush)Application.Current.Resources["HoverBgBrush"];
        var transparentBg = new SolidColorBrush(Microsoft.UI.Colors.Transparent);

        var thumbTransform = new TranslateTransform();
        var thumb = new Border
        {
            Background = activeBg,
            CornerRadius = new CornerRadius(16),
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Stretch,
            IsHitTestVisible = false,
            RenderTransform = thumbTransform,
        };
        var trackGrid = new Grid();
        trackGrid.Children.Add(thumb);
        trackGrid.Children.Add(row);

        Storyboard? activeStoryboard = null;
        var isTransitioning = false;
        var isAnimating = false;

        void SnapThumb()
        {
            var selBtn = buttons.FirstOrDefault(b => b.Tag == selected).Btn;
            if (selBtn == null) return;
            var targetW = selBtn.ActualWidth;
            if (targetW <= 0) return;
            var targetX = selBtn.TransformToVisual(trackGrid).TransformPoint(new Windows.Foundation.Point(0, 0)).X;
            isAnimating = false;
            activeStoryboard?.Stop();
            thumbTransform.X = targetX;
            thumb.Width = targetW;
        }

        void SyncButtonStates()
        {
            foreach (var (btn, chk, txt, tag) in buttons)
            {
                var isSel = tag == selected;
                btn.Background = transparentBg;
                chk.Visibility = isSel ? Visibility.Visible : Visibility.Collapsed;
                txt.FontWeight = isSel ? Microsoft.UI.Text.FontWeights.SemiBold : Microsoft.UI.Text.FontWeights.Normal;
            }
        }

        void AnimateSelection(string newTag)
        {
            var prevBtn = buttons.FirstOrDefault(b => b.Tag == selected).Btn;
            var curX = thumbTransform.X;
            var curW = !double.IsNaN(thumb.Width) && thumb.Width > 0
                ? thumb.Width
                : (prevBtn?.ActualWidth ?? 0);

            selected = newTag;
            isTransitioning = true;
            SyncButtonStates();
            trackGrid.UpdateLayout();
            isTransitioning = false;

            var selBtn = buttons.FirstOrDefault(b => b.Tag == selected).Btn;
            if (selBtn == null) return;
            var targetW = selBtn.ActualWidth;
            if (targetW <= 0) return;
            var targetX = selBtn.TransformToVisual(trackGrid).TransformPoint(new Windows.Foundation.Point(0, 0)).X;

            if (curW <= 0)
            {
                SnapThumb();
                return;
            }

            isAnimating = false;
            activeStoryboard?.Stop();
            thumbTransform.X = curX;
            thumb.Width = curW;

            var sb = new Storyboard();
            var ease = new CubicEase { EasingMode = EasingMode.EaseOut };
            var dur = new Duration(TimeSpan.FromMilliseconds(240));

            var animX = new DoubleAnimation
            {
                From = curX,
                To = targetX,
                Duration = dur,
                EasingFunction = ease,
                FillBehavior = FillBehavior.HoldEnd,
            };
            Storyboard.SetTarget(animX, thumbTransform);
            Storyboard.SetTargetProperty(animX, "X");

            var animW = new DoubleAnimation
            {
                From = curW,
                To = targetW,
                Duration = dur,
                EasingFunction = ease,
                EnableDependentAnimation = true,
                FillBehavior = FillBehavior.HoldEnd,
            };
            Storyboard.SetTarget(animW, thumb);
            Storyboard.SetTargetProperty(animW, "Width");

            sb.Children.Add(animX);
            sb.Children.Add(animW);
            sb.Completed += (_, _) =>
            {
                if (ReferenceEquals(activeStoryboard, sb))
                    isAnimating = false;
            };
            activeStoryboard = sb;
            isAnimating = true;
            sb.Begin();
        }

        foreach (var (label, tag) in options)
        {
            var chk = new FontIcon
            {
                Glyph = "\uE73E",
                FontSize = 11,
                VerticalAlignment = VerticalAlignment.Center,
            };
            var txt = new TextBlock
            {
                Text = label,
                FontSize = 13,
                VerticalAlignment = VerticalAlignment.Center,
            };
            var content = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 5 };
            content.Children.Add(chk);
            content.Children.Add(txt);
            var capturedTag = tag;
            var btn = new Button
            {
                Content = content,
                CornerRadius = new CornerRadius(16),
                Padding = new Thickness(12, 6, 12, 6),
                BorderThickness = new Thickness(0),
            };
            btn.Click += (_, _) =>
            {
                if (selected == capturedTag) return;
                AnimateSelection(capturedTag);
            };
            buttons.Add((btn, chk, txt, capturedTag));
            row.Children.Add(btn);
        }

        SyncButtonStates();
        row.Loaded += (_, _) => SnapThumb();
        row.SizeChanged += (_, _) =>
        {
            if (!isTransitioning && !isAnimating)
                SnapThumb();
        };

        var border = new Border
        {
            CornerRadius = new CornerRadius(18),
            BorderThickness = new Thickness(1),
            BorderBrush = (Brush)Application.Current.Resources["StrongBorderBrush"],
            Background = (Brush)Application.Current.Resources["PanelBgBrush"],
            HorizontalAlignment = HorizontalAlignment.Left,
            Padding = new Thickness(2),
            Child = trackGrid,
        };
        wrapper.Children.Add(border);
        return (wrapper, () => selected);
    }

    private static (StackPanel Container, Func<string[]> GetSelected) BuildMultiPillOption(
        string header,
        (string Label, string Tag, bool DefaultChecked)[] options)
    {
        var selected = new HashSet<string>(options.Where(o => o.DefaultChecked).Select(o => o.Tag));
        var wrapper = new StackPanel { Spacing = 6 };
        wrapper.Children.Add(new TextBlock
        {
            Text = header,
            FontSize = 13,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
        });
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        var activeBg = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["HoverBgBrush"];
        var panelBg = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["PanelBgBrush"];
        var strongBorder = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["StrongBorderBrush"];
        var subtleBorder = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["SubtleBorderBrush"];

        foreach (var (label, tag, _) in options)
        {
            var chk = new FontIcon { Glyph = "\uE73E", FontSize = 11, VerticalAlignment = VerticalAlignment.Center };
            var txt = new TextBlock { Text = label, FontSize = 13, VerticalAlignment = VerticalAlignment.Center };
            var content = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 5 };
            content.Children.Add(chk);
            content.Children.Add(txt);
            var capturedTag = tag;
            var btn = new Button
            {
                Content = content,
                CornerRadius = new CornerRadius(16),
                Padding = new Thickness(12, 6, 12, 6),
                BorderThickness = new Thickness(1),
            };

            void SyncPill()
            {
                var on = selected.Contains(capturedTag);
                btn.Background = on ? activeBg : panelBg;
                btn.BorderBrush = on ? strongBorder : subtleBorder;
                chk.Visibility = on ? Visibility.Visible : Visibility.Collapsed;
                txt.FontWeight = on ? Microsoft.UI.Text.FontWeights.SemiBold : Microsoft.UI.Text.FontWeights.Normal;
            }

            SyncPill();
            btn.Click += (_, _) =>
            {
                if (selected.Contains(capturedTag))
                {
                    if (selected.Count > 1) selected.Remove(capturedTag);
                }
                else
                {
                    selected.Add(capturedTag);
                }
                SyncPill();
            };
            row.Children.Add(btn);
        }

        wrapper.Children.Add(row);
        return (wrapper, () => options.Where(o => selected.Contains(o.Tag)).Select(o => o.Tag).ToArray());
    }

    /// <summary>Pictures attached in the last generation dialog (null when none); sent as <c>images</c>.</summary>
    private JsonElement? _cfgImages;

    private async Task<JsonElement?> ConfigureArtifactAsync(string kind, JsonElement? initialConfig = null, bool isUpdate = false)
    {
        _cfgImages = null;
        var panel = new StackPanel { Spacing = 12, MinWidth = 460 };
        if (isUpdate)
        {
            panel.Children.Add(new TextBlock
            {
                Text = L10n.T("studio.cfg.update_hint"),
                FontSize = 12,
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                TextWrapping = TextWrapping.Wrap,
            });
        }
        var topic = new TextBox
        {
            Header = L10n.T("studio.cfg.topic_header"),
            PlaceholderText = L10n.T("studio.cfg.topic_placeholder"),
            Text = initialConfig is { } ic0 ? Str(ic0, "topic") : "",
            AcceptsReturn = true,
            TextWrapping = TextWrapping.Wrap,
            MinHeight = 96,
            MaxHeight = 200,
        };
        ScrollViewer.SetVerticalScrollBarVisibility(topic, ScrollBarVisibility.Auto);
        var lang = new ComboBox { Header = L10n.T("studio.cfg.lang_header"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var initLang = initialConfig is { } ic1 && Str(ic1, "lang").Length > 0
            ? Str(ic1, "lang")
            : L10n.Prefs.DefaultStudioLang;
        var langTags = new[]
        {
            (L10n.T("studio.cfg.lang_source"), ""),
            (L10n.T("studio.cfg.lang_zh"), "zh-CN"),
            (L10n.T("studio.cfg.lang_en"), "en"),
        };
        foreach (var (label, tag) in langTags)
            lang.Items.Add(new ComboBoxItem { Content = label, Tag = tag });
        lang.SelectedIndex = Math.Max(0, Array.FindIndex(langTags, x => x.Item2 == initLang));
        panel.Children.Add(topic);
        var cfgImages = new ImageAttachments(
            () => WinRT.Interop.WindowNative.GetWindowHandle(this),
            () => Content.XamlRoot,
            msg => ShowStatus(InfoBarSeverity.Warning, L10n.T("img.title"), msg));
        var imageRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        imageRow.Children.Add(cfgImages.AttachButton);
        imageRow.Children.Add(new TextBlock
        {
            Text = isUpdate ? L10n.T("img.cfg_hint_update") : L10n.T("img.cfg_hint"),
            FontSize = 12,
            Opacity = 0.7,
            VerticalAlignment = VerticalAlignment.Center,
            TextWrapping = TextWrapping.Wrap,
        });
        panel.Children.Add(imageRow);
        panel.Children.Add(cfgImages.Strip);
        cfgImages.Hook(panel, topic);
        panel.Children.Add(lang);

        Func<string>? getQuantity = null;
        Func<string>? getDifficulty = null;
        Func<string[]>? getTypes = null;
        Func<string>? getDetail = null;
        Func<string>? getFormat = null;
        Func<string>? getDepth = null;
        Func<string>? getLength = null;
        Func<string>? getOrientation = null;
        Func<string>? getMode = null;

        string InitStr(string key, string fallback) =>
            initialConfig is { } ic && Str(ic, key) is { Length: > 0 } v ? v : fallback;

        switch (kind)
        {
            case "mindmap":
            case "knowledge_graph":
            {
                var (ctl, getter) = BuildSegmentedOption(
                    L10n.T("studio.cfg.detail_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.level_concise"), "concise"),
                        (L10n.T("studio.cfg.level_balanced"), "standard"),
                        (L10n.T("studio.cfg.level_detailed"), "detailed"),
                    },
                    InitStr("detail", "standard"));
                getDetail = getter;
                panel.Children.Add(ctl);
                break;
            }
            case "quiz":
            {
                var (qtyCtl, qtyGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.quantity_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.qty_fewer"), "fewer"),
                        (L10n.T("studio.cfg.qty_standard"), "standard"),
                        (L10n.T("studio.cfg.qty_more"), "more"),
                    },
                    InitStr("quantity", "standard"));
                getQuantity = qtyGet;
                panel.Children.Add(qtyCtl);

                var (diffCtl, diffGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.difficulty_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.diff_auto"), "auto"),
                        (L10n.T("studio.cfg.diff_mixed"), "mixed"),
                        (L10n.T("studio.cfg.diff_easy"), "easy"),
                        (L10n.T("studio.cfg.diff_medium"), "medium"),
                        (L10n.T("studio.cfg.diff_hard"), "hard"),
                    },
                    InitStr("difficulty", "auto"));
                getDifficulty = diffGet;
                panel.Children.Add(diffCtl);

                var (typesCtl, typesGet) = BuildMultiPillOption(
                    L10n.T("studio.cfg.types_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.type_single"), "single", true),
                        (L10n.T("studio.cfg.type_multi"), "multi", true),
                        (L10n.T("studio.cfg.type_fill"), "fill", false),
                        (L10n.T("studio.cfg.type_short"), "short", false),
                    });
                getTypes = typesGet;
                panel.Children.Add(typesCtl);
                break;
            }
            case "flashcards":
            {
                var (qtyCtl, qtyGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.quantity_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.qty_fewer"), "fewer"),
                        (L10n.T("studio.cfg.qty_standard"), "standard"),
                        (L10n.T("studio.cfg.qty_more"), "more"),
                    },
                    InitStr("quantity", "standard"));
                getQuantity = qtyGet;
                panel.Children.Add(qtyCtl);

                var (diffCtl, diffGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.difficulty_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.diff_auto"), "auto"),
                        (L10n.T("studio.cfg.diff_mixed"), "mixed"),
                        (L10n.T("studio.cfg.diff_easy"), "easy"),
                        (L10n.T("studio.cfg.diff_medium"), "medium"),
                        (L10n.T("studio.cfg.diff_hard"), "hard"),
                    },
                    InitStr("difficulty", "auto"));
                getDifficulty = diffGet;
                panel.Children.Add(diffCtl);
                break;
            }
            case "report":
            {
                var (fmtCtl, fmtGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.format_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.report_doc"), "document"),
                        (L10n.T("studio.cfg.report_interactive"), "interactive"),
                    },
                    InitStr("format", "document"));
                getFormat = fmtGet;
                panel.Children.Add(fmtCtl);

                var (depthCtl, depthGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.detail_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.level_concise"), "brief"),
                        (L10n.T("studio.cfg.level_balanced"), "standard"),
                        (L10n.T("studio.cfg.level_detailed"), "deep"),
                    },
                    InitStr("depth", "standard"));
                getDepth = depthGet;
                panel.Children.Add(depthCtl);
                break;
            }
            case "slides":
            {
                var (lenCtl, lenGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.slides_length_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.level_concise"), "concise"),
                        (L10n.T("studio.cfg.level_balanced"), "standard"),
                        (L10n.T("studio.cfg.level_detailed"), "detailed"),
                    },
                    InitStr("length", "standard"));
                getLength = lenGet;
                panel.Children.Add(lenCtl);

                break;
            }
            case "lecture":
            {
                topic.Header = L10n.T("studio.cfg.lecture_topic_header");
                topic.PlaceholderText = L10n.T("studio.cfg.lecture_topic_placeholder");
                var (modeCtl, modeGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.lecture_mode_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.lecture_mode_general"), "general"),
                        (L10n.T("studio.cfg.lecture_mode_preview"), "preview"),
                        (L10n.T("studio.cfg.lecture_mode_review"), "review"),
                        (L10n.T("studio.cfg.lecture_mode_practice"), "practice"),
                    },
                    InitStr("mode", "general"));
                getMode = modeGet;
                panel.Children.Add(modeCtl);
                var (detCtl, detGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.detail_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.level_auto"), "auto"),
                        (L10n.T("studio.cfg.level_concise"), "brief"),
                        (L10n.T("studio.cfg.level_balanced"), "standard"),
                        (L10n.T("studio.cfg.level_detailed"), "detailed"),
                    },
                    InitStr("detail", "auto"));
                getDetail = detGet;
                panel.Children.Add(detCtl);
                panel.Children.Add(new TextBlock { Text = L10n.T("studio.cfg.lecture_hint"), FontSize = 12, Opacity = 0.7, TextWrapping = TextWrapping.Wrap });
                break;
            }
            case "infographic":
            {
                var (oriCtl, oriGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.orientation_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.orient_landscape"), "landscape"),
                        (L10n.T("studio.cfg.orient_portrait"), "portrait"),
                        (L10n.T("studio.cfg.orient_square"), "square"),
                    },
                    InitStr("orientation", "landscape"));
                getOrientation = oriGet;
                panel.Children.Add(oriCtl);
                break;
            }
            case "audio":
            {
                var (fmtCtl, fmtGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.format_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.audio_deep_dive"), "deep_dive"),
                        (L10n.T("studio.cfg.audio_debate"), "debate"),
                        (L10n.T("studio.cfg.audio_brief"), "brief"),
                        (L10n.T("studio.cfg.audio_critique"), "critique"),
                    },
                    InitStr("format", "deep_dive"));
                getFormat = fmtGet;
                panel.Children.Add(fmtCtl);

                var (lenCtl, lenGet) = BuildSegmentedOption(
                    L10n.T("studio.cfg.audio_length_header"),
                    new[]
                    {
                        (L10n.T("studio.cfg.level_concise"), "short"),
                        (L10n.T("studio.cfg.level_balanced"), "standard"),
                        (L10n.T("studio.cfg.level_detailed"), "long"),
                    },
                    InitStr("length", "standard"));
                getLength = lenGet;
                panel.Children.Add(lenCtl);

                panel.Children.Add(new TextBlock { Text = L10n.T("studio.cfg.audio_hint"), FontSize = 12, Opacity = 0.7, TextWrapping = TextWrapping.Wrap });
                break;
            }
        }

        var supportsOutlineSplit = kind is "slides" or "report";
        var isLecture = kind == "lecture";
        var dlg = new ContentDialog
        {
            Title = isUpdate ? L10n.Fmt("studio.cfg.title_update", KindName(kind)) : L10n.Fmt("studio.cfg.title_create", KindName(kind)),
            Content = new ScrollViewer { Content = panel, MaxHeight = 560 },
            // lectures confirm the resolved scope + outline first by default
            PrimaryButtonText = isLecture
                ? L10n.T("studio.cfg.btn_lecture_confirm_scope")
                : supportsOutlineSplit
                    ? L10n.T("studio.cfg.btn_one_click")
                    : (isUpdate ? L10n.T("studio.cfg.btn_update") : L10n.T("studio.cfg.btn_create")),
            SecondaryButtonText = isLecture ? L10n.T("studio.cfg.btn_one_click") : supportsOutlineSplit ? L10n.T("studio.cfg.btn_outline_preview") : "",
            CloseButtonText = L10n.T("common.cancel"),
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = Content.XamlRoot,
        };
        var dlgRes = await dlg.ShowAsync();
        if (dlgRes == ContentDialogResult.None) return null;
        var wantOutlinePreview = supportsOutlineSplit && dlgRes == ContentDialogResult.Secondary;

        _cfgImages = cfgImages.ToArgs();
        var kv = new List<(string, object?)>();
        if (topic.Text.Trim().Length > 0) kv.Add(("topic", topic.Text.Trim()));
        if ((lang.SelectedItem as ComboBoxItem)?.Tag is string l && l.Length > 0) kv.Add(("lang", l));
        switch (kind)
        {
            case "quiz":
            {
                var q = getQuantity?.Invoke() ?? "standard";
                kv.Add(("quantity", q));
                kv.Add(("count", q switch { "fewer" => 4, "more" => 15, _ => 8 }));
                kv.Add(("difficulty", getDifficulty?.Invoke() ?? "mixed"));
                var types = getTypes?.Invoke() ?? Array.Empty<string>();
                if (types.Length > 0) kv.Add(("types", types));
                break;
            }
            case "mindmap":
            case "knowledge_graph":
                kv.Add(("detail", getDetail?.Invoke() ?? "standard"));
                break;
            case "flashcards":
            {
                var q = getQuantity?.Invoke() ?? "standard";
                kv.Add(("quantity", q));
                kv.Add(("count", q switch { "fewer" => 6, "more" => 24, _ => 12 }));
                kv.Add(("difficulty", getDifficulty?.Invoke() ?? "mixed"));
                break;
            }
            case "report":
                kv.Add(("format", getFormat?.Invoke() ?? "document"));
                kv.Add(("depth", getDepth?.Invoke() ?? "standard"));
                kv.Add(("reviewOutline", wantOutlinePreview));
                break;
            case "slides":
                kv.Add(("length", getLength?.Invoke() ?? "standard"));
                kv.Add(("reviewOutline", wantOutlinePreview));
                break;
            case "infographic":
                kv.Add(("orientation", getOrientation?.Invoke() ?? "landscape"));
                break;
            case "lecture":
                kv.Add(("mode", getMode?.Invoke() ?? "general"));
                kv.Add(("detail", getDetail?.Invoke() ?? "auto"));
                kv.Add(("reviewOutline", dlgRes == ContentDialogResult.Primary));
                break;
            case "audio":
                kv.Add(("format", getFormat?.Invoke() ?? "deep_dive"));
                kv.Add(("length", getLength?.Invoke() ?? "standard"));
                break;
        }
        return Args(kv.ToArray());
    }
}
