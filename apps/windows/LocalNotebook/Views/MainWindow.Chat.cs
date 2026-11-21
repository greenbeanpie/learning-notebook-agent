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
        public StackPanel Root = new() { Spacing = 8 };
        public ContentControl Body = new() { HorizontalContentAlignment = HorizontalAlignment.Stretch };
        public TextBlock StreamText = new() { TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
        public StringBuilder Text = new();
        public Expander? Thoughts;
        public TextBlock? ThoughtsHeaderText;
        public StackPanel ThoughtList = new() { Spacing = 4 };
        public TextBlock ThoughtStream = new() { TextWrapping = TextWrapping.Wrap, Opacity = 0.7, FontSize = 12 };
        public Dictionary<string, TextBlock> Steps = new();
        public Dictionary<int, (string Title, int Page, string Quote)> Citations = new();
        public StackPanel Footer = new() { Spacing = 8 };
        public StackPanel Followups = new() { Spacing = 8, Margin = new Thickness(0, 6, 0, 0) };
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
        v.Root.Margin = new Thickness(0, 8, 0, 14);
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
            : Md.Render(markdown, n => v.Citations.ContainsKey(n) ? CitationChip(v, n) : null, 14, _math, MathRenderHost);
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
            if (v.ThoughtsHeaderText != null) v.ThoughtsHeaderText.Text = $"思考过程（{v.Steps.Count} 步）";
            return;
        }
        var inner = new StackPanel { Spacing = 6, Padding = new Thickness(4, 4, 4, 4) };
        inner.Children.Add(v.ThoughtList);
        inner.Children.Add(v.ThoughtStream);
        var headerRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
        headerRow.Children.Add(new PathIcon
        {
            Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M12,2A7,7 0 0,0 5,9C5,11.38 6.19,13.47 8,14.74V17A1,1 0 0,0 9,18H15A1,1 0 0,0 16,17V14.74C17.81,13.47 19,11.38 19,9A7,7 0 0,0 12,2M9,21A1,1 0 0,0 10,22H14A1,1 0 0,0 15,21V20H9V21Z"),
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentBlueBrush"],
        });
        v.ThoughtsHeaderText = new TextBlock { Text = $"思考过程（{v.Steps.Count} 步）", FontSize = 13, Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"], VerticalAlignment = VerticalAlignment.Center };
        headerRow.Children.Add(v.ThoughtsHeaderText);
        v.Thoughts = new Expander
        {
            Header = headerRow,
            Content = inner,
            IsExpanded = false,
            MinHeight = 42,
            Margin = new Thickness(0, 4, 0, 4),
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"],
            BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(12),
        };
        v.Root.Children.Insert(1, v.Thoughts);
    }

    private void AddActions(MessageView v, string content, long feedback, bool isLast)
    {
        var bar = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Margin = new Thickness(0, 6, 0, 2) };
        Button Mk(string text, string tip, Func<Task> act)
        {
            var b = new Button { Content = text, MinHeight = 36, Padding = new Thickness(12, 6, 12, 6), CornerRadius = new CornerRadius(18), FontSize = 12, Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent), BorderThickness = new Thickness(0) };
            ToolTipService.SetToolTip(b, tip);
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(b, tip);
            b.Click += async (_, _) => await act();
            return b;
        }
        var saveContent = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        saveContent.Children.Add(new PathIcon
        {
            Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M16,12V4H17V2H7V4H8V12L6,14V16H11.2V22H12.8V16H18V14L16,12Z"),
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
        });
        saveContent.Children.Add(new TextBlock { Text = "保存到笔记", FontSize = 12, Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"], VerticalAlignment = VerticalAlignment.Center });
        var saveBtn = new Button
        {
            Content = saveContent,
            MinHeight = 36,
            Padding = new Thickness(12, 6, 12, 6),
            CornerRadius = new CornerRadius(18),
            Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"],
            BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["StrongBorderBrush"],
            BorderThickness = new Thickness(1),
        };
        ToolTipService.SetToolTip(saveBtn, "把这条回答保存为笔记（保留引用）");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(saveBtn, "把这条回答保存为笔记（保留引用）");
        saveBtn.Click += async (_, _) =>
        {
            if (await CoreAsync("note.from_message", Args(("message_id", v.Id)), "保存笔记") != null)
            {
                ShowStatus(InfoBarSeverity.Success, "已保存", "回答已保存为笔记。");
                await RefreshNotebookDataAsync();
            }
        };
        bar.Children.Add(saveBtn);

        var copyBtn = new Button
        {
            Content = new PathIcon
            {
                Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M19,21H8V7H19M19,5H8A2,2 0 0,0 6,7V21A2,2 0 0,0 8,23H19A2,2 0 0,0 21,21V7A2,2 0 0,0 19,5M16,1H4A2,2 0 0,0 2,3V17H4V3H16V1Z"),
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
            },
            MinWidth = 38,
            MinHeight = 36,
            Padding = new Thickness(10, 6, 10, 6),
            CornerRadius = new CornerRadius(18),
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
        };
        ToolTipService.SetToolTip(copyBtn, "复制回答");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(copyBtn, "复制回答");
        copyBtn.Click += (_, _) =>
        {
            var dp = new DataPackage();
            dp.SetText(content);
            Clipboard.SetContent(dp);
            ShowStatus(InfoBarSeverity.Success, "已复制", "回答已复制到剪贴板。");
        };
        bar.Children.Add(copyBtn);
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
        Button MkIcon(string pathData, string label, bool active, string tip, Func<Task> act)
        {
            var sp = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
            sp.Children.Add(new PathIcon
            {
                Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), pathData),
                Foreground = active
                    ? (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentBlueBrush"]
                    : (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
            });
            if (label.Length > 0)
            {
                sp.Children.Add(new TextBlock
                {
                    Text = label,
                    FontSize = 12,
                    Foreground = active
                        ? (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentBlueBrush"]
                        : (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
                    VerticalAlignment = VerticalAlignment.Center,
                });
            }
            var b = new Button
            {
                Content = sp,
                MinWidth = 38,
                MinHeight = 36,
                Padding = new Thickness(10, 6, 10, 6),
                CornerRadius = new CornerRadius(18),
                Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
                BorderThickness = new Thickness(0),
            };
            ToolTipService.SetToolTip(b, tip);
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(b, tip);
            b.Click += async (_, _) => await act();
            return b;
        }
        bar.Children.Add(MkIcon(
            "M23,10C23,8.89 22.1,8 21,8H14.68L15.64,3.43C15.66,3.33 15.67,3.22 15.67,3.11C15.67,2.7 15.5,2.32 15.23,2.05L14.17,1L7.59,7.58C7.22,7.95 7,8.45 7,9V19A2,2 0 0,0 9,21H18C18.83,21 19.54,20.5 19.84,19.78L22.86,12.73C22.95,12.5 23,12.26 23,12V10M1,21H5V9H1V21Z",
            feedback > 0 ? "已赞" : "", feedback > 0, "回答有帮助", async () =>
        {
            await CoreAsync("chat.set_feedback", Args(("message_id", v.Id), ("feedback", 1)), "提交反馈");
            await RefreshNotebookDataAsync();
        }));
        bar.Children.Add(MkIcon(
            "M19,15H23V3H19M15,3H6C5.17,3 4.46,3.5 4.16,4.22L1.14,11.27C1.05,11.5 1,11.74 1,12V14A2,2 0 0,0 3,16H9.31L8.36,20.57C8.34,20.67 8.33,20.77 8.33,20.88C8.33,21.3 8.5,21.67 8.77,21.94L9.83,23L16.41,16.41C16.78,16.05 17,15.55 17,15V5A2,2 0 0,0 15,3Z",
            feedback < 0 ? "已踩" : "", feedback < 0, "回答没有帮助", async () =>
        {
            await CoreAsync("chat.set_feedback", Args(("message_id", v.Id), ("feedback", -1)), "提交反馈");
            await RefreshNotebookDataAsync();
        }));
        v.Footer.Children.Add(bar);
        v.Footer.Visibility = Visibility.Visible;
    }

    private void AddFollowups(MessageView v, IEnumerable<string> questions)
    {
        v.Followups.Children.Clear();
        foreach (var q in questions.Where(q => q.Length > 0))
        {
            var b = new Button
            {
                Content = new TextBlock
                {
                    Text = q,
                    FontSize = 13,
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                    TextWrapping = TextWrapping.Wrap,
                },
                Tag = q,
                MinHeight = 42,
                Padding = new Thickness(14, 10, 14, 10),
                CornerRadius = new CornerRadius(12),
                Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["PanelBgBrush"],
                BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["StrongBorderBrush"],
                BorderThickness = new Thickness(1),
                HorizontalAlignment = HorizontalAlignment.Left,
                HorizontalContentAlignment = HorizontalAlignment.Left,
            };
            b.Click += async (s, _) => { if (s is Button btn && btn.Tag is string text) await SendChatAsync(text); };
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
}
