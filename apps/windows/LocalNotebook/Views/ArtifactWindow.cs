using LocalNotebook.Services;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using System.Text;
using System.Text.Json;
using Windows.Storage.Pickers;
using WinRT.Interop;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>
/// Inline right-side Studio artifact viewer pane (replaces popup ArtifactWindow per Requirement 3).
/// Supports fullscreen toggle, Infographic zoomable canvas (Requirement 4), Report TOC+Content (Requirement 4),
/// Slides Markdown outline review without raw JSON (Requirement 5), and zero emojis (Requirement 6).
/// </summary>
public sealed class ArtifactViewerPane : IDisposable
{
    private const string FullscreenIconPath = "M5,5H10V7H7V10H5V5M14,5H19V10H17V7H14V5M17,14H19V19H14V17H17V14M10,17V19H5V14H7V17H10Z";
    private const string ExitFullscreenIconPath = "M14,14H19V16H16V19H14V14M5,14H10V19H8V16H5V14M8,5H10V10H5V8H8V5M19,8V10H14V5H16V8H19Z";
    private const string CloseIconPath = "M19,6.41L17.59,5L12,10.59L6.41,5L5,6.41L10.59,12L5,17.59L6.41,19L12,13.41L17.59,19L19,17.59L13.41,12L19,6.41Z";
    private const string ThumbUpPath = "M23,10C23,8.89 22.1,8 21,8H14.68L15.64,3.43C15.66,3.33 15.67,3.22 15.67,3.11C15.67,2.7 15.5,2.32 15.23,2.05L14.17,1L7.59,7.58C7.22,7.95 7,8.45 7,9V19A2,2 0 0,0 9,21H18C18.83,21 19.54,20.5 19.84,19.78L22.86,12.73C22.95,12.5 23,12.26 23,12V10M1,21H5V9H1V21Z";
    private const string ThumbDownPath = "M19,15H23V3H19M15,3H6C5.17,3 4.46,3.5 4.16,4.22L1.14,11.27C1.05,11.5 1,11.74 1,12V14A2,2 0 0,0 3,16H9.31L8.36,20.57C8.34,20.67 8.33,20.77 8.33,20.88C8.33,21.3 8.5,21.67 8.77,21.94L9.83,23L16.41,16.41C16.78,16.05 17,15.55 17,15V5A2,2 0 0,0 15,3Z";
    private const string CheckIconPath = "M9,16.17L4.83,12L3.41,13.41L9,19L21,7L19.59,5.59L9,16.17Z";
    private const string CrossIconPath = "M19,6.41L17.59,5L12,10.59L6.41,5L5,6.41L10.59,12L5,17.59L6.41,19L12,13.41L17.59,19L19,17.59L13.41,12L19,6.41Z";
    private const string PrevIconPath = "M15.41,16.58L10.83,12L15.41,7.41L14,6L8,12L14,18L15.41,16.58Z";
    private const string NextIconPath = "M8.59,16.58L13.17,12L8.59,7.41L10,6L16,12L10,18L8.59,16.58Z";
    private const string PlusIconPath = "M19,13H13V19H11V13H5V11H11V5H13V11H19V13Z";
    private const string MinusIconPath = "M19,13H5V11H19V13Z";
    private const string SparkleEditIconPath = "M12,2L14.4,8.6L21,11L14.4,13.4L12,20L9.6,13.4L3,11L9.6,8.6L12,2Z";
    private const string FolderIconPath = "M10,4H4C2.89,4 2,4.89 2,6V18A2,2 0 0,0 4,20H20A2,2 0 0,0 22,18V8C22,6.89 21.1,6 20,6H12L10,4Z";
    private const string RegenerateIconPath = "M17.65,6.35C16.2,4.9 14.21,4 12,4A8,8 0 0,0 4,12A8,8 0 0,0 12,20C15.73,20 18.84,17.45 19.73,14H17.65C16.83,16.33 14.61,18 12,18A6,6 0 0,1 6,12A6,6 0 0,1 12,6C13.66,6 15.14,6.69 16.22,7.78L13,11H20V4L17.65,6.35Z";
    private const string CopyIconPath = "M19,21H8V7H19M19,5H8A2,2 0 0,0 6,7V21A2,2 0 0,0 8,23H19A2,2 0 0,0 21,21V7A2,2 0 0,0 19,5M16,1H4A2,2 0 0,0 2,3V17H4V3H16V1Z";

    public Grid Root { get; } = new();

    private readonly string _id;
    private readonly Window _hostWindow;
    private readonly Action _onClose;
    private readonly Action _onToggleFullscreen;
    private readonly TextBlock _title = new() { FontSize = 17, FontWeight = FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center };
    private readonly Button _promptSourcesBtn = new()
    {
        Style = (Style)Application.Current.Resources["PillButtonStyle"],
        MinHeight = 28,
        Padding = new Thickness(12, 3, 12, 3),
        FontSize = 12,
        HorizontalAlignment = HorizontalAlignment.Left,
        Margin = new Thickness(0, 4, 0, 0),
        Visibility = Visibility.Collapsed,
    };
    private readonly StackPanel _tools = new() { Orientation = Orientation.Horizontal, Spacing = 6, HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Center };
    private PathIcon? _fullscreenIcon;
    private bool _isFullscreen;
    private readonly ContentControl _body = new() { HorizontalContentAlignment = HorizontalAlignment.Stretch, VerticalContentAlignment = VerticalAlignment.Stretch };
    private readonly InfoBar _bar = new() { IsOpen = false, IsClosable = true };
    private JsonElement _art;
    private string _builtFor = "";
    private string _builtStatus = "";
    private bool _showingThoughts;
    private InteractiveArtifactView? _interactiveView;
    private TextBlock? _genStageText;
    private ProgressBar? _genProgressBar;
    private Button? _genCancelBtn;
    private ScrollViewer? _thoughtScroll;
    private StackPanel? _thoughtPanel;
    private TextBlock? _thoughtEmptyHint;
    private readonly Dictionary<int, TextBlock> _thoughtBodyBySeq = new();
    private bool _disposed;

    private static Geometry Geom(string path) =>
        (Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Geometry), path);

    public ArtifactViewerPane(string id, Window hostWindow, Action onClose, Action onToggleFullscreen)
    {
        _id = id;
        _hostWindow = hostWindow;
        _onClose = onClose;
        _onToggleFullscreen = onToggleFullscreen;

        Root.RowSpacing = 10;
        Root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        Root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        Root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });

        var head = new Grid { ColumnSpacing = 10 };
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        var titleCol = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        titleCol.Children.Add(_title);
        titleCol.Children.Add(_promptSourcesBtn);
        head.Children.Add(titleCol);

        Grid.SetColumn(_tools, 1);
        head.Children.Add(_tools);
        Root.Children.Add(head);

        Grid.SetRow(_body, 1);
        Root.Children.Add(_body);

        Grid.SetRow(_bar, 2);
        Root.Children.Add(_bar);

        App.Core.EventReceived += OnEvent;
        _ = LoadAsync();
    }

    public void SetFullscreenIcon(bool isFullscreen)
    {
        _isFullscreen = isFullscreen;
        if (_fullscreenIcon != null)
        {
            _fullscreenIcon.Data = Geom(isFullscreen ? ExitFullscreenIconPath : FullscreenIconPath);
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        App.Core.EventReceived -= OnEvent;
        _interactiveView?.Dispose();
        _interactiveView = null;
        _thoughtBodyBySeq.Clear();
        _thoughtPanel = null;
        _thoughtScroll = null;
        _thoughtEmptyHint = null;
        _genStageText = null;
        _genProgressBar = null;
        _genCancelBtn = null;
        try { _body.Content = null; } catch { }
        try { Root.Children.Clear(); } catch { }
    }

    private void OnEvent(Interop.ApiEventDto ev)
    {
        if (_disposed || Str(ev.Data, "artifact_id") != _id) return;
        if (ev.Event == "studio.thought")
        {
            var data = ev.Data.Clone();
            _hostWindow.DispatcherQueue.TryEnqueue(() =>
            {
                if (!_disposed) HandleThoughtAdded(data);
            });
            return;
        }
        if (ev.Event == "studio.thought_delta")
        {
            var data = ev.Data.Clone();
            _hostWindow.DispatcherQueue.TryEnqueue(() =>
            {
                if (!_disposed) HandleThoughtDelta(data);
            });
            return;
        }
        if (ev.Event != "studio.status") return;
        var stData = ev.Data.Clone();
        _hostWindow.DispatcherQueue.TryEnqueue(async () =>
        {
            if (_disposed) return;
            var status = Str(stData, "status");
            var stage = Str(stData, "stage");
            var progress = Num(stData, "progress");
            if (_builtStatus is "PENDING" or "RUNNING"
                && status is "PENDING" or "RUNNING"
                && _genStageText != null
                && _genProgressBar != null)
            {
                _genStageText.Text = FormatGenStage(status, stage);
                _genProgressBar.IsIndeterminate = progress <= 0.01;
                _genProgressBar.Value = progress * 100;
                return;
            }
            await LoadAsync();
        });
    }

    private void Say(InfoBarSeverity sev, string title, string msg)
    {
        _bar.Severity = sev;
        _bar.Title = title;
        _bar.Message = msg;
        _bar.IsOpen = true;
    }

    private async Task<JsonElement?> Core(string method, JsonElement args, string what)
    {
        var (res, err) = await Api.CallAsync(method, args);
        if (err is { } e)
        {
            Say(InfoBarSeverity.Error, L10n.Fmt("common.failed", what), Advice(e.Code) is { Length: > 0 } a ? a : e.Message);
            return null;
        }
        return res;
    }

    private async Task LoadAsync()
    {
        if (_disposed) return;
        var res = await Core("studio.get", Args(("artifact_id", _id)), "读取产物");
        if (_disposed || res is not { } a) return;
        _art = a;
        _title.Text = Str(a, "title");
        UpdatePromptAndSourcesPopover(a);

        var status = Str(a, "status");
        var kind = Str(a, "artifact_type");
        var key = $"{status}:{Str(a, "updated_at")}:{Int(a, "updated_at")}";
        if (key == _builtFor) return;
        _builtFor = key;
        if (_builtStatus != status) _showingThoughts = false;
        _builtStatus = status;

        BuildTools(kind, status);
        RenderCurrentBody(kind, status, a);
    }

    private void RenderCurrentBody(string kind, string status, JsonElement a)
    {
        _interactiveView?.Dispose();
        _interactiveView = null;
        _thoughtBodyBySeq.Clear();
        _thoughtPanel = null;
        _thoughtScroll = null;
        _thoughtEmptyHint = null;
        _genStageText = null;
        _genProgressBar = null;
        _genCancelBtn = null;

        if (status == "READY")
        {
            if (_showingThoughts)
            {
                _body.Content = BuildRecordedThoughtsView(a, headerText: L10n.T("studio.process_records"));
                return;
            }
            _interactiveView = new InteractiveArtifactView(
                _id, kind, a, _hostWindow, Say, compact: false, feedbackBar: BuildFeedbackBar(),
                onExportSvg: () => _ = ExportAsync("svg", ".svg", "SVG"));
            _body.Content = _interactiveView.Root;
            return;
        }

        _body.Content = status switch
        {
            "DRAFT_REVIEW" => BuildDraftReview(a),
            "FAILED" => BuildFailedView(a),
            "CANCELLED" => BuildCancelledView(a),
            _ => BuildGeneratingView(a),
        };
    }

    private static string FormatGenStage(string status, string stage) => status switch
    {
        "PENDING" => "正在排队等待后台工作线程处理…",
        _ => stage switch
        {
            "COLLECT" => "步骤 1/3 · 正在检索与整理来源资料…",
            "PLAN" => "步骤 2/3 · 正在规划大纲结构…",
            "GENERATE" => "步骤 2/3 · 正在思考并生成结构化内容…",
            "RENDER" => "步骤 3/3 · 正在渲染输出文件…",
            _ => "正在后台处理中…",
        },
    };

    private FrameworkElement BuildGeneratingView(JsonElement art)
    {
        var grid = new Grid { RowSpacing = 12 };
        grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        grid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });

        var status = Str(art, "status");
        var stage = Str(art, "stage");
        var progress = Num(art, "progress");

        var topCard = new Border
        {
            Background = (Brush)Application.Current.Resources["ElevatedBgBrush"],
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(12),
            Padding = new Thickness(14),
        };
        var topStack = new StackPanel { Spacing = 8 };
        var topRow = new Grid { ColumnSpacing = 10 };
        topRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        topRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        _genStageText = new TextBlock
        {
            Text = FormatGenStage(status, stage),
            FontSize = 13,
            FontWeight = FontWeights.SemiBold,
            TextWrapping = TextWrapping.Wrap,
            VerticalAlignment = VerticalAlignment.Center,
        };
        topRow.Children.Add(_genStageText);

        _genCancelBtn = new Button
        {
            Content = "取消生成",
            Style = (Style)Application.Current.Resources["PillButtonStyle"],
            MinHeight = 30,
            Padding = new Thickness(12, 4, 12, 4),
            FontSize = 12,
        };
        _genCancelBtn.Click += async (_, _) =>
        {
            if (_genCancelBtn != null) _genCancelBtn.IsEnabled = false;
            await Core("studio.cancel", Args(("artifact_id", _id)), "取消");
            await LoadAsync();
        };
        Grid.SetColumn(_genCancelBtn, 1);
        topRow.Children.Add(_genCancelBtn);
        topStack.Children.Add(topRow);

        _genProgressBar = new ProgressBar
        {
            IsIndeterminate = progress <= 0.01,
            Value = progress * 100,
            Maximum = 100,
        };
        topStack.Children.Add(_genProgressBar);
        topCard.Child = topStack;
        grid.Children.Add(topCard);

        var processGrid = new Grid { RowSpacing = 8 };
        processGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        processGrid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        processGrid.Children.Add(new TextBlock
        {
            Text = "思考与执行过程",
            FontSize = 13,
            FontWeight = FontWeights.SemiBold,
            Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
        });

        _thoughtPanel = new StackPanel { Spacing = 8, Margin = new Thickness(0, 0, 8, 8) };
        foreach (var t in Arr(art, "thoughts"))
        {
            AppendThoughtCard((int)Int(t, "seq"), Str(t, "kind"), Str(t, "title"), Str(t, "body"), scroll: false);
        }
        if (_thoughtPanel.Children.Count == 0)
        {
            _thoughtEmptyHint = new TextBlock
            {
                Text = "等待后台任务开始输出过程与思考内容…",
                FontSize = 12,
                Foreground = (Brush)Application.Current.Resources["TextMutedBrush"],
            };
            _thoughtPanel.Children.Add(_thoughtEmptyHint);
        }

        _thoughtScroll = new ScrollViewer
        {
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            Content = _thoughtPanel,
        };
        Grid.SetRow(_thoughtScroll, 1);
        processGrid.Children.Add(_thoughtScroll);

        Grid.SetRow(processGrid, 1);
        grid.Children.Add(processGrid);
        return grid;
    }

    private void HandleThoughtAdded(JsonElement data)
    {
        if (_thoughtPanel == null)
        {
            if (_builtStatus is "PENDING" or "RUNNING")
                _ = LoadAsync();
            return;
        }
        var seq = (int)Int(data, "seq");
        var kind = Str(data, "kind");
        var title = Str(data, "title");
        var body = Str(data, "body");
        AppendThoughtCard(seq, kind, title, body, scroll: true);
    }

    private void HandleThoughtDelta(JsonElement data)
    {
        var seq = (int)Int(data, "seq");
        var delta = Str(data, "delta");
        if (delta.Length == 0) return;
        if (_thoughtBodyBySeq.TryGetValue(seq, out var tb))
        {
            tb.Text += delta;
            tb.Visibility = Visibility.Visible;
            if (_thoughtScroll != null)
            {
                _thoughtScroll.UpdateLayout();
                _thoughtScroll.ChangeView(null, _thoughtScroll.ScrollableHeight, null, disableAnimation: true);
            }
            return;
        }
        if (_builtStatus is "PENDING" or "RUNNING")
            _ = LoadAsync();
    }

    private void AppendThoughtCard(int seq, string kind, string title, string body, bool scroll)
    {
        if (_thoughtPanel == null) return;
        if (_thoughtEmptyHint != null)
        {
            _thoughtPanel.Children.Remove(_thoughtEmptyHint);
            _thoughtEmptyHint = null;
        }
        if (seq > 0 && _thoughtBodyBySeq.TryGetValue(seq, out var existingBody))
        {
            if (body.Length > existingBody.Text.Length)
            {
                existingBody.Text = body;
                existingBody.Visibility = Visibility.Visible;
            }
            return;
        }

        var (card, bodyTb) = BuildThoughtCard(kind, title, body);
        if (seq > 0) _thoughtBodyBySeq[seq] = bodyTb;
        _thoughtPanel.Children.Add(card);
        if (scroll && _thoughtScroll != null)
        {
            _thoughtScroll.UpdateLayout();
            _thoughtScroll.ChangeView(null, _thoughtScroll.ScrollableHeight, null, disableAnimation: true);
        }
    }

    private static (Border Card, TextBlock BodyBlock) BuildThoughtCard(string kind, string title, string body)
    {
        var accentColor = kind switch
        {
            "retry" => Windows.UI.Color.FromArgb(255, 253, 214, 99),
            "error" => Windows.UI.Color.FromArgb(255, 242, 139, 130),
            "reasoning" => Windows.UI.Color.FromArgb(255, 168, 199, 250),
            _ => Windows.UI.Color.FromArgb(255, 129, 201, 149),
        };
        var bodyTb = new TextBlock
        {
            Text = body,
            FontSize = 12,
            TextWrapping = TextWrapping.Wrap,
            IsTextSelectionEnabled = true,
            Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
            Visibility = body.Length > 0 ? Visibility.Visible : Visibility.Collapsed,
        };

        var stack = new StackPanel { Spacing = 4 };
        var head = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        head.Children.Add(new Border
        {
            Width = 6,
            Height = 6,
            CornerRadius = new CornerRadius(3),
            Background = new SolidColorBrush(accentColor),
            VerticalAlignment = VerticalAlignment.Center,
        });
        head.Children.Add(new TextBlock
        {
            Text = title.Length > 0 ? title : kind,
            FontSize = 12,
            FontWeight = FontWeights.SemiBold,
            TextWrapping = TextWrapping.Wrap,
        });
        stack.Children.Add(head);
        stack.Children.Add(bodyTb);

        var card = new Border
        {
            Background = (Brush)Application.Current.Resources["ElevatedBgBrush"],
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(10),
            Padding = new Thickness(12, 8, 12, 8),
            Child = stack,
        };
        return (card, bodyTb);
    }

    private FrameworkElement BuildRecordedThoughtsView(JsonElement art, string headerText)
    {
        var root = new StackPanel { Spacing = 10, Margin = new Thickness(0, 4, 8, 8) };
        root.Children.Add(new TextBlock
        {
            Text = headerText,
            FontSize = 13,
            FontWeight = FontWeights.SemiBold,
            Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
        });
        var thoughts = Arr(art, "thoughts").ToList();
        if (thoughts.Count == 0)
        {
            root.Children.Add(new TextBlock
            {
                Text = "该产物未记录详细思考步骤。",
                FontSize = 12,
                Foreground = (Brush)Application.Current.Resources["TextMutedBrush"],
            });
        }
        else
        {
            foreach (var t in thoughts.Where(x => Str(x, "kind") != "retry"))
            {
                root.Children.Add(BuildThoughtCard(Str(t, "kind"), Str(t, "title"), Str(t, "body")).Card);
            }
        }
        return new ScrollViewer
        {
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            Content = root,
        };
    }

    private FrameworkElement BuildFailedView(JsonElement art)
    {
        var root = new StackPanel { Spacing = 14, Margin = new Thickness(0, 4, 8, 8) };
        var errBox = new Border
        {
            Background = (Brush)Application.Current.Resources["ElevatedBgBrush"],
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(12),
            Padding = new Thickness(14),
            Child = ArtifactErrorPanel.Build(art, Say, RetryAsync, onViewProcess: null, compact: false),
        };
        root.Children.Add(errBox);

        var thoughts = Arr(art, "thoughts").Where(x => Str(x, "kind") != "retry").ToList();
        if (thoughts.Count > 0)
        {
            root.Children.Add(new TextBlock
            {
                Text = L10n.T("studio.process_records"),
                FontSize = 13,
                FontWeight = FontWeights.SemiBold,
                Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
            });
            foreach (var t in thoughts)
            {
                root.Children.Add(BuildThoughtCard(Str(t, "kind"), Str(t, "title"), Str(t, "body")).Card);
            }
        }

        return new ScrollViewer
        {
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            Content = root,
        };
    }

    private FrameworkElement BuildCancelledView(JsonElement art)
    {
        var root = new StackPanel { Spacing = 12, Margin = new Thickness(0, 8, 8, 8) };
        root.Children.Add(new TextBlock
        {
            Text = "已停止生成。",
            FontSize = 13,
            Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
        });
        var retryBtn = new Button
        {
            Content = "重新生成",
            Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"],
            MinHeight = 32,
            Padding = new Thickness(14, 4, 14, 4),
            FontSize = 12,
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        retryBtn.Click += async (_, _) => await RetryAsync();
        root.Children.Add(retryBtn);

        foreach (var t in Arr(art, "thoughts"))
        {
            root.Children.Add(BuildThoughtCard(Str(t, "kind"), Str(t, "title"), Str(t, "body")).Card);
        }

        return new ScrollViewer
        {
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            Content = root,
        };
    }

    /// <summary>Builds the NotebookLM-style "查看提示和 N 个来源" pill button and rich popover flyout.</summary>
    private void UpdatePromptAndSourcesPopover(JsonElement art)
    {
        var cfg = Obj(art, "config");
        var promptText = Str(cfg, "topic").Trim();
        var srcRefs = Arr(art, "source_refs").ToList();
        var srcCount = srcRefs.Count > 0 ? srcRefs.Count : Arr(art, "source_ids").Count();

        _promptSourcesBtn.Content = srcCount > 0
            ? L10n.Fmt("studio.view_prompt_sources", srcCount)
            : L10n.T("studio.view_prompt");
        _promptSourcesBtn.Visibility = Visibility.Visible;

        var flyoutBody = new StackPanel { Spacing = 14, MinWidth = 340, MaxWidth = 440 };

        // Prompt section
        var promptHead = new Grid { ColumnSpacing = 8 };
        promptHead.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        promptHead.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        var promptTitleRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
        promptTitleRow.Children.Add(new Viewbox
        {
            Width = 16, Height = 16,
            Child = new PathIcon { Width = 24, Height = 24, Data = Geom(SparkleEditIconPath), Foreground = (Brush)Application.Current.Resources["TextPrimaryBrush"] },
        });
        promptTitleRow.Children.Add(new TextBlock { Text = L10n.T("studio.prompt_header"), FontSize = 14, FontWeight = FontWeights.SemiBold });
        promptHead.Children.Add(promptTitleRow);

        var promptActions = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        var regenBtn = IconBtn(RegenerateIconPath, L10n.T("studio.regenerate"), async (_, _) =>
        {
            (_promptSourcesBtn.Flyout as Flyout)?.Hide();
            var res = await Core("studio.regenerate", Args(("artifact_id", _id), ("config", cfg.ValueKind == JsonValueKind.Object ? cfg : Args()), ("use_selected_sources", true)), L10n.T("studio.regenerate"));
            if (res != null) { _builtFor = ""; await LoadAsync(); }
        }, out _);
        var copyBtn = IconBtn(CopyIconPath, L10n.T("studio.copy_prompt"), (_, _) =>
        {
            var dp = new Windows.ApplicationModel.DataTransfer.DataPackage();
            dp.SetText(promptText.Length > 0 ? promptText : Str(art, "title"));
            Windows.ApplicationModel.DataTransfer.Clipboard.SetContent(dp);
            Say(InfoBarSeverity.Success, L10n.T("studio.prompt_copied"), "");
        }, out _);
        promptActions.Children.Add(regenBtn);
        promptActions.Children.Add(copyBtn);
        Grid.SetColumn(promptActions, 1);
        promptHead.Children.Add(promptActions);
        flyoutBody.Children.Add(promptHead);

        flyoutBody.Children.Add(new TextBlock
        {
            Text = promptText.Length > 0 ? promptText : L10n.T("studio.prompt_empty"),
            FontSize = 13,
            TextWrapping = TextWrapping.Wrap,
            IsTextSelectionEnabled = true,
            Foreground = (Brush)Application.Current.Resources[promptText.Length > 0 ? "TextPrimaryBrush" : "TextMutedBrush"],
        });

        // Sources section
        var srcTitleRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(0, 4, 0, 0) };
        srcTitleRow.Children.Add(new Viewbox
        {
            Width = 15, Height = 15,
            Child = new PathIcon { Width = 24, Height = 24, Data = Geom(FolderIconPath), Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"] },
        });
        srcTitleRow.Children.Add(new TextBlock { Text = L10n.T("studio.sources_header"), FontSize = 13, FontWeight = FontWeights.SemiBold, Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"] });
        flyoutBody.Children.Add(srcTitleRow);

        if (srcRefs.Count > 0)
        {
            var chipsGrid = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
            var chipsWrap = new StackPanel { Spacing = 6 };
            var rowPanel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
            int colCount = 0;
            foreach (var sr in srcRefs)
            {
                var stitle = Str(sr, "title");
                var skind = Str(sr, "kind").ToUpperInvariant();
                var badgeColor = skind switch
                {
                    "PDF" => Windows.UI.Color.FromArgb(255, 229, 83, 75),
                    "MD" or "MARKDOWN" => Windows.UI.Color.FromArgb(255, 108, 158, 248),
                    "DOCX" or "DOC" => Windows.UI.Color.FromArgb(255, 76, 141, 246),
                    _ => Windows.UI.Color.FromArgb(255, 130, 140, 155),
                };
                var badgeTag = skind.Length > 4 ? skind[..4] : (skind.Length > 0 ? skind : "FILE");
                var chipRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
                chipRow.Children.Add(new Border
                {
                    Background = new SolidColorBrush(badgeColor),
                    CornerRadius = new CornerRadius(4),
                    Padding = new Thickness(4, 1, 4, 1),
                    Child = new TextBlock { Text = badgeTag, FontSize = 9, FontWeight = FontWeights.Bold, Foreground = new SolidColorBrush(Microsoft.UI.Colors.White) },
                });
                chipRow.Children.Add(new TextBlock
                {
                    Text = stitle,
                    FontSize = 12,
                    MaxWidth = 165,
                    TextTrimming = TextTrimming.CharacterEllipsis,
                    VerticalAlignment = VerticalAlignment.Center,
                });
                var chip = new Border
                {
                    Background = (Brush)Application.Current.Resources["PanelBgBrush"],
                    BorderBrush = (Brush)Application.Current.Resources["StrongBorderBrush"],
                    BorderThickness = new Thickness(1),
                    CornerRadius = new CornerRadius(14),
                    Padding = new Thickness(10, 5, 12, 5),
                    Child = chipRow,
                };
                ToolTipService.SetToolTip(chip, stitle);
                rowPanel.Children.Add(chip);
                colCount++;
                if (colCount % 2 == 0)
                {
                    chipsWrap.Children.Add(rowPanel);
                    rowPanel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
                }
            }
            if (rowPanel.Children.Count > 0) chipsWrap.Children.Add(rowPanel);
            flyoutBody.Children.Add(chipsWrap);
        }
        else
        {
            flyoutBody.Children.Add(new TextBlock
            {
                Text = srcCount > 0 ? L10n.Fmt("studio.view_prompt_sources", srcCount) : L10n.T("studio.sources_empty"),
                FontSize = 12,
                Foreground = (Brush)Application.Current.Resources["TextMutedBrush"],
            });
        }

        _promptSourcesBtn.Flyout = new Flyout
        {
            Placement = FlyoutPlacementMode.BottomEdgeAlignedLeft,
            Content = flyoutBody,
        };
    }

    private async Task RetryAsync()
    {
        if (_disposed) return;
        var res = await Core("studio.retry", Args(("artifact_id", _id)), "重试");
        if (res != null && !_disposed)
        {
            _builtFor = "";
            await LoadAsync();
        }
    }

    private static Button IconBtn(string path, string tip, RoutedEventHandler onClick, out PathIcon iconRef)
    {
        iconRef = new PathIcon
        {
            Width = 24,
            Height = 24,
            Data = Geom(path),
            Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
        };
        var btn = new Button
        {
            Content = new Viewbox
            {
                Width = 16,
                Height = 16,
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
                Child = iconRef,
            },
            MinWidth = 34,
            Width = 34,
            MinHeight = 34,
            Height = 34,
            Padding = new Thickness(0),
            CornerRadius = new CornerRadius(17),
            HorizontalContentAlignment = HorizontalAlignment.Center,
            VerticalContentAlignment = VerticalAlignment.Center,
            Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
        };
        ToolTipService.SetToolTip(btn, tip);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(btn, tip);
        btn.Click += onClick;
        return btn;
    }

    // ---------- toolbar / export ----------

    private void BuildTools(string kind, string status)
    {
        _tools.Children.Clear();
        if (status == "READY")
        {
            if (Arr(_art, "thoughts").Any())
            {
                var thoughtsBtn = new Button
                {
                    Content = _showingThoughts ? "返回成品" : "思考过程",
                    Style = (Style)Application.Current.Resources["PillButtonStyle"],
                    MinHeight = 34,
                    Padding = new Thickness(12, 4, 12, 4),
                    FontSize = 12,
                };
                thoughtsBtn.Click += (_, _) =>
                {
                    _showingThoughts = !_showingThoughts;
                    thoughtsBtn.Content = _showingThoughts ? "返回成品" : "思考过程";
                    RenderCurrentBody(kind, status, _art);
                };
                _tools.Children.Add(thoughtsBtn);
            }

            var formats = kind switch
            {
                "quiz" => new[] { ("Markdown", "markdown", ".md"), ("JSON", "json", ".json") },
                "flashcards" => new[] { ("CSV", "csv", ".csv"), ("Markdown", "markdown", ".md") },
                "mindmap" => new[] { ("Markdown", "markdown", ".md"), ("SVG", "svg", ".svg") },
                "report" => new[] { ("Markdown", "markdown", ".md"), ("HTML", "html", ".html") },
                "slides" => new[] { ("PPTX", "pptx", ".pptx"), ("Markdown", "markdown", ".md") },
                "infographic" => new[] { ("SVG", "svg", ".svg"), ("Markdown", "markdown", ".md") },
                "audio" => new[] { ("WAV", "wav", ".wav"), ("文字稿", "markdown", ".md") },
                _ => new[] { ("Markdown", "markdown", ".md") },
            };
            var menu = new MenuFlyout { Placement = FlyoutPlacementMode.BottomEdgeAlignedRight };
            foreach (var (label, fmt, ext) in formats)
            {
                var item = new MenuFlyoutItem { Text = $"导出为 {label}" };
                item.Click += async (_, _) => await ExportAsync(fmt, ext, label);
                menu.Items.Add(item);
            }
            _tools.Children.Add(new DropDownButton
            {
                Content = "导出",
                Flyout = menu,
                Style = (Style)Application.Current.Resources["PillDropDownButtonStyle"],
                MinHeight = 34,
                Padding = new Thickness(12, 4, 12, 4),
            });
        }

        _tools.Children.Add(IconBtn(_isFullscreen ? ExitFullscreenIconPath : FullscreenIconPath, "全屏显示 / 还原", (_, _) => _onToggleFullscreen(), out _fullscreenIcon));
        _tools.Children.Add(IconBtn(CloseIconPath, "关闭详情", (_, _) => _onClose(), out _));
    }

    private async Task ExportAsync(string fmt, string ext, string label)
    {
        var picker = new FileSavePicker { SuggestedFileName = Str(_art, "title") };
        picker.FileTypeChoices.Add(label, new List<string> { ext });
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(_hostWindow));
        var file = await picker.PickSaveFileAsync();
        if (file == null) return;
        if (await Core("studio.export", Args(("artifact_id", _id), ("format", fmt), ("out_path", file.Path)), "导出") != null)
            Say(InfoBarSeverity.Success, "已导出", file.Path);
    }

    private StackPanel BuildFeedbackBar()
    {
        var bar = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Margin = new Thickness(0, 8, 0, 0) };
        Button MkFb(string path, string text, int val)
        {
            var sp = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
            sp.Children.Add(new Viewbox { Width = 14, Height = 14, VerticalAlignment = VerticalAlignment.Center, Child = new PathIcon { Width = 24, Height = 24, Data = Geom(path), Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"] } });
            sp.Children.Add(new TextBlock { Text = text, FontSize = 12, Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"] });
            var b = new Button
            {
                Content = sp,
                Style = (Style)Application.Current.Resources["PillButtonStyle"],
                MinHeight = 32,
                Padding = new Thickness(12, 4, 12, 4),
            };
            b.Click += async (_, _) =>
            {
                await Core("studio.set_feedback", Args(("artifact_id", _id), ("feedback", val)), "提交反馈");
                Say(InfoBarSeverity.Success, "已收到反馈", "");
            };
            return b;
        }
        bar.Children.Add(MkFb(ThumbUpPath, "优质内容", 1));
        bar.Children.Add(MkFb(ThumbDownPath, "劣质内容", -1));
        return bar;
    }

    // slides / outline draft review (Requirement 5: Rendered Markdown ONLY, NEVER raw JSON)

    private static string DraftToMarkdown(JsonElement draft)
    {
        if (draft.ValueKind == JsonValueKind.String)
        {
            var raw = draft.GetString() ?? "";
            var trimmed = raw.Trim();
            if (trimmed.StartsWith("{") || trimmed.StartsWith("["))
            {
                try
                {
                    using var doc = JsonDocument.Parse(trimmed);
                    return JsonOutlineToMarkdown(doc.RootElement);
                }
                catch { return raw; }
            }
            return raw;
        }
        if (draft.ValueKind == JsonValueKind.Object || draft.ValueKind == JsonValueKind.Array)
            return JsonOutlineToMarkdown(draft);
        return "";
    }

    private static string JsonOutlineToMarkdown(JsonElement el)
    {
        var sb = new StringBuilder();
        if (el.ValueKind == JsonValueKind.Object)
        {
            var title = Str(el, "title");
            if (title.Length > 0) sb.AppendLine($"# {title}").AppendLine();
            var list = el.TryGetProperty("slides", out var sl) && sl.ValueKind == JsonValueKind.Array ? sl
                : el.TryGetProperty("sections", out var sc) && sc.ValueKind == JsonValueKind.Array ? sc
                : el.TryGetProperty("outline", out var ol) && ol.ValueKind == JsonValueKind.Array ? ol
                : default;
            if (list.ValueKind == JsonValueKind.Array)
            {
                var i = 0;
                foreach (var item in list.EnumerateArray())
                {
                    i++;
                    var t = Str(item, "title");
                    if (t.Length == 0) t = Str(item, "heading");
                    sb.AppendLine($"## 第 {i} 页：{(t.Length > 0 ? t : $"主题 {i}")}");
                    foreach (var b in Arr(item, "bullets")) sb.AppendLine($"- {b.GetString()}");
                    foreach (var b in Arr(item, "points")) sb.AppendLine($"- {b.GetString()}");
                    var sn = Str(item, "speaker_notes");
                    if (sn.Length > 0) sb.AppendLine($"> 讲稿提示：{sn}");
                    sb.AppendLine();
                }
                return sb.ToString().Trim();
            }
            foreach (var prop in el.EnumerateObject())
            {
                sb.AppendLine($"## {prop.Name}");
                sb.AppendLine(prop.Value.ValueKind == JsonValueKind.String ? prop.Value.GetString() : prop.Value.ToString());
                sb.AppendLine();
            }
        }
        return sb.ToString().Trim();
    }

    private FrameworkElement BuildDraftReview(JsonElement art)
    {
        var draft = art.TryGetProperty("draft", out var d) && d.ValueKind != JsonValueKind.Null ? d : default;
        var mdText = DraftToMarkdown(draft);

        var previewHost = new Border
        {
            Background = (Brush)Application.Current.Resources["ElevatedBgBrush"],
            CornerRadius = new CornerRadius(12),
            Padding = new Thickness(16),
            Child = Md.Render(mdText.Length > 0 ? mdText : "（暂无提纲内容）", null, 14),
        };
        var editorBox = new TextBox
        {
            Text = mdText,
            AcceptsReturn = true,
            TextWrapping = TextWrapping.Wrap,
            MinHeight = 320,
            Visibility = Visibility.Collapsed,
            Header = "编辑 Markdown 提纲（修改后点击“预览提纲”可实时查看渲染效果）",
        };

        var previewBtn = new Button { Content = "预览提纲", Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"] };
        var editBtn = new Button { Content = "编辑提纲", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        previewBtn.Click += (_, _) =>
        {
            previewHost.Child = Md.Render(editorBox.Text.Trim().Length > 0 ? editorBox.Text : "（暂无提纲内容）", null, 14);
            previewHost.Visibility = Visibility.Visible;
            editorBox.Visibility = Visibility.Collapsed;
            previewBtn.Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"];
            editBtn.Style = (Style)Application.Current.Resources["PillButtonStyle"];
        };
        editBtn.Click += (_, _) =>
        {
            previewHost.Visibility = Visibility.Collapsed;
            editorBox.Visibility = Visibility.Visible;
            editBtn.Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"];
            previewBtn.Style = (Style)Application.Current.Resources["PillButtonStyle"];
        };

        var tabRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        tabRow.Children.Add(previewBtn);
        tabRow.Children.Add(editBtn);

        var approveBtn = new Button { Content = "批准并生成演示文稿", Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"] };
        approveBtn.Click += async (_, _) =>
        {
            var editedMd = editorBox.Text.Trim();
            var args = editedMd.Length > 0
                ? Args(("artifact_id", _id), ("draft", editedMd))
                : Args(("artifact_id", _id));
            if (await Core("studio.approve_draft", args, "批准提纲") != null)
                Say(InfoBarSeverity.Success, "已批准提纲", "正在通过本地演示文稿工具生成 PPTX，完成后会自动展示。");
        };

        var col = new StackPanel { Spacing = 12 };
        col.Children.Add(new TextBlock { Text = "请先审阅渲染后的 Markdown 提纲，确认或调整后再生成最终演示文稿。", Opacity = 0.8, TextWrapping = TextWrapping.Wrap });
        col.Children.Add(tabRow);
        col.Children.Add(previewHost);
        col.Children.Add(editorBox);
        col.Children.Add(approveBtn);
        return new ScrollViewer { Content = col };
    }
}

/// <summary>
/// Reusable interactive viewer component shared by <see cref="ArtifactViewerPane"/> (right Studio panel)
/// and <see cref="ChatArtifactCard"/> (in-chat inline preview &amp; interactive quiz/flashcards/slides/mindmap/report/audio).
/// </summary>
internal sealed class InteractiveArtifactView : IDisposable
{
    private const string CheckIconPath = "M9,16.17L4.83,12L3.41,13.41L9,19L21,7L19.59,5.59L9,16.17Z";
    private const string CrossIconPath = "M19,6.41L17.59,5L12,10.59L6.41,5L5,6.41L10.59,12L5,17.59L6.41,19L12,13.41L17.59,19L19,17.59L13.41,12L19,6.41Z";
    private const string PrevIconPath = "M15.41,16.58L10.83,12L15.41,7.41L14,6L8,12L14,18L15.41,16.58Z";
    private const string NextIconPath = "M8.59,16.58L13.17,12L8.59,7.41L10,6L16,12L10,18L8.59,16.58Z";
    private const string PlusIconPath = "M19,13H13V19H11V13H5V11H11V5H13V11H19V13Z";
    private const string MinusIconPath = "M19,13H5V11H19V13Z";

    public FrameworkElement Root { get; }

    private readonly string _artifactId;
    private readonly Window _hostWindow;
    private readonly Action<InfoBarSeverity, string, string> _notify;
    private readonly bool _compact;
    private readonly WebContentHost? _math;
    private readonly Panel? _mathHost;
    private readonly FrameworkElement? _feedbackBar;
    private readonly Action? _onExportSvg;
    private readonly JsonElement _art;

    private Windows.Media.Playback.MediaPlayer? _player;
    private Windows.Media.Core.MediaSource? _mediaSource;
    private MediaPlayerElement? _mediaElement;
    private DispatcherTimer? _audioTimer;
    private bool _disposed;

    public InteractiveArtifactView(
        string artifactId,
        string kind,
        JsonElement artifact,
        Window hostWindow,
        Action<InfoBarSeverity, string, string> notify,
        bool compact,
        FrameworkElement? feedbackBar = null,
        WebContentHost? math = null,
        Panel? mathHost = null,
        Action? onExportSvg = null)
    {
        _artifactId = artifactId;
        _art = artifact;
        _hostWindow = hostWindow;
        _notify = notify;
        _compact = compact;
        _feedbackBar = feedbackBar;
        _math = math;
        _mathHost = mathHost;
        _onExportSvg = onExportSvg;

        var payload = Obj(artifact, "payload");
        Root = kind switch
        {
            "quiz" => BuildQuiz(payload),
            "flashcards" => BuildFlashcards(payload),
            "mindmap" => BuildMindmap(payload),
            "report" => BuildReport(payload),
            "slides" => BuildSlides(payload),
            "infographic" => BuildInfographic(payload),
            "audio" => BuildAudio(payload),
            _ => Message("暂不支持查看这种产物。"),
        };
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        DisposeAudio();
    }

    private static Geometry Geom(string path) =>
        (Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Geometry), path);

    private static FrameworkElement Message(string text) =>
        new TextBlock { Text = text, TextWrapping = TextWrapping.Wrap, Margin = new Thickness(0, 12, 0, 12) };

    private static string CiteText(JsonElement item)
    {
        var nums = Arr(item, "citations").Where(c => c.ValueKind == JsonValueKind.Number).Select(c => $"[{c.GetInt32()}]").ToArray();
        return nums.Length > 0 ? " " + string.Join("", nums) : "";
    }

    private void AppendFeedbackIfPresent(Panel panel)
    {
        if (_feedbackBar != null) panel.Children.Add(_feedbackBar);
    }

    private ScrollViewer WrapScroll(FrameworkElement content, double compactMaxHeight = 520)
    {
        var sv = new ScrollViewer { Content = content };
        if (_compact)
        {
            sv.MaxHeight = compactMaxHeight;
            sv.Padding = new Thickness(0, 0, 8, 0);
        }
        return sv;
    }

    private async Task<JsonElement?> CallGradeAsync(string method, JsonElement args, string what)
    {
        var (res, err) = await Api.CallAsync(method, args);
        if (err is { } e)
        {
            _notify(InfoBarSeverity.Error, L10n.Fmt("common.failed", what), Advice(e.Code) is { Length: > 0 } a ? a : e.Message);
            return null;
        }
        return res;
    }

    private void DisposeAudio()
    {
        if (_audioTimer != null)
        {
            _audioTimer.Stop();
            _audioTimer = null;
        }
        if (_mediaElement != null)
        {
            try { _mediaElement.AreTransportControlsEnabled = false; } catch { }
            try { _mediaElement.SetMediaPlayer(null); } catch { }
            try { _mediaElement.Source = null; } catch { }
            if (_mediaElement.Parent is Panel parent)
                parent.Children.Remove(_mediaElement);
            _mediaElement = null;
        }
        var player = _player;
        _player = null;
        var source = _mediaSource;
        _mediaSource = null;
        if (player != null)
        {
            try { player.Pause(); } catch { }
            try { player.Source = null; } catch { }
            _hostWindow.DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () =>
            {
                try { player.Dispose(); } catch { }
                try { source?.Dispose(); } catch { }
            });
        }
        else if (source != null)
        {
            try { source.Dispose(); } catch { }
        }
    }

    // quiz ------------------------------------------------------------------

    private FrameworkElement BuildQuiz(JsonElement payload)
    {
        var panel = new StackPanel { Spacing = _compact ? 10 : 14 };
        var questions = Arr(payload, "questions").ToList();
        if (questions.Count == 0) return Message("没有测验题目。");
        var score = new TextBlock { FontSize = _compact ? 13 : 14, FontWeight = FontWeights.SemiBold, Text = $"共 {questions.Count} 题，作答后点击“提交”查看结果。" };
        panel.Children.Add(score);
        var correct = 0;
        var answered = new HashSet<string>();
        var number = 0;
        var cardBg = (Brush)Application.Current.Resources[_compact ? "PanelBgBrush" : "ElevatedBgBrush"];
        foreach (var q in questions)
        {
            number++;
            var qid = Str(q, "id");
            var type = Str(q, "type");
            var card = new Border
            {
                Background = cardBg,
                BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(12),
                Padding = new Thickness(_compact ? 12 : 14),
            };
            var col = new StackPanel { Spacing = 8 };
            col.Children.Add(new TextBlock { Text = $"{number}. {Str(q, "prompt")}{CiteText(q)}", TextWrapping = TextWrapping.Wrap, FontWeight = FontWeights.SemiBold, IsTextSelectionEnabled = true });
            col.Children.Add(new TextBlock { Text = type switch { "single" => "单选", "multi" => "多选", "fill" => "填空", _ => "简答" }, FontSize = 11, Opacity = 0.6 });
            var options = Arr(q, "options").Select(o => o.GetString() ?? "").ToList();
            RadioButtons? radios = null;
            var checks = new List<CheckBox>();
            TextBox? text = null;
            if (type == "single")
            {
                radios = new RadioButtons();
                foreach (var o in options) radios.Items.Add(o);
                col.Children.Add(radios);
            }
            else if (type == "multi")
            {
                foreach (var o in options)
                {
                    var cb = new CheckBox { Content = o };
                    checks.Add(cb);
                    col.Children.Add(cb);
                }
            }
            else
            {
                text = new TextBox { PlaceholderText = type == "fill" ? "输入答案" : "写下你的回答", AcceptsReturn = type == "short", TextWrapping = TextWrapping.Wrap, MinHeight = type == "short" ? 76 : 0 };
                col.Children.Add(text);
            }
            var result = new StackPanel { Spacing = 4 };
            var submit = new Button { Content = "提交", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
            submit.Click += async (_, _) =>
            {
                JsonElement answer;
                if (type == "single") answer = Api.Args(("a", radios!.SelectedIndex)).GetProperty("a");
                else if (type == "multi") answer = Api.Args(("a", checks.Select((c, i) => (c, i)).Where(t => t.c.IsChecked == true).Select(t => t.i).ToArray())).GetProperty("a");
                else answer = Api.Args(("a", text!.Text)).GetProperty("a");
                if (type == "single" && radios!.SelectedIndex < 0) { _notify(InfoBarSeverity.Warning, "请先选择一个选项", ""); return; }
                var res = await CallGradeAsync("studio.grade_answer", Args(("artifact_id", _artifactId), ("question_id", qid), ("answer", answer)), "评分");
                if (res is not { } g) return;
                result.Children.Clear();
                var isSelfEval = Bool(g, "self_eval");
                var ok = Bool(g, "correct");
                if (isSelfEval)
                {
                    result.Children.Add(new TextBlock { Text = $"评分要点：{Str(g, "rubric")}", TextWrapping = TextWrapping.Wrap });
                    result.Children.Add(new TextBlock { Text = "请对照要点自评。", Opacity = 0.7, FontSize = 12 });
                }
                else
                {
                    var resBrush = new SolidColorBrush(ok ? Windows.UI.Color.FromArgb(255, 120, 220, 140) : Windows.UI.Color.FromArgb(255, 255, 140, 130));
                    var statusRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
                    statusRow.Children.Add(new Viewbox { Width = 14, Height = 14, VerticalAlignment = VerticalAlignment.Center, Child = new PathIcon { Width = 24, Height = 24, Data = Geom(ok ? CheckIconPath : CrossIconPath), Foreground = resBrush } });
                    statusRow.Children.Add(new TextBlock { Text = ok ? "回答正确" : "回答错误", Foreground = resBrush, FontWeight = FontWeights.SemiBold });
                    result.Children.Add(statusRow);
                    var ans = Arr(q, "answer_indices").Select(i => i.GetInt32()).Where(i => i < options.Count).Select(i => $"{(char)('A' + i)}. {options[i]}").ToList();
                    var accepted = Arr(q, "accepted_answers").Select(a => a.GetString() ?? "");
                    var reference = ans.Count > 0 ? string.Join("；", ans) : string.Join(" / ", accepted);
                    if (!ok && reference.Length > 0) result.Children.Add(new TextBlock { Text = $"参考答案：{reference}", TextWrapping = TextWrapping.Wrap });
                    if (answered.Add(qid) && ok) correct++;
                    score.Text = $"已作答 {answered.Count}/{questions.Count}，答对 {correct}";
                }
                if (Str(q, "explanation").Length > 0) result.Children.Add(new TextBlock { Text = $"解析：{Str(q, "explanation")}", TextWrapping = TextWrapping.Wrap, Opacity = 0.9 });
            };
            col.Children.Add(submit);
            col.Children.Add(result);
            card.Child = col;
            panel.Children.Add(card);
        }
        AppendFeedbackIfPresent(panel);
        return WrapScroll(panel);
    }

    // flashcards --------------------------------------------------------------

    private FrameworkElement BuildFlashcards(JsonElement payload)
    {
        var cards = Arr(payload, "cards").ToList();
        if (cards.Count == 0) return Message("没有卡片。");
        var order = Enumerable.Range(0, cards.Count).ToList();
        var index = 0;
        var flipped = false;
        var known = 0;
        var unknown = new List<int>();

        var progress = new TextBlock { HorizontalAlignment = HorizontalAlignment.Center, Opacity = 0.8 };
        var face = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = _compact ? 18 : 20, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center, TextAlignment = TextAlignment.Center, IsTextSelectionEnabled = true };
        var hint = new TextBlock { Opacity = 0.6, HorizontalAlignment = HorizontalAlignment.Center, TextWrapping = TextWrapping.Wrap };
        var card = new Border
        {
            Background = (Brush)Application.Current.Resources[_compact ? "PanelBgBrush" : "ElevatedBgBrush"],
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(14),
            Padding = new Thickness(_compact ? 20 : 28),
            MinHeight = _compact ? 150 : 220,
            Child = face,
        };
        var flip = new Button { Content = "翻转", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        var yes = new Button { Content = "认识", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        var no = new Button { Content = "不认识", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, HorizontalAlignment = HorizontalAlignment.Center };
        row.Children.Add(no);
        row.Children.Add(flip);
        row.Children.Add(yes);
        var root = new StackPanel { Spacing = 12, MaxWidth = 640, HorizontalAlignment = HorizontalAlignment.Center };
        root.Children.Add(progress);
        root.Children.Add(card);
        root.Children.Add(hint);
        root.Children.Add(row);
        AppendFeedbackIfPresent(root);

        void Show()
        {
            if (index >= order.Count)
            {
                face.Text = unknown.Count == 0 ? $"全部完成！认识 {known}/{cards.Count}" : $"本轮结束：认识 {known}，不认识 {unknown.Count}";
                hint.Text = "";
                progress.Text = "";
                flip.Content = unknown.Count > 0 ? "只复习不认识的" : "重新开始";
                yes.Visibility = no.Visibility = Visibility.Collapsed;
                return;
            }
            var c = cards[order[index]];
            progress.Text = $"{index + 1} / {order.Count}";
            face.Text = flipped ? Str(c, "back") : Str(c, "front");
            hint.Text = !flipped && Str(c, "hint").Length > 0 ? $"提示：{Str(c, "hint")}" : "";
            flip.Content = flipped ? "看正面" : "翻转";
            yes.Visibility = no.Visibility = Visibility.Visible;
        }
        void Next(bool knew) { if (knew) known++; else unknown.Add(order[index]); index++; flipped = false; Show(); }
        flip.Click += (_, _) =>
        {
            if (index >= order.Count) { order = unknown.Count > 0 ? unknown.ToList() : Enumerable.Range(0, cards.Count).ToList(); unknown.Clear(); known = 0; index = 0; }
            else flipped = !flipped;
            Show();
        };
        yes.Click += (_, _) => Next(true);
        no.Click += (_, _) => Next(false);
        Show();
        return WrapScroll(root, 460);
    }

    // mind map ----------------------------------------------------------------

    private FrameworkElement BuildMindmap(JsonElement payload) =>
        new MindmapTreeView(payload, _onExportSvg, _compact, _feedbackBar).Root;

    // report ------------------------------------------------------------------

    private FrameworkElement BuildReport(JsonElement payload)
    {
        var tocPanel = new StackPanel { Spacing = 4 };
        if (!_compact)
        {
            tocPanel.Children.Add(new TextBlock
            {
                Text = "目录",
                FontSize = 13,
                FontWeight = FontWeights.SemiBold,
                Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
                Margin = new Thickness(8, 4, 8, 8),
            });
        }

        var contentPanel = new StackPanel { Spacing = 14 };
        var sections = Arr(payload, "sections").ToList();
        if (sections.Count == 0) return Message("没有报告内容。");
        var compactTocHost = new ContentControl { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        if (_compact && sections.Count > 1) contentPanel.Children.Add(compactTocHost);

        var sectionIndex = 0;
        foreach (var s in sections)
        {
            sectionIndex++;
            var headingText = Str(s, "heading");
            var headingBlock = new TextBlock
            {
                Text = headingText,
                FontSize = _compact ? 17 : 19,
                FontWeight = FontWeights.SemiBold,
                Margin = new Thickness(0, 6, 0, 0),
                TextWrapping = TextWrapping.Wrap,
                IsTextSelectionEnabled = true,
            };
            contentPanel.Children.Add(headingBlock);
            contentPanel.Children.Add(Md.Render(Str(s, "body_md"), n => Md.SuperscriptCitation(n), 14, _math, _mathHost));
            var takeaways = Arr(s, "key_takeaways").Select(t => t.GetString() ?? "").Where(t => t.Length > 0).ToList();
            if (takeaways.Count > 0)
            {
                var box = new Border
                {
                    Background = (Brush)Application.Current.Resources[_compact ? "PanelBgBrush" : "ElevatedBgBrush"],
                    BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
                    BorderThickness = new Thickness(1),
                    CornerRadius = new CornerRadius(10),
                    Padding = new Thickness(12),
                };
                var col = new StackPanel { Spacing = 4 };
                col.Children.Add(new TextBlock { Text = "核心要点", FontWeight = FontWeights.SemiBold, FontSize = 13 });
                foreach (var t in takeaways) col.Children.Add(new TextBlock { Text = $"- {t}", TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true, FontSize = 13 });
                box.Child = col;
                contentPanel.Children.Add(box);
            }

            var target = headingBlock;
            var tocBtn = new Button
            {
                Content = new TextBlock
                {
                    Text = $"{sectionIndex}. {headingText}",
                    FontSize = 12,
                    TextWrapping = TextWrapping.Wrap,
                    Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
                },
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Left,
                Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent),
                BorderThickness = new Thickness(0),
                Padding = new Thickness(8, 6, 8, 6),
                CornerRadius = new CornerRadius(8),
            };
            tocBtn.Click += (_, _) => target.StartBringIntoView();
            tocPanel.Children.Add(tocBtn);
        }
        AppendFeedbackIfPresent(contentPanel);

        if (_compact)
        {
            if (sections.Count > 1)
            {
                compactTocHost.Content = new Expander
                {
                    Header = $"目录（共 {sections.Count} 节）",
                    Content = tocPanel,
                    HorizontalAlignment = HorizontalAlignment.Stretch,
                };
            }
            return WrapScroll(contentPanel);
        }

        var split = new Grid { ColumnSpacing = 14 };
        split.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(200) });
        split.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });

        var tocBorder = new Border
        {
            Background = (Brush)Application.Current.Resources["ElevatedBgBrush"],
            CornerRadius = new CornerRadius(12),
            Padding = new Thickness(6),
            Child = new ScrollViewer { Content = tocPanel },
        };
        Grid.SetColumn(tocBorder, 0);
        split.Children.Add(tocBorder);

        var contentScroll = new ScrollViewer { Content = contentPanel, Padding = new Thickness(0, 0, 8, 0) };
        Grid.SetColumn(contentScroll, 1);
        split.Children.Add(contentScroll);
        return split;
    }

    // slides ------------------------------------------------------------------

    private FrameworkElement BuildSlides(JsonElement payload)
    {
        var slides = Arr(payload, "slides").ToList();
        if (slides.Count == 0) return Message("没有幻灯片。");
        var index = 0;
        var title = new TextBlock { FontSize = _compact ? 22 : 26, FontWeight = FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap };
        var bullets = new StackPanel { Spacing = 8 };
        var notes = new TextBlock { TextWrapping = TextWrapping.Wrap, Opacity = 0.8, IsTextSelectionEnabled = true };
        var counter = new TextBlock { VerticalAlignment = VerticalAlignment.Center };
        var slide = new Border
        {
            Background = (Brush)Application.Current.Resources[_compact ? "PanelBgBrush" : "ElevatedBgBrush"],
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(14),
            Padding = new Thickness(_compact ? 20 : 28),
            MinHeight = _compact ? 200 : 280,
        };
        var inner = new StackPanel { Spacing = 12 };
        inner.Children.Add(title);
        inner.Children.Add(bullets);
        slide.Child = inner;

        var prevSp = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        prevSp.Children.Add(new Viewbox { Width = 14, Height = 14, VerticalAlignment = VerticalAlignment.Center, Child = new PathIcon { Width = 24, Height = 24, Data = Geom(PrevIconPath) } });
        prevSp.Children.Add(new TextBlock { Text = "上一页" });
        var prev = new Button { Content = prevSp, Style = (Style)Application.Current.Resources["PillButtonStyle"] };

        var nextSp = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        nextSp.Children.Add(new TextBlock { Text = "下一页" });
        nextSp.Children.Add(new Viewbox { Width = 14, Height = 14, VerticalAlignment = VerticalAlignment.Center, Child = new PathIcon { Width = 24, Height = 24, Data = Geom(NextIconPath) } });
        var next = new Button { Content = nextSp, Style = (Style)Application.Current.Resources["PillButtonStyle"] };

        void Show()
        {
            var s = slides[index];
            var layout = Str(s, "layout");
            title.Text = Str(s, "title");
            title.FontSize = layout is "title" or "section" ? (_compact ? 24 : 32) : (_compact ? 20 : 24);
            bullets.Children.Clear();
            foreach (var b in Arr(s, "bullets"))
            {
                var isQuote = layout == "quote";
                bullets.Children.Add(new TextBlock { Text = isQuote ? $"“{b.GetString()}”" : $"- {b.GetString()}", TextWrapping = TextWrapping.Wrap, FontSize = _compact ? 14 : 16, IsTextSelectionEnabled = true, FontStyle = isQuote ? Windows.UI.Text.FontStyle.Italic : Windows.UI.Text.FontStyle.Normal });
            }
            notes.Text = Str(s, "speaker_notes").Length > 0 ? Str(s, "speaker_notes") : "（没有讲稿）";
            counter.Text = $"{index + 1} / {slides.Count}";
            prev.IsEnabled = index > 0;
            next.IsEnabled = index < slides.Count - 1;
        }
        prev.Click += (_, _) => { index--; Show(); };
        next.Click += (_, _) => { index++; Show(); };
        Show();
        var nav = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12, HorizontalAlignment = HorizontalAlignment.Center };
        nav.Children.Add(prev);
        nav.Children.Add(counter);
        nav.Children.Add(next);
        var root = new StackPanel { Spacing = 12 };
        root.Children.Add(slide);
        root.Children.Add(nav);
        root.Children.Add(new Expander { Header = "讲稿", Content = notes, HorizontalAlignment = HorizontalAlignment.Stretch });
        AppendFeedbackIfPresent(root);
        return WrapScroll(root);
    }

    // infographic -------------------------------------------------------------

    private FrameworkElement BuildInfographic(JsonElement payload)
    {
        var scale = new ScaleTransform { ScaleX = 1.0, ScaleY = 1.0 };
        var canvasHost = new Grid { RenderTransform = scale, RenderTransformOrigin = new Windows.Foundation.Point(0, 0), Padding = new Thickness(8) };

        var dir = Str(_art, "output_dir");
        var svgPath = dir.Length > 0 ? Path.Combine(dir, "infographic.svg") : "";
        if (svgPath.Length > 0 && File.Exists(svgPath))
        {
            var img = new Image
            {
                Source = new SvgImageSource(new Uri(svgPath)),
                Stretch = Stretch.Uniform,
                HorizontalAlignment = HorizontalAlignment.Center,
            };
            canvasHost.Children.Add(new Border
            {
                CornerRadius = new CornerRadius(14),
                Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 245, 248, 255)),
                Padding = new Thickness(8),
                Child = img,
            });
        }
        else
        {
            var cardGrid = new StackPanel { Spacing = 12, Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 245, 248, 255)), Padding = new Thickness(20), CornerRadius = new CornerRadius(14) };
            cardGrid.Children.Add(new TextBlock { Text = Str(payload, "title"), FontSize = 24, FontWeight = FontWeights.Bold, Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 23, 58, 122)), TextWrapping = TextWrapping.Wrap });
            cardGrid.Children.Add(new TextBlock { Text = Str(payload, "subtitle"), FontSize = 14, Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 75, 85, 99)), TextWrapping = TextWrapping.Wrap });
            foreach (var b in Arr(payload, "blocks"))
            {
                var c = new StackPanel { Spacing = 4 };
                c.Children.Add(new TextBlock { Text = Str(b, "heading"), FontWeight = FontWeights.SemiBold, Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 23, 58, 122)) });
                c.Children.Add(new TextBlock { Text = Str(b, "metric"), FontSize = 26, FontWeight = FontWeights.Bold, Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 26, 115, 232)) });
                c.Children.Add(new TextBlock { Text = $"{Str(b, "summary")}{CiteText(b)}", TextWrapping = TextWrapping.Wrap, Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 60, 64, 67)) });
                cardGrid.Children.Add(new Border { Background = new SolidColorBrush(Microsoft.UI.Colors.White), CornerRadius = new CornerRadius(12), Padding = new Thickness(14), Child = c });
            }
            canvasHost.Children.Add(cardGrid);
        }

        var scroll = new ScrollViewer
        {
            Content = canvasHost,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
        };

        Button MakeZoomBtn(string path, string tip, RoutedEventHandler onClick)
        {
            var b = new Button
            {
                Content = new Viewbox
                {
                    Width = 16,
                    Height = 16,
                    HorizontalAlignment = HorizontalAlignment.Center,
                    VerticalAlignment = VerticalAlignment.Center,
                    Child = new PathIcon
                    {
                        Width = 24,
                        Height = 24,
                        Data = Geom(path),
                        Foreground = (Brush)Application.Current.Resources["TextPrimaryBrush"],
                    },
                },
                MinWidth = 34,
                Width = 34,
                MinHeight = 34,
                Height = 34,
                Padding = new Thickness(0),
                CornerRadius = new CornerRadius(17),
                HorizontalContentAlignment = HorizontalAlignment.Center,
                VerticalContentAlignment = VerticalAlignment.Center,
                Background = (Brush)Application.Current.Resources["ElevatedBgBrush"],
                BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
                BorderThickness = new Thickness(1),
            };
            ToolTipService.SetToolTip(b, tip);
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(b, tip);
            b.Click += onClick;
            return b;
        }
        var zoomIn = MakeZoomBtn(PlusIconPath, "放大", (_, _) => { scale.ScaleX = scale.ScaleY = Math.Min(2.4, scale.ScaleX + 0.15); });
        var zoomOut = MakeZoomBtn(MinusIconPath, "缩小", (_, _) => { scale.ScaleX = scale.ScaleY = Math.Max(0.5, scale.ScaleX - 0.15); });
        var zoomBox = new Border
        {
            Background = (Brush)Application.Current.Resources["ElevatedBgBrush"],
            BorderBrush = (Brush)Application.Current.Resources["StrongBorderBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(18),
            Padding = new Thickness(4),
            HorizontalAlignment = HorizontalAlignment.Right,
            VerticalAlignment = VerticalAlignment.Bottom,
            Margin = new Thickness(0, 0, 14, 14),
            Child = new StackPanel { Spacing = 4, Children = { zoomIn, zoomOut } },
        };

        var stage = new Grid();
        stage.Children.Add(scroll);
        stage.Children.Add(zoomBox);

        var outer = new Grid { RowSpacing = 8 };
        if (_compact) outer.Height = 420;
        outer.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        outer.Children.Add(stage);
        if (_feedbackBar != null)
        {
            outer.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            Grid.SetRow(_feedbackBar, 1);
            outer.Children.Add(_feedbackBar);
        }
        return outer;
    }

    // audio -------------------------------------------------------------------

    private FrameworkElement BuildAudio(JsonElement payload)
    {
        var root = new StackPanel { Spacing = 12 };
        var dir = Str(_art, "output_dir");
        var wav = dir.Length > 0 ? Path.Combine(dir, "overview.wav") : "";
        var turns = Arr(payload, "turns").ToList();
        var lines = new List<TextBlock>();
        if (wav.Length > 0 && File.Exists(wav))
        {
            DisposeAudio();
            _mediaSource = Windows.Media.Core.MediaSource.CreateFromUri(new Uri(wav));
            _player = new Windows.Media.Playback.MediaPlayer { Source = _mediaSource };
            _mediaElement = new MediaPlayerElement { AreTransportControlsEnabled = true, Height = 90, HorizontalAlignment = HorizontalAlignment.Stretch };
            _mediaElement.SetMediaPlayer(_player);
            root.Children.Add(_mediaElement);
            var total = Math.Max(1, turns.Sum(t => Str(t, "text").Length));
            _audioTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(400) };
            _audioTimer.Tick += (_, _) =>
            {
                if (_disposed || _player == null) return;
                Windows.Media.Playback.MediaPlaybackSession? session;
                try { session = _player.PlaybackSession; }
                catch { return; }
                if (session == null || session.NaturalDuration.TotalSeconds <= 0) return;
                var spoken = session.Position.TotalSeconds / session.NaturalDuration.TotalSeconds * total;
                var acc = 0.0;
                for (var i = 0; i < turns.Count; i++)
                {
                    acc += Str(turns[i], "text").Length;
                    lines[i].Opacity = spoken <= acc && spoken > acc - Str(turns[i], "text").Length ? 1.0 : 0.45;
                }
            };
            _audioTimer.Start();
        }
        else
        {
            root.Children.Add(new TextBlock { Text = "没有找到音频文件，只显示文字稿。", Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 200, 120)) });
        }
        foreach (var t in turns)
        {
            var tb = new TextBlock { Text = $"{Str(t, "speaker")}：{Str(t, "text")}", TextWrapping = TextWrapping.Wrap, FontSize = 14, IsTextSelectionEnabled = true, Opacity = 0.9 };
            lines.Add(tb);
            root.Children.Add(tb);
        }
        AppendFeedbackIfPresent(root);
        return WrapScroll(root, 480);
    }
}
