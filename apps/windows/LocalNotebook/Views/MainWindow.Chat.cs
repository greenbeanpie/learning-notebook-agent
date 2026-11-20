using LocalNotebook.Interop;
using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Text;
using System.Text.Json;
using Windows.ApplicationModel.DataTransfer;
using Windows.System;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

public sealed partial class MainWindow
{
    /// <summary>One chat message as shown in the list. Streaming updates touch only <see cref="StreamText"/>.</summary>
    private sealed class MessageView
    {
        public string Id = "";
        public string Role = "";
        public StackPanel Root = new() { Spacing = 6 };
        public ContentControl Body = new() { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        public TextBlock StreamText = new() { TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
        public StringBuilder Text = new();
        public Expander? Thoughts;
        public StackPanel ThoughtList = new() { Spacing = 2 };
        public TextBlock ThoughtStream = new() { TextWrapping = TextWrapping.Wrap, Opacity = 0.7, FontSize = 12 };
        public Dictionary<string, TextBlock> Steps = new();
        public Dictionary<int, (string Title, int Page, string Quote)> Citations = new();
        public StackPanel Footer = new() { Spacing = 4 };
        public StackPanel Followups = new() { Spacing = 2 };
        public bool Streaming;
    }

    private MessageView? _streaming;
    private DispatcherTimer? _flushTimer;
    private readonly Dictionary<string, MessageView> _views = new();
    private readonly Services.WebContentHost _math = new();
    private string? _lastUserText;
    private bool _hintShown;

    // ---------- rendering ----------

    private MessageView NewView(string id, string role)
    {
        var v = new MessageView { Id = id, Role = role };
        var isUser = role == "user";
        var header = new TextBlock { Text = isUser ? "你" : "助手", FontSize = 12, Opacity = 0.6 };
        v.Root.Children.Add(header);
        v.Body.Content = v.StreamText;
        v.Root.Children.Add(v.Body);
        v.Footer.Visibility = Visibility.Collapsed;
        v.Root.Children.Add(v.Footer);
        v.Root.Children.Add(v.Followups);
        v.Root.Margin = new Thickness(0, 6, 0, 10);
        if (isUser) v.Root.HorizontalAlignment = HorizontalAlignment.Stretch;
        return v;
    }

    private Microsoft.UI.Xaml.Documents.Inline? CitationChip(MessageView v, int no)
    {
        var tip = v.Citations.TryGetValue(no, out var c)
            ? $"{c.Title}{(c.Page > 0 ? $" · 第 {c.Page} 页" : "")}{Environment.NewLine}{c.Quote}"
            : "引用";
        var link = new Microsoft.UI.Xaml.Documents.Hyperlink { UnderlineStyle = Microsoft.UI.Xaml.Documents.UnderlineStyle.None };
        link.Inlines.Add(new Microsoft.UI.Xaml.Documents.Run { Text = $"[{no}]", FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        ToolTipService.SetToolTip(link, tip);
        var messageId = v.Id;
        link.Click += async (_, _) => await ShowCitationAsync(messageId, no);
        return link;
    }

    private void RenderFinal(MessageView v, string markdown)
    {
        v.Streaming = false;
        v.Body.Content = v.Role == "user"
            ? new TextBlock { Text = markdown, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true }
            : Md.Render(markdown, n => v.Citations.ContainsKey(n) ? CitationChip(v, n) : null);
    }

    private void RenderChat(JsonElement history)
    {
        ChatList.Items.Clear();
        _views.Clear();
        var messages = Arr(history, "messages").ToList();
        _hintShown = messages.Count == 0;
        if (messages.Count == 0)
        {
            ChatList.Items.Add(EmptyChatHint());
            return;
        }
        for (var i = 0; i < messages.Count; i++)
        {
            var m = messages[i];
            var v = NewView(Str(m, "id"), Str(m, "role"));
            foreach (var c in Arr(m, "citations"))
            {
                v.Citations[(int)Int(c, "no")] = (Str(c, "source_title"), (int)Int(c, "page_start"), Str(c, "quote"));
            }
            RenderFinal(v, Str(m, "content"));
            if (v.Role == "assistant")
            {
                FillThoughts(v, Arr(m, "thoughts"));
                var status = Str(m, "status");
                var code = Str(m, "error_code");
                if (status is "INCOMPLETE" or "FAILED" or "CANCELLED")
                {
                    var why = status == "CANCELLED" ? "已停止生成。" : code.Length > 0 && Advice(code).Length > 0 ? Advice(code) : "回答没有完成。";
                    v.Footer.Children.Add(new TextBlock { Text = why, Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 180, 120)), TextWrapping = TextWrapping.Wrap });
                }
                AddActions(v, Str(m, "content"), Int(m, "feedback"), i == messages.Count - 1);
                if (i == messages.Count - 1) AddFollowups(v, Arr(m, "followups").Select(f => f.GetString() ?? ""));
            }
            _views[v.Id] = v;
            ChatList.Items.Add(v.Root);
        }
        ChatList.ScrollIntoView(ChatList.Items[^1]);
    }

    private UIElement EmptyChatHint()
    {
        var panel = new StackPanel { Spacing = 6, Margin = new Thickness(0, 20, 0, 0) };
        panel.Children.Add(new TextBlock { Text = "开始提问", FontSize = 16, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        panel.Children.Add(Line("1. 先在左侧添加来源（文件、网页或粘贴文本），并勾选要使用的来源。"));
        panel.Children.Add(Line("2. 在下方输入问题，回答会带有 [1] [2] 这样的引用，点击可查看原文。"));
        panel.Children.Add(Line("3. 右侧 Studio 可以把来源生成为测验、闪卡、思维导图、报告等。"));
        return panel;
    }

    private void FillThoughts(MessageView v, IEnumerable<JsonElement> steps)
    {
        foreach (var s in steps) UpdateStep(v, Str(s, "step_id"), Str(s, "title"), Str(s, "detail"), Str(s, "state"));
    }

    private static string Mark(string state) => state switch { "done" => "✔", "skipped" => "↷", "failed" => "✖", _ => "…" };

    private void UpdateStep(MessageView v, string id, string title, string detail, string state)
    {
        if (!v.Steps.TryGetValue(id, out var tb))
        {
            tb = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.85 };
            v.Steps[id] = tb;
            v.ThoughtList.Children.Add(tb);
        }
        tb.Text = detail.Length > 0 ? $"{Mark(state)} {title} — {detail}" : $"{Mark(state)} {title}";
        EnsureThoughts(v);
    }

    private void EnsureThoughts(MessageView v)
    {
        if (v.Thoughts != null)
        {
            v.Thoughts.Header = $"思考过程（{v.Steps.Count} 步）";
            return;
        }
        var inner = new StackPanel { Spacing = 4 };
        inner.Children.Add(v.ThoughtList);
        inner.Children.Add(v.ThoughtStream);
        v.Thoughts = new Expander { Header = $"思考过程（{v.Steps.Count} 步）", Content = inner, IsExpanded = false, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch };
        v.Root.Children.Insert(1, v.Thoughts);
    }

    private void AddActions(MessageView v, string content, long feedback, bool isLast)
    {
        var bar = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 2 };
        Button Mk(string text, string tip, Func<Task> act)
        {
            var b = new Button { Content = text, Padding = new Thickness(8, 2, 8, 2), FontSize = 12, Background = null, BorderThickness = new Thickness(0) };
            ToolTipService.SetToolTip(b, tip);
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(b, tip);
            b.Click += async (_, _) => await act();
            return b;
        }
        bar.Children.Add(Mk("复制", "复制回答", () =>
        {
            var dp = new DataPackage();
            dp.SetText(content);
            Clipboard.SetContent(dp);
            ShowStatus(InfoBarSeverity.Success, "已复制", "回答已复制到剪贴板。");
            return Task.CompletedTask;
        }));
        bar.Children.Add(Mk("存为笔记", "把这条回答保存为笔记（保留引用）", async () =>
        {
            if (await CoreAsync("note.from_message", Args(("message_id", v.Id)), "保存笔记") != null)
            {
                ShowStatus(InfoBarSeverity.Success, "已保存", "回答已保存为笔记。");
                await RefreshNotebookDataAsync();
            }
        }));
        if (isLast)
        {
            bar.Children.Add(Mk("重新生成", "重新生成这条回答", async () =>
            {
                if (_activeNotebookId == null) return;
                SetBusy(true);
                try { await CoreAsync("chat.regenerate", Args(("notebook_id", _activeNotebookId)), "重新生成"); }
                finally { SetBusy(false); }
                await RefreshNotebookDataAsync();
            }));
        }
        bar.Children.Add(Mk(feedback > 0 ? "👍 已赞" : "👍", "回答有帮助", async () =>
        {
            await CoreAsync("chat.set_feedback", Args(("message_id", v.Id), ("feedback", 1)), "提交反馈");
            await RefreshNotebookDataAsync();
        }));
        bar.Children.Add(Mk(feedback < 0 ? "👎 已踩" : "👎", "回答没有帮助", async () =>
        {
            await CoreAsync("chat.set_feedback", Args(("message_id", v.Id), ("feedback", -1)), "提交反馈");
            await RefreshNotebookDataAsync();
        }));
        if (Md.HasMath(content))
        {
            bar.Children.Add(Mk("公式视图", "用排版好的公式查看这条回答", () => ShowMathAsync(content)));
        }
        v.Footer.Children.Add(bar);
        v.Footer.Visibility = Visibility.Visible;
    }

    private void AddFollowups(MessageView v, IEnumerable<string> questions)
    {
        v.Followups.Children.Clear();
        foreach (var q in questions.Where(q => q.Length > 0))
        {
            var b = new HyperlinkButton { Content = q, Padding = new Thickness(0, 2, 0, 2), Tag = q };
            b.Click += async (s, _) => { if (s is HyperlinkButton hb && hb.Tag is string text) await SendChatAsync(text); };
            v.Followups.Children.Add(b);
        }
    }

    // ---------- sending ----------

    private void SetBusy(bool busy)
    {
        SendBtn.Visibility = busy ? Visibility.Collapsed : Visibility.Visible;
        StopBtn.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        ChatInputBox.IsEnabled = !busy;
    }

    private void OnChatKeyDown(object sender, Microsoft.UI.Xaml.Input.KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter)
        {
            e.Handled = true;
            OnSendChatClick(sender, e);
        }
    }

    private async void OnSendChatClick(object sender, RoutedEventArgs e)
    {
        var text = ChatInputBox.Text.Trim();
        if (text.Length == 0) return;
        ChatInputBox.Text = string.Empty;
        await SendChatAsync(text);
    }

    private async void OnStopChatClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId != null) await CoreAsync("chat.cancel", Args(("notebook_id", _activeNotebookId)), "停止生成");
    }

    private async Task SendChatAsync(string text)
    {
        if (_activeNotebookId == null || _streaming != null) return;
        if (!await EnsurePrivacyAckAsync()) return;
        _lastUserText = text;
        if (_hintShown)
        {
            ChatList.Items.Clear();
            _hintShown = false;
        }
        var user = NewView("pending-user", "user");
        RenderFinal(user, text);
        ChatList.Items.Add(user.Root);
        ChatList.ScrollIntoView(user.Root);
        SetBusy(true);
        try
        {
            var (_, err) = await Api.CallAsync("chat.send", Args(("notebook_id", _activeNotebookId), ("text", text)));
            if (err is { } e) ShowChatError(e);
        }
        finally
        {
            SetBusy(false);
            _streaming = null;
            _flushTimer?.Stop();
        }
        await RefreshNotebookDataAsync();
    }

    private void ShowChatError(ApiErrorDto e)
    {
        var advice = Advice(e.Code);
        ShowStatus(InfoBarSeverity.Error, "对话失败", advice.Length > 0 ? $"{advice}（{e.Code}）" : $"{e.Message}（{e.Code}）");
    }

    private async void OnClearChatClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        if (!await ConfirmAsync("清空对话", "确定清空当前笔记本的全部对话吗？来源和笔记不受影响。", "清空")) return;
        await CoreAsync("chat.clear", Args(("notebook_id", _activeNotebookId)), "清空对话");
        await RefreshNotebookDataAsync();
    }

    // ---------- streaming events ----------

    private async Task OnChatEventAsync(ApiEventDto ev)
    {
        if (_activeNotebookId == null || ev.Topic != $"chat:{_activeNotebookId}") return;
        switch (ev.Event)
        {
            case "chat.started":
            {
                var v = NewView(Str(ev.Data, "message_id"), "assistant");
                v.Streaming = true;
                _streaming = v;
                _views[v.Id] = v;
                ChatList.Items.Add(v.Root);
                ChatList.ScrollIntoView(v.Root);
                _flushTimer ??= CreateFlushTimer();
                _flushTimer.Start();
                break;
            }
            case "chat.thought_step":
                if (_streaming != null)
                    UpdateStep(_streaming, Str(ev.Data, "step_id"), Str(ev.Data, "title"), Str(ev.Data, "detail"), Str(ev.Data, "state"));
                break;
            case "chat.thought_delta":
                if (_streaming != null)
                {
                    EnsureThoughts(_streaming);
                    _streaming.ThoughtStream.Text += Str(ev.Data, "text");
                }
                break;
            case "chat.text_delta":
                _streaming?.Text.Append(Str(ev.Data, "text"));
                break;
            case "chat.citation":
                if (_streaming != null)
                    _streaming.Citations[(int)Int(ev.Data, "no")] = (Str(ev.Data, "title"), (int)Int(ev.Data, "page"), Str(ev.Data, "quote"));
                break;
            case "chat.followups":
                if (_views.TryGetValue(Str(ev.Data, "message_id"), out var fv)) AddFollowups(fv, Arr(ev.Data, "questions").Select(q => q.GetString() ?? ""));
                break;
            case "chat.finished":
            {
                if (_streaming is { } done)
                {
                    FlushStream(done);
                    RenderFinal(done, done.Text.ToString());
                }
                var status = Str(ev.Data, "status");
                if (status is "FAILED" or "INCOMPLETE" && ev.Data.TryGetProperty("error", out var err))
                {
                    ShowChatError(new ApiErrorDto { Code = Str(err, "code"), Message = Str(err, "message") });
                }
                break;
            }
        }
        await Task.CompletedTask;
    }

    private DispatcherTimer CreateFlushTimer()
    {
        var t = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(33) };
        t.Tick += (_, _) => { if (_streaming != null) FlushStream(_streaming); };
        return t;
    }

    private int _flushedLength;
    private void FlushStream(MessageView v)
    {
        if (v.Text.Length == _flushedLength && v.StreamText.Text.Length == v.Text.Length) return;
        v.StreamText.Text = v.Text.ToString();
        _flushedLength = v.Text.Length;
        if (ChatList.Items.Count > 0) ChatList.ScrollIntoView(ChatList.Items[^1]);
    }

    // ---------- citations ----------

    private async Task ShowCitationAsync(string messageId, int no)
    {
        var detail = await CoreAsync("chat.citation_detail", Args(("message_id", messageId), ("citation_no", no)), "读取引用");
        if (detail is not { } d) return;
        var panel = new StackPanel { Spacing = 8, MinWidth = 480, MaxWidth = 640 };
        var removed = Bool(d, "source_removed");
        var page = Int(d, "page_start");
        panel.Children.Add(new TextBlock
        {
            Text = $"{Str(d, "source_title")}{(page > 0 ? $" · 第 {page} 页" : "")}{(Str(d, "section_path").Length > 0 ? $" · {Str(d, "section_path")}" : "")}",
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
            TextWrapping = TextWrapping.Wrap,
        });
        if (removed) panel.Children.Add(new TextBlock { Text = "来源已移除，以下是当时保存的引用片段。", Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 180, 120)) });
        foreach (var p in Arr(d, "prev_chunks")) panel.Children.Add(new TextBlock { Text = Str(p, "content"), Opacity = 0.5, TextWrapping = TextWrapping.Wrap, MaxLines = 4, TextTrimming = TextTrimming.CharacterEllipsis });
        panel.Children.Add(new Border
        {
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 40, 60, 90)),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(10),
            Child = Line(Str(d, "content")),
        });
        foreach (var n in Arr(d, "next_chunks")) panel.Children.Add(new TextBlock { Text = Str(n, "content"), Opacity = 0.5, TextWrapping = TextWrapping.Wrap, MaxLines = 4, TextTrimming = TextTrimming.CharacterEllipsis });
        var scroll = new ScrollViewer { Content = panel, MaxHeight = 520 };
        var dlg = new ContentDialog { Title = $"引用 [{no}]", Content = scroll, CloseButtonText = "关闭", XamlRoot = Content.XamlRoot };
        if (!removed)
        {
            dlg.PrimaryButtonText = "在来源中查看";
            var sourceId = "";
            if (_views.TryGetValue(messageId, out _)) sourceId = await FindSourceIdByTitleAsync(Str(d, "source_title"));
            dlg.IsPrimaryButtonEnabled = sourceId.Length > 0;
            if (await dlg.ShowAsync() == ContentDialogResult.Primary && sourceId.Length > 0)
            {
                await ShowSourcePreviewAsync(sourceId, page > 0 ? (int)page : null, Str(d, "content"));
            }
            return;
        }
        await dlg.ShowAsync();
    }

    private async Task<string> FindSourceIdByTitleAsync(string title)
    {
        if (_activeNotebookId == null) return "";
        var src = await Api.CallAsync("source.list", Args(("notebook_id", _activeNotebookId)));
        if (src.Result is { } r)
        {
            foreach (var s in Arr(r, "sources")) if (Str(s, "title") == title) return Str(s, "id");
        }
        return "";
    }

    // ---------- formulas ----------

    private async Task ShowMathAsync(string text)
    {
        var host = new Grid { MinWidth = 640, Height = 480 };
        var dlg = new ContentDialog { Title = "公式视图", Content = host, CloseButtonText = "关闭", XamlRoot = Content.XamlRoot };
        dlg.Opened += async (_, _) =>
        {
            try { await _math.RenderMathAsync(host, text); }
            catch (Exception ex) { ShowStatus(InfoBarSeverity.Error, "公式视图不可用", ex.Message); }
        };
        await dlg.ShowAsync();
        _math.Release(); // the next view creates a fresh WebView2; an idle one holds tens of MB
    }
}
