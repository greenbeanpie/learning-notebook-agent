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
public sealed class ArtifactViewerPane : Grid, IDisposable
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

    private readonly string _id;
    private readonly Window _hostWindow;
    private readonly Action _onClose;
    private readonly Action _onToggleFullscreen;
    private readonly TextBlock _title = new() { FontSize = 17, FontWeight = FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center };
    private readonly TextBlock _sourceBadge = new() { FontSize = 11, Foreground = (Brush)Application.Current.Resources["TextMutedBrush"], VerticalAlignment = VerticalAlignment.Center };
    private readonly StackPanel _tools = new() { Orientation = Orientation.Horizontal, Spacing = 6, HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Center };
    private readonly PathIcon _fullscreenIcon = new() { Data = Geom(FullscreenIconPath) };
    private readonly ContentControl _body = new() { HorizontalContentAlignment = HorizontalAlignment.Stretch, VerticalContentAlignment = VerticalAlignment.Stretch };
    private readonly InfoBar _bar = new() { IsOpen = false, IsClosable = true };
    private JsonElement _art;
    private string _builtFor = "";
    private Windows.Media.Playback.MediaPlayer? _player;
    private DispatcherTimer? _audioTimer;

    private static Geometry Geom(string path) =>
        (Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Geometry), path);

    public ArtifactViewerPane(string id, Window hostWindow, Action onClose, Action onToggleFullscreen)
    {
        _id = id;
        _hostWindow = hostWindow;
        _onClose = onClose;
        _onToggleFullscreen = onToggleFullscreen;

        RowSpacing = 10;
        RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });

        var head = new Grid { ColumnSpacing = 10 };
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        var titleCol = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        titleCol.Children.Add(_title);
        titleCol.Children.Add(_sourceBadge);
        head.Children.Add(titleCol);

        Grid.SetColumn(_tools, 1);
        head.Children.Add(_tools);
        Children.Add(head);

        Grid.SetRow(_body, 1);
        Children.Add(_body);

        Grid.SetRow(_bar, 2);
        Children.Add(_bar);

        App.Core.EventReceived += OnEvent;
        _ = LoadAsync();
    }

    public void SetFullscreenIcon(bool isFullscreen)
    {
        _fullscreenIcon.Data = Geom(isFullscreen ? ExitFullscreenIconPath : FullscreenIconPath);
    }

    public void Dispose()
    {
        App.Core.EventReceived -= OnEvent;
        _audioTimer?.Stop();
        _player?.Dispose();
        _player = null;
    }

    private void OnEvent(Interop.ApiEventDto ev)
    {
        if (ev.Event != "studio.status" || Str(ev.Data, "artifact_id") != _id) return;
        DispatcherQueue.TryEnqueue(async () => await LoadAsync());
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
            Say(InfoBarSeverity.Error, $"{what}失败", Advice(e.Code) is { Length: > 0 } a ? $"{a}（{e.Code}）" : $"{e.Message}（{e.Code}）");
            return null;
        }
        return res;
    }

    private async Task LoadAsync()
    {
        var res = await Core("studio.get", Args(("artifact_id", _id)), "读取产物");
        if (res is not { } a) return;
        _art = a;
        _title.Text = Str(a, "title");
        var srcCount = Arr(a, "source_ids").Count();
        _sourceBadge.Text = srcCount > 0 ? $"基于 {srcCount} 个来源" : "";

        var status = Str(a, "status");
        var kind = Str(a, "artifact_type");
        var key = $"{status}:{Str(a, "updated_at")}:{Int(a, "updated_at")}";
        if (key == _builtFor) return;
        _builtFor = key;
        BuildTools(kind, status);
        _body.Content = status switch
        {
            "READY" => BuildViewer(kind, Obj(a, "payload")),
            "DRAFT_REVIEW" => BuildDraftReview(a),
            "FAILED" => Message($"生成失败：{(Advice(Str(a, "error_code")) is { Length: > 0 } adv ? adv : Str(a, "error_detail"))}"),
            "CANCELLED" => Message("已取消。"),
            _ => Message("正在生成，请稍候…完成后这里会自动显示。"),
        };
    }

    private static FrameworkElement Message(string text) => new TextBlock { Text = text, TextWrapping = TextWrapping.Wrap, Margin = new Thickness(0, 20, 0, 0) };

    private static Button IconBtn(string path, string tip, RoutedEventHandler onClick, PathIcon? iconOverride = null)
    {
        var icon = iconOverride ?? new PathIcon { Data = Geom(path) };
        icon.Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"];
        var btn = new Button
        {
            Content = new Viewbox { Width = 16, Height = 16, Child = icon },
            Width = 34,
            Height = 34,
            Padding = new Thickness(0),
            CornerRadius = new CornerRadius(17),
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

        _tools.Children.Add(IconBtn(FullscreenIconPath, "全屏显示 / 还原", (_, _) => _onToggleFullscreen(), _fullscreenIcon));
        _tools.Children.Add(IconBtn(CloseIconPath, "关闭详情", (_, _) => _onClose()));
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
            sp.Children.Add(new Viewbox { Width = 14, Height = 14, Child = new PathIcon { Data = Geom(path), Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"] } });
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

    // ---------- viewers ----------

    private FrameworkElement BuildViewer(string kind, JsonElement payload) => kind switch
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

    private static string CiteText(JsonElement item)
    {
        var nums = Arr(item, "citations").Where(c => c.ValueKind == JsonValueKind.Number).Select(c => $"[{c.GetInt32()}]").ToArray();
        return nums.Length > 0 ? " " + string.Join("", nums) : "";
    }

    // quiz ------------------------------------------------------------------

    private FrameworkElement BuildQuiz(JsonElement payload)
    {
        var panel = new StackPanel { Spacing = 14 };
        var questions = Arr(payload, "questions").ToList();
        var score = new TextBlock { FontSize = 14, FontWeight = FontWeights.SemiBold, Text = $"共 {questions.Count} 题，作答后点击“提交”查看结果。" };
        panel.Children.Add(score);
        var correct = 0;
        var answered = new HashSet<string>();
        var number = 0;
        foreach (var q in questions)
        {
            number++;
            var qid = Str(q, "id");
            var type = Str(q, "type");
            var card = new Border { Background = (Brush)Application.Current.Resources["ElevatedBgBrush"], CornerRadius = new CornerRadius(12), Padding = new Thickness(14) };
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
                text = new TextBox { PlaceholderText = type == "fill" ? "输入答案" : "写下你的回答", AcceptsReturn = type == "short", TextWrapping = TextWrapping.Wrap, MinHeight = type == "short" ? 90 : 0 };
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
                if (type == "single" && radios!.SelectedIndex < 0) { Say(InfoBarSeverity.Warning, "请先选择一个选项", ""); return; }
                var res = await Core("studio.grade_answer", Args(("artifact_id", _id), ("question_id", qid), ("answer", answer)), "评分");
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
                    statusRow.Children.Add(new Viewbox { Width = 14, Height = 14, Child = new PathIcon { Data = Geom(ok ? CheckIconPath : CrossIconPath), Foreground = resBrush } });
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
        panel.Children.Add(BuildFeedbackBar());
        return new ScrollViewer { Content = panel };
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
        var face = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 20, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center, TextAlignment = TextAlignment.Center, IsTextSelectionEnabled = true };
        var hint = new TextBlock { Opacity = 0.6, HorizontalAlignment = HorizontalAlignment.Center, TextWrapping = TextWrapping.Wrap };
        var card = new Border { Background = (Brush)Application.Current.Resources["ElevatedBgBrush"], CornerRadius = new CornerRadius(16), Padding = new Thickness(28), MinHeight = 220, Child = face };
        var flip = new Button { Content = "翻转", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        var yes = new Button { Content = "认识", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        var no = new Button { Content = "不认识", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, HorizontalAlignment = HorizontalAlignment.Center };
        row.Children.Add(no);
        row.Children.Add(flip);
        row.Children.Add(yes);
        var root = new StackPanel { Spacing = 14, MaxWidth = 640, HorizontalAlignment = HorizontalAlignment.Center };
        root.Children.Add(progress);
        root.Children.Add(card);
        root.Children.Add(hint);
        root.Children.Add(row);
        root.Children.Add(BuildFeedbackBar());

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
        return new ScrollViewer { Content = root };
    }

    // mind map ----------------------------------------------------------------

    private FrameworkElement BuildMindmap(JsonElement payload)
    {
        var nodes = Arr(payload, "nodes").ToList();
        var byParent = nodes.GroupBy(n => n.TryGetProperty("parent_id", out var p) && p.ValueKind == JsonValueKind.String ? p.GetString()! : "").ToDictionary(g => g.Key, g => g.ToList());
        var tree = new TreeView { SelectionMode = TreeViewSelectionMode.None };
        void Fill(string parentKey, TreeViewNode? parentNode)
        {
            if (!byParent.TryGetValue(parentKey, out var children)) return;
            foreach (var n in children)
            {
                var node = new TreeViewNode { Content = $"{Str(n, "label")}{CiteText(n)}", IsExpanded = true };
                if (parentNode == null) tree.RootNodes.Add(node); else parentNode.Children.Add(node);
                Fill(Str(n, "id"), node);
            }
        }
        Fill("", null);
        var scale = new ScaleTransform { ScaleX = 1, ScaleY = 1 };
        var host = new Grid { RenderTransform = scale, RenderTransformOrigin = new Windows.Foundation.Point(0, 0) };
        host.Children.Add(tree);
        var zoom = new Slider { Minimum = 0.6, Maximum = 2.0, Value = 1, StepFrequency = 0.1, Width = 140, Header = "缩放" };
        zoom.ValueChanged += (_, e) => { scale.ScaleX = scale.ScaleY = e.NewValue; };
        void SetAll(bool expand) { foreach (var r in tree.RootNodes) Expand(r, expand); }
        var expandAll = new Button { Content = "全部展开", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        expandAll.Click += (_, _) => SetAll(true);
        var collapseAll = new Button { Content = "全部折叠", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        collapseAll.Click += (_, _) => SetAll(false);
        var bar = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        bar.Children.Add(expandAll);
        bar.Children.Add(collapseAll);
        bar.Children.Add(zoom);
        var grid = new Grid { RowSpacing = 8 };
        grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        grid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        grid.Children.Add(bar);
        var scroll = new ScrollViewer { Content = host, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto, VerticalScrollBarVisibility = ScrollBarVisibility.Auto };
        Grid.SetRow(scroll, 1);
        grid.Children.Add(scroll);
        return grid;
    }

    private static void Expand(TreeViewNode n, bool expand)
    {
        n.IsExpanded = expand;
        foreach (var c in n.Children) Expand(c, expand);
    }

    // report (Requirement 4: TOC sidebar + rendered content) ------------------

    private FrameworkElement BuildReport(JsonElement payload)
    {
        var split = new Grid { ColumnSpacing = 14 };
        split.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(200) });
        split.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });

        var tocPanel = new StackPanel { Spacing = 4 };
        tocPanel.Children.Add(new TextBlock
        {
            Text = "目录",
            FontSize = 13,
            FontWeight = FontWeights.SemiBold,
            Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
            Margin = new Thickness(8, 4, 8, 8),
        });

        var contentPanel = new StackPanel { Spacing = 14 };
        var sections = Arr(payload, "sections").ToList();
        var sectionIndex = 0;
        foreach (var s in sections)
        {
            sectionIndex++;
            var headingText = Str(s, "heading");
            var headingBlock = new TextBlock
            {
                Text = headingText,
                FontSize = 19,
                FontWeight = FontWeights.SemiBold,
                Margin = new Thickness(0, 8, 0, 0),
                TextWrapping = TextWrapping.Wrap,
                IsTextSelectionEnabled = true,
            };
            contentPanel.Children.Add(headingBlock);
            contentPanel.Children.Add(Md.Render(Str(s, "body_md"), n => new Microsoft.UI.Xaml.Documents.Run { Text = $"[{n}]", FontSize = 11 }, 14));
            var takeaways = Arr(s, "key_takeaways").Select(t => t.GetString() ?? "").Where(t => t.Length > 0).ToList();
            if (takeaways.Count > 0)
            {
                var box = new Border { Background = (Brush)Application.Current.Resources["ElevatedBgBrush"], CornerRadius = new CornerRadius(10), Padding = new Thickness(12) };
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
        contentPanel.Children.Add(BuildFeedbackBar());

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
        var title = new TextBlock { FontSize = 26, FontWeight = FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap };
        var bullets = new StackPanel { Spacing = 8 };
        var notes = new TextBlock { TextWrapping = TextWrapping.Wrap, Opacity = 0.8, IsTextSelectionEnabled = true };
        var counter = new TextBlock { VerticalAlignment = VerticalAlignment.Center };
        var slide = new Border { Background = (Brush)Application.Current.Resources["ElevatedBgBrush"], CornerRadius = new CornerRadius(16), Padding = new Thickness(28), MinHeight = 280 };
        var inner = new StackPanel { Spacing = 14 };
        inner.Children.Add(title);
        inner.Children.Add(bullets);
        slide.Child = inner;

        var prevSp = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        prevSp.Children.Add(new Viewbox { Width = 14, Height = 14, Child = new PathIcon { Data = Geom(PrevIconPath) } });
        prevSp.Children.Add(new TextBlock { Text = "上一页" });
        var prev = new Button { Content = prevSp, Style = (Style)Application.Current.Resources["PillButtonStyle"] };

        var nextSp = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        nextSp.Children.Add(new TextBlock { Text = "下一页" });
        nextSp.Children.Add(new Viewbox { Width = 14, Height = 14, Child = new PathIcon { Data = Geom(NextIconPath) } });
        var next = new Button { Content = nextSp, Style = (Style)Application.Current.Resources["PillButtonStyle"] };

        void Show()
        {
            var s = slides[index];
            var layout = Str(s, "layout");
            title.Text = Str(s, "title");
            title.FontSize = layout is "title" or "section" ? 32 : 24;
            bullets.Children.Clear();
            foreach (var b in Arr(s, "bullets"))
            {
                var isQuote = layout == "quote";
                bullets.Children.Add(new TextBlock { Text = isQuote ? $"“{b.GetString()}”" : $"- {b.GetString()}", TextWrapping = TextWrapping.Wrap, FontSize = 16, IsTextSelectionEnabled = true, FontStyle = isQuote ? Windows.UI.Text.FontStyle.Italic : Windows.UI.Text.FontStyle.Normal });
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
        root.Children.Add(BuildFeedbackBar());
        return new ScrollViewer { Content = root };
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

    // infographic (Requirement 4: image_c93c68 zoomable canvas + feedback) ----

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

        // Floating +/- zoom buttons (bottom-right, matching image_c93c68.jpg)
        var zoomIn = new Button { Content = "+", Width = 34, Height = 34, Padding = new Thickness(0), CornerRadius = new CornerRadius(17), Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        var zoomOut = new Button { Content = "-", Width = 34, Height = 34, Padding = new Thickness(0), CornerRadius = new CornerRadius(17), Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        zoomIn.Click += (_, _) => { scale.ScaleX = scale.ScaleY = Math.Min(2.4, scale.ScaleX + 0.15); };
        zoomOut.Click += (_, _) => { scale.ScaleX = scale.ScaleY = Math.Max(0.5, scale.ScaleX - 0.15); };
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
        outer.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        outer.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        outer.Children.Add(stage);
        var fb = BuildFeedbackBar();
        Grid.SetRow(fb, 1);
        outer.Children.Add(fb);
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
            _player?.Dispose();
            _player = new Windows.Media.Playback.MediaPlayer { Source = Windows.Media.Core.MediaSource.CreateFromUri(new Uri(wav)) };
            var element = new MediaPlayerElement { AreTransportControlsEnabled = true, Height = 90, HorizontalAlignment = HorizontalAlignment.Stretch };
            element.SetMediaPlayer(_player);
            root.Children.Add(element);
            var total = Math.Max(1, turns.Sum(t => Str(t, "text").Length));
            _audioTimer?.Stop();
            _audioTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(400) };
            _audioTimer.Tick += (_, _) =>
            {
                var session = _player?.PlaybackSession;
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
        root.Children.Add(BuildFeedbackBar());
        return new ScrollViewer { Content = root };
    }
}
