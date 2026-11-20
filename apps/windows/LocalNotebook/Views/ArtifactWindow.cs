using LocalNotebook.Services;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using System.Text.Json;
using Windows.Storage.Pickers;
using WinRT.Interop;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>Viewer for one Studio artifact: quiz, flashcards, mind map, report, slides, infographic or audio.</summary>
public sealed class ArtifactWindow : Window
{
    private readonly string _id;
    private readonly Grid _root = new() { Background = (Brush)Application.Current.Resources["CanvasBgBrush"], Padding = new Thickness(16), RowSpacing = 10 };
    private readonly TextBlock _title = new() { FontSize = 20, FontWeight = FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap };
    private readonly StackPanel _tools = new() { Orientation = Orientation.Horizontal, Spacing = 8, HorizontalAlignment = HorizontalAlignment.Right };
    private readonly ContentControl _body = new() { HorizontalContentAlignment = HorizontalAlignment.Stretch, VerticalContentAlignment = VerticalAlignment.Stretch };
    private readonly InfoBar _bar = new() { IsOpen = false, IsClosable = true };
    private JsonElement _art;
    private string _builtFor = "";
    private Windows.Media.Playback.MediaPlayer? _player;

    public ArtifactWindow(string id)
    {
        _id = id;
        Title = "Studio 产物";
        _root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        _root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        _root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        var head = new Grid { ColumnSpacing = 12 };
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        head.Children.Add(_title);
        Grid.SetColumn(_tools, 1);
        head.Children.Add(_tools);
        _root.Children.Add(head);
        Grid.SetRow(_body, 1);
        _root.Children.Add(_body);
        Grid.SetRow(_bar, 2);
        _root.Children.Add(_bar);
        Content = _root;
        App.Core.EventReceived += OnEvent;
        Closed += (_, _) =>
        {
            App.Core.EventReceived -= OnEvent;
            _player?.Dispose();
        };
        _ = LoadAsync();
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
        Title = $"{Str(a, "title")} — Studio";
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

    // ---------- toolbar / export ----------

    private void BuildTools(string kind, string status)
    {
        _tools.Children.Clear();
        if (status != "READY") return;
        var formats = kind switch
        {
            "quiz" => new[] { ("Markdown", "markdown", ".md"), ("JSON", "json", ".json") },
            "flashcards" => new[] { ("CSV", "csv", ".csv"), ("Markdown", "markdown", ".md") },
            "mindmap" => new[] { ("Markdown", "markdown", ".md"), ("JSON", "json", ".json") },
            "report" => new[] { ("Markdown", "markdown", ".md"), ("JSON", "json", ".json") },
            "slides" => new[] { ("PPTX", "pptx", ".pptx"), ("Markdown", "markdown", ".md") },
            "infographic" => new[] { ("SVG", "svg", ".svg"), ("Markdown", "markdown", ".md") },
            "audio" => new[] { ("WAV", "wav", ".wav"), ("文字稿", "markdown", ".md") },
            _ => new[] { ("Markdown", "markdown", ".md") },
        };
        var menu = new MenuFlyout();
        foreach (var (label, fmt, ext) in formats)
        {
            var item = new MenuFlyoutItem { Text = $"导出为 {label}" };
            item.Click += async (_, _) => await ExportAsync(fmt, ext, label);
            menu.Items.Add(item);
        }
        _tools.Children.Add(new DropDownButton { Content = "导出", Flyout = menu });
        if (kind == "report")
        {
            var nl = Environment.NewLine;
            var text = string.Join(nl + nl, Arr(Obj(_art, "payload"), "sections").Select(s => $"{Str(s, "heading")}{nl}{Str(s, "body_md")}"));
            if (Md.HasMath(text))
            {
                var math = new Button { Content = "公式视图" };
                math.Click += async (_, _) => await ShowMathAsync(text);
                _tools.Children.Add(math);
            }
        }
        var up = new Button { Content = "👍" };
        up.Click += async (_, _) => { await Core("studio.set_feedback", Args(("artifact_id", _id), ("feedback", 1)), "提交反馈"); Say(InfoBarSeverity.Success, "谢谢反馈", ""); };
        var down = new Button { Content = "👎" };
        down.Click += async (_, _) => { await Core("studio.set_feedback", Args(("artifact_id", _id), ("feedback", -1)), "提交反馈"); Say(InfoBarSeverity.Success, "谢谢反馈", ""); };
        _tools.Children.Add(up);
        _tools.Children.Add(down);
    }

    private readonly WebContentHost _math = new();

    private async Task ShowMathAsync(string text)
    {
        var host = new Grid { MinWidth = 720, Height = 520 };
        var dlg = new ContentDialog { Title = "公式视图", Content = host, CloseButtonText = "关闭", XamlRoot = _root.XamlRoot };
        dlg.Opened += async (_, _) =>
        {
            try { await _math.RenderMathAsync(host, text); }
            catch (Exception ex) { Say(InfoBarSeverity.Error, "公式视图不可用", ex.Message); }
        };
        await dlg.ShowAsync();
        _math.Release();
    }

    private async Task ExportAsync(string fmt, string ext, string label)
    {
        var picker = new FileSavePicker { SuggestedFileName = Str(_art, "title") };
        picker.FileTypeChoices.Add(label, new List<string> { ext });
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        var file = await picker.PickSaveFileAsync();
        if (file == null) return;
        if (await Core("studio.export", Args(("artifact_id", _id), ("format", fmt), ("out_path", file.Path)), "导出") != null)
            Say(InfoBarSeverity.Success, "已导出", file.Path);
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
        var score = new TextBlock { FontSize = 15, FontWeight = FontWeights.SemiBold, Text = $"共 {questions.Count} 题，作答后点击“提交”查看结果。" };
        panel.Children.Add(score);
        var correct = 0;
        var answered = new HashSet<string>();
        var number = 0;
        foreach (var q in questions)
        {
            number++;
            var qid = Str(q, "id");
            var type = Str(q, "type");
            var card = new Border { Background = (Brush)Application.Current.Resources["PanelBgBrush"], CornerRadius = new CornerRadius(12), Padding = new Thickness(14) };
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
            var submit = new Button { Content = "提交" };
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
                    result.Children.Add(new TextBlock { Text = ok ? "✔ 回答正确" : "✖ 回答错误", Foreground = new SolidColorBrush(ok ? Windows.UI.Color.FromArgb(255, 120, 220, 140) : Windows.UI.Color.FromArgb(255, 255, 140, 130)), FontWeight = FontWeights.SemiBold });
                    var ans = Arr(q, "answer_indices").Select(i => i.GetInt32()).Where(i => i < options.Count).Select(i => $"{(char)('A' + i)}. {options[i]}").ToList();
                    var accepted = Arr(q, "accepted_answers").Select(a => a.GetString() ?? "");
                    var reference = ans.Count > 0 ? string.Join("；", ans) : string.Join(" / ", accepted);
                    if (!ok && reference.Length > 0) result.Children.Add(new TextBlock { Text = $"参考答案：{reference}", TextWrapping = TextWrapping.Wrap });
                    if (answered.Add(qid) && ok) correct++;
                    score.Text = $"已作答 {answered.Count}/{questions.Count}，答对 {correct}";
                }
                if (Str(q, "explanation").Length > 0) result.Children.Add(new TextBlock { Text = $"解析：{Str(q, "explanation")}", TextWrapping = TextWrapping.Wrap, Opacity = 0.9 });
                if (isSelfEval && Str(q, "explanation").Length == 0 && Str(q, "rubric").Length == 0) result.Children.Add(new TextBlock { Text = "没有参考答案。" });
            };
            col.Children.Add(submit);
            col.Children.Add(result);
            card.Child = col;
            panel.Children.Add(card);
        }
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
        var face = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 22, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center, TextAlignment = TextAlignment.Center, IsTextSelectionEnabled = true };
        var hint = new TextBlock { Opacity = 0.6, HorizontalAlignment = HorizontalAlignment.Center, TextWrapping = TextWrapping.Wrap };
        var card = new Border { Background = (Brush)Application.Current.Resources["ElevatedBgBrush"], CornerRadius = new CornerRadius(16), Padding = new Thickness(30), MinHeight = 240, Child = face };
        var flip = new Button { Content = "翻转", HorizontalAlignment = HorizontalAlignment.Center };
        var yes = new Button { Content = "认识 ✔" };
        var no = new Button { Content = "不认识 ✖" };
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, HorizontalAlignment = HorizontalAlignment.Center };
        row.Children.Add(no);
        row.Children.Add(flip);
        row.Children.Add(yes);
        var root = new StackPanel { Spacing = 14, MaxWidth = 640, HorizontalAlignment = HorizontalAlignment.Center };
        root.Children.Add(progress);
        root.Children.Add(card);
        root.Children.Add(hint);
        root.Children.Add(row);

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
            face.Opacity = flipped ? 1 : 0.95;
            hint.Text = !flipped && Str(c, "hint").Length > 0 ? $"提示：{Str(c, "hint")}" : "";
            flip.Content = flipped ? "看正面" : "翻转";
            yes.Visibility = no.Visibility = Visibility.Visible;
        }
        void Next(bool knew)
        {
            if (knew) known++; else unknown.Add(order[index]);
            index++;
            flipped = false;
            Show();
        }
        flip.Click += (_, _) =>
        {
            if (index >= order.Count)
            {
                order = unknown.Count > 0 ? unknown.ToList() : Enumerable.Range(0, cards.Count).ToList();
                unknown.Clear();
                known = 0;
                index = 0;
            }
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
        void Fill(string parentKey, Microsoft.UI.Xaml.Controls.TreeViewNode? parentNode)
        {
            if (!byParent.TryGetValue(parentKey, out var children)) return;
            foreach (var n in children)
            {
                var node = new Microsoft.UI.Xaml.Controls.TreeViewNode { Content = $"{Str(n, "label")}{CiteText(n)}", IsExpanded = true };
                if (parentNode == null) tree.RootNodes.Add(node); else parentNode.Children.Add(node);
                Fill(Str(n, "id"), node);
            }
        }
        Fill("", null);
        var scale = new ScaleTransform { ScaleX = 1, ScaleY = 1 };
        var host = new Grid { RenderTransform = scale, RenderTransformOrigin = new Windows.Foundation.Point(0, 0) };
        host.Children.Add(tree);
        var zoom = new Slider { Minimum = 0.6, Maximum = 2.0, Value = 1, StepFrequency = 0.1, Width = 160, Header = "缩放" };
        zoom.ValueChanged += (_, e) => { scale.ScaleX = scale.ScaleY = e.NewValue; };
        void SetAll(bool expand) { foreach (var r in tree.RootNodes) Expand(r, expand); }
        var expandAll = new Button { Content = "全部展开" };
        expandAll.Click += (_, _) => SetAll(true);
        var collapseAll = new Button { Content = "全部折叠" };
        collapseAll.Click += (_, _) => SetAll(false);
        var bar = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
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

    private static void Expand(Microsoft.UI.Xaml.Controls.TreeViewNode n, bool expand)
    {
        n.IsExpanded = expand;
        foreach (var c in n.Children) Expand(c, expand);
    }

    // report ------------------------------------------------------------------

    private FrameworkElement BuildReport(JsonElement payload)
    {
        var panel = new StackPanel { Spacing = 12, MaxWidth = 820, HorizontalAlignment = HorizontalAlignment.Left };
        foreach (var s in Arr(payload, "sections"))
        {
            panel.Children.Add(new TextBlock { Text = Str(s, "heading"), FontSize = 20, FontWeight = FontWeights.SemiBold, Margin = new Thickness(0, 10, 0, 0), TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true });
            panel.Children.Add(Md.Render(Str(s, "body_md"), n => new Microsoft.UI.Xaml.Documents.Run { Text = $"[{n}]", FontSize = 11 }, 15));
            var takeaways = Arr(s, "key_takeaways").Select(t => t.GetString() ?? "").Where(t => t.Length > 0).ToList();
            if (takeaways.Count > 0)
            {
                var box = new Border { Background = (Brush)Application.Current.Resources["ElevatedBgBrush"], CornerRadius = new CornerRadius(10), Padding = new Thickness(12) };
                var col = new StackPanel { Spacing = 4 };
                col.Children.Add(new TextBlock { Text = "要点", FontWeight = FontWeights.SemiBold });
                foreach (var t in takeaways) col.Children.Add(new TextBlock { Text = $"• {t}", TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true });
                box.Child = col;
                panel.Children.Add(box);
            }
        }
        return new ScrollViewer { Content = panel };
    }

    // slides ------------------------------------------------------------------

    private FrameworkElement BuildSlides(JsonElement payload)
    {
        var slides = Arr(payload, "slides").ToList();
        if (slides.Count == 0) return Message("没有幻灯片。");
        var index = 0;
        var title = new TextBlock { FontSize = 30, FontWeight = FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap };
        var bullets = new StackPanel { Spacing = 8 };
        var notes = new TextBlock { TextWrapping = TextWrapping.Wrap, Opacity = 0.8, IsTextSelectionEnabled = true };
        var counter = new TextBlock { VerticalAlignment = VerticalAlignment.Center };
        var slide = new Border { Background = (Brush)Application.Current.Resources["ElevatedBgBrush"], CornerRadius = new CornerRadius(16), Padding = new Thickness(36), MinHeight = 320 };
        var inner = new StackPanel { Spacing = 18 };
        inner.Children.Add(title);
        inner.Children.Add(bullets);
        slide.Child = inner;
        var prev = new Button { Content = "◀ 上一页" };
        var next = new Button { Content = "下一页 ▶" };
        void Show()
        {
            var s = slides[index];
            var layout = Str(s, "layout");
            title.Text = Str(s, "title");
            title.FontSize = layout is "title" or "section" ? 38 : 28;
            bullets.Children.Clear();
            foreach (var b in Arr(s, "bullets"))
            {
                var isQuote = layout == "quote";
                bullets.Children.Add(new TextBlock { Text = isQuote ? $"“{b.GetString()}”" : $"• {b.GetString()}", TextWrapping = TextWrapping.Wrap, FontSize = 18, IsTextSelectionEnabled = true, FontStyle = isQuote ? Windows.UI.Text.FontStyle.Italic : Windows.UI.Text.FontStyle.Normal });
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
        var root = new StackPanel { Spacing = 12, MaxWidth = 900, HorizontalAlignment = HorizontalAlignment.Left };
        root.Children.Add(slide);
        root.Children.Add(nav);
        root.Children.Add(new Expander { Header = "讲稿", Content = notes, HorizontalAlignment = HorizontalAlignment.Stretch });
        return new ScrollViewer { Content = root };
    }

    private static string PrettyJson(JsonElement e)
    {
        using var ms = new MemoryStream();
        using (var w = new Utf8JsonWriter(ms, new JsonWriterOptions { Indented = true, Encoder = System.Text.Encodings.Web.JavaScriptEncoder.UnsafeRelaxedJsonEscaping })) e.WriteTo(w);
        return System.Text.Encoding.UTF8.GetString(ms.ToArray());
    }

    private FrameworkElement BuildDraftReview(JsonElement art)
    {
        var draft = art.TryGetProperty("draft", out var d) && d.ValueKind != JsonValueKind.Null ? d : default;
        var text = draft.ValueKind == JsonValueKind.String ? draft.GetString() ?? "" : draft.ValueKind == JsonValueKind.Undefined ? "" : PrettyJson(draft);
        // The draft may itself be a JSON string produced by the model; show it as text either way.
        var box = new TextBox { Text = text, AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 320, FontFamily = new FontFamily("Consolas"), Header = "大纲（可以直接修改，保持 JSON 格式）" };
        var ok = new Button { Content = "批准并生成", HorizontalAlignment = HorizontalAlignment.Left };
        ok.Click += async (_, _) =>
        {
            JsonElement? edited = null;
            try
            {
                using var doc = JsonDocument.Parse(box.Text);
                edited = doc.RootElement.Clone();
            }
            catch (JsonException)
            {
                Say(InfoBarSeverity.Warning, "大纲不是有效的 JSON", "请检查括号与引号，或直接批准原始大纲（清空后点击批准）。");
                if (box.Text.Trim().Length > 0) return;
            }
            var args = edited is { } e ? Args(("artifact_id", _id), ("draft", e)) : Args(("artifact_id", _id));
            if (await Core("studio.approve_draft", args, "批准大纲") != null) Say(InfoBarSeverity.Success, "已批准", "开始生成，完成后这里会自动显示。");
        };
        var col = new StackPanel { Spacing = 10 };
        col.Children.Add(new TextBlock { Text = "先看看大纲，确认后再生成，可以省下不必要的用量。", Opacity = 0.8, TextWrapping = TextWrapping.Wrap });
        col.Children.Add(box);
        col.Children.Add(ok);
        return new ScrollViewer { Content = col };
    }

    // infographic -------------------------------------------------------------

    private FrameworkElement BuildInfographic(JsonElement payload)
    {
        var orientation = Str(payload, "orientation");
        var wrap = new VariableSizedWrapGrid { Orientation = Orientation.Horizontal, ItemWidth = orientation == "portrait" ? 300 : 260, ItemHeight = 180, MaximumRowsOrColumns = orientation == "portrait" ? 1 : orientation == "square" ? 2 : 3 };
        foreach (var b in Arr(payload, "blocks"))
        {
            var col = new StackPanel { Spacing = 4 };
            col.Children.Add(new TextBlock { Text = Str(b, "metric"), FontSize = 34, FontWeight = FontWeights.Bold, Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 120, 170, 255)), TextWrapping = TextWrapping.Wrap });
            col.Children.Add(new TextBlock { Text = Str(b, "heading"), FontWeight = FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap });
            col.Children.Add(new TextBlock { Text = $"{Str(b, "summary")}{CiteText(b)}", TextWrapping = TextWrapping.Wrap, Opacity = 0.85, FontSize = 13 });
            wrap.Children.Add(new Border { Background = (Brush)Application.Current.Resources["ElevatedBgBrush"], CornerRadius = new CornerRadius(14), Padding = new Thickness(14), Margin = new Thickness(6), Child = col });
        }
        var root = new StackPanel { Spacing = 10 };
        root.Children.Add(new TextBlock { Text = Str(payload, "subtitle"), Opacity = 0.8 });
        root.Children.Add(wrap);
        root.Children.Add(new TextBlock { Text = "预览为示意排版；导出的 SVG 可在浏览器或设计软件中打开。", FontSize = 11, Opacity = 0.5 });
        return new ScrollViewer { Content = root };
    }

    // audio -------------------------------------------------------------------

    private FrameworkElement BuildAudio(JsonElement payload)
    {
        var root = new StackPanel { Spacing = 12, MaxWidth = 820, HorizontalAlignment = HorizontalAlignment.Left };
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
            // Subtitles follow playback proportionally to the characters spoken so far.
            var total = Math.Max(1, turns.Sum(t => Str(t, "text").Length));
            var timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(400) };
            timer.Tick += (_, _) =>
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
            timer.Start();
            Closed += (_, _) => timer.Stop();
        }
        else
        {
            root.Children.Add(new TextBlock { Text = "没有找到音频文件，只显示文字稿。", Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 200, 120)) });
        }
        foreach (var t in turns)
        {
            var tb = new TextBlock { Text = $"{Str(t, "speaker")}：{Str(t, "text")}", TextWrapping = TextWrapping.Wrap, FontSize = 15, IsTextSelectionEnabled = true, Opacity = 0.9 };
            lines.Add(tb);
            root.Children.Add(tb);
        }
        return new ScrollViewer { Content = root };
    }
}
