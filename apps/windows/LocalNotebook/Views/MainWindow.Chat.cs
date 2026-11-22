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
        public StreamingMarkdownView? StreamView;
        public StringBuilder Text = new();
        public StackPanel? ThoughtsContainer;
        public PathIcon? ThoughtsChevron;
        public bool ThoughtsExpanded;
        public StackPanel ThoughtList = new() { Spacing = 10, Margin = new Thickness(4, 8, 0, 6), Visibility = Visibility.Collapsed };
        public Dictionary<string, FrameworkElement> Steps = new();
        public Dictionary<int, (string Title, int Page, string Quote)> Citations = new();
        public StackPanel Artifacts = new() { Spacing = 8, Visibility = Visibility.Collapsed };
        public Dictionary<string, ChatArtifactCard> ArtifactCards = new();
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
    private ScrollViewer? _chatScrollViewer;
    private bool _chatStickToBottom = true;
    private bool _programmaticChatScroll;
    private bool _scrollPending;
    private double _lastScrollableHeight;
    private double _lastVerticalOffset;

    private ScrollViewer? EnsureChatScrollViewer()
    {
        if (_chatScrollViewer != null) return _chatScrollViewer;
        _chatScrollViewer = FindDescendant<ScrollViewer>(ChatList);
        if (_chatScrollViewer != null)
        {
            _lastScrollableHeight = _chatScrollViewer.ScrollableHeight;
            _lastVerticalOffset = _chatScrollViewer.VerticalOffset;
            _chatScrollViewer.ViewChanged += (_, _) =>
            {
                if (_chatScrollViewer == null) return;
                if (_programmaticChatScroll)
                {
                    _lastScrollableHeight = _chatScrollViewer.ScrollableHeight;
                    _lastVerticalOffset = _chatScrollViewer.VerticalOffset;
                    return;
                }
                var sh = _chatScrollViewer.ScrollableHeight;
                var vo = _chatScrollViewer.VerticalOffset;
                if (sh > _lastScrollableHeight + 0.5 && vo >= _lastVerticalOffset - 1.0)
                {
                    if (_chatStickToBottom) ScrollChatToBottomIfSticky();
                }
                else
                {
                    _chatStickToBottom = (sh - vo) <= 36.0;
                }
                _lastScrollableHeight = sh;
                _lastVerticalOffset = vo;
            };
        }
        return _chatScrollViewer;
    }

    private static T? FindDescendant<T>(DependencyObject parent) where T : DependencyObject
    {
        var count = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChildrenCount(parent);
        for (var i = 0; i < count; i++)
        {
            var child = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChild(parent, i);
            if (child is T match) return match;
            var nested = FindDescendant<T>(child);
            if (nested != null) return nested;
        }
        return null;
    }

    private void ScrollChatToBottomIfSticky()
    {
        if (!_chatStickToBottom || _scrollPending) return;
        _scrollPending = true;
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () =>
        {
            _scrollPending = false;
            if (!_chatStickToBottom) return;
            var sv = EnsureChatScrollViewer();
            if (sv == null)
            {
                if (ChatList.Items.Count > 0) ChatList.ScrollIntoView(ChatList.Items[^1]);
                return;
            }
            if (sv.ScrollableHeight - sv.VerticalOffset <= 0.5) return;
            _programmaticChatScroll = true;
            try
            {
                sv.ChangeView(null, sv.ScrollableHeight, null, true);
                _lastScrollableHeight = sv.ScrollableHeight;
                _lastVerticalOffset = sv.ScrollableHeight;
            }
            finally
            {
                _programmaticChatScroll = false;
            }
        });
    }

    // ---------- rendering ----------

    private MessageView NewView(string id, string role)
    {
        var v = new MessageView { Id = id, Role = role };
        var isUser = role == "user";
        var header = new TextBlock { Text = isUser ? "你" : "助手", FontSize = 12, Opacity = 0.6 };
        v.Root.Children.Add(header);
        v.Body.Content = v.StreamText;
        v.Root.Children.Add(v.Body);
        v.Root.Children.Add(v.Artifacts);
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
        var messageId = v.Id;
        return Md.SuperscriptCitation(no, tip, () => _ = ShowCitationAsync(messageId, no));
    }

    private void RenderFinal(MessageView v, string markdown)
    {
        v.Streaming = false;
        v.StreamView = null;
        v.Body.Content = v.Role == "user"
            ? new TextBlock { Text = markdown, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true }
            : Md.Render(markdown, n => v.Citations.ContainsKey(n) ? CitationChip(v, n) : null, 14, _math, MathRenderHost);
    }

    private static void DisposeViewArtifacts(MessageView v)
    {
        foreach (var card in v.ArtifactCards.Values) card.Dispose();
        v.ArtifactCards.Clear();
        v.Artifacts.Children.Clear();
        v.Artifacts.Visibility = Visibility.Collapsed;
    }

    private void UpsertChatArtifact(MessageView v, JsonElement artifact)
    {
        var aid = Str(artifact, "id");
        if (aid.Length == 0) return;
        if (v.ArtifactCards.TryGetValue(aid, out var existing))
        {
            existing.Update(artifact);
            return;
        }
        var card = new ChatArtifactCard(
            artifact,
            this,
            ShowStatus,
            () => _ = RefreshStudioArtifactsAsync(),
            id => OpenArtifact(id),
            _math,
            MathRenderHost);
        v.ArtifactCards[aid] = card;
        v.Artifacts.Children.Add(card.Root);
        v.Artifacts.Visibility = Visibility.Visible;
    }

    private void RenderChat(JsonElement history)
    {
        foreach (var old in _views.Values) DisposeViewArtifacts(old);
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
                foreach (var art in Arr(m, "artifacts")) UpsertChatArtifact(v, art);
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
        _chatStickToBottom = true;
        ScrollChatToBottomIfSticky();
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

    private const string RingIconPath = "M12,6A6,6 0 1,0 18,12A6,6 0 0,0 12,6M12,9A3,3 0 1,1 9,12A3,3 0 0,1 12,9Z";
    private const string ChevronDownPath = "M7.41,8.58L12,13.17L16.59,8.58L18,10L12,16L6,10L7.41,8.58Z";
    private const string ChevronUpPath = "M7.41,15.41L12,10.83L16.59,15.41L18,14L12,8L6,14L7.41,15.41Z";
    private const string BrainIconPath = "M13,3A3,3 0 0,1 16,6C16,6.35 15.94,6.68 15.83,7H16.5A2.5,2.5 0 0,1 19,9.5C19,10.22 18.69,10.87 18.19,11.33C18.69,11.83 19,12.53 19,13.3A2.7,2.7 0 0,1 16.3,16H16V17.5A2.5,2.5 0 0,1 13.5,20H12.5V3H13M11,3H11.5V20H10.5A2.5,2.5 0 0,1 8,17.5V16H7.7A2.7,2.7 0 0,1 5,13.3C5,12.53 5.31,11.83 5.81,11.33C5.31,10.87 5,10.22 5,9.5A2.5,2.5 0 0,1 7.5,7H8.17C8.06,6.68 8,6.35 8,6A3,3 0 0,1 11,3Z";
    private const string SearchDocIconPath = "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H12.5C12.18,21.39 12,20.72 12,20H6V4H13V9H18V12.08C18.71,12.22 19.38,12.5 20,12.88V8L14,2M17.5,14A3.5,3.5 0 0,0 14,17.5A3.5,3.5 0 0,0 17.5,21C18.23,21 18.9,20.77 19.45,20.38L21.54,22.46L22.96,21.04L20.88,18.95C21.27,18.4 21.5,17.73 21.5,17.5A3.5,3.5 0 0,0 17.5,14M17.5,16A1.5,1.5 0 0,1 19,17.5A1.5,1.5 0 0,1 17.5,19A1.5,1.5 0 0,1 16,17.5A1.5,1.5 0 0,1 17.5,16Z";
    private const string ReadContextIconPath = "M18,22A2,2 0 0,0 20,20V4C20,2.89 19.1,2 18,2H12V9L9.5,7.5L7,9V2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18Z";

    private static Microsoft.UI.Xaml.Media.Geometry Geom(string path) =>
        (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), path);

    private void FillThoughts(MessageView v, IEnumerable<JsonElement> steps)
    {
        foreach (var s in steps) UpdateStep(v, Str(s, "step_id"), Str(s, "kind"), Str(s, "title"), Str(s, "detail"), Str(s, "state"));
    }

    private static bool IsToolKind(string kind) =>
        kind is "retrieve" or "retrieve_fallback" or "tool_search" or "tool_read_context" or "tool_list_sources" or "tool_call" or "tool_studio";

    private void UpdateStep(MessageView v, string id, string kind, string title, string detail, string state)
    {
        var isTool = IsToolKind(kind);
        var iconPath = kind switch
        {
            "tool_read_context" or "tool_list_sources" => ReadContextIconPath,
            "retrieve" or "retrieve_fallback" or "tool_search" or "tool_call" => SearchDocIconPath,
            _ => BrainIconPath,
        };

        var mutedBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"];
        var subBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"];
        var borderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["StrongBorderBrush"];

        var container = new StackPanel { Spacing = 4 };
        var headRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, VerticalAlignment = VerticalAlignment.Center };
        headRow.Children.Add(new Viewbox
        {
            Width = 16,
            Height = 16,
            VerticalAlignment = VerticalAlignment.Center,
            Child = new PathIcon { Data = Geom(iconPath), Foreground = mutedBrush },
        });
        headRow.Children.Add(new TextBlock
        {
            Text = title,
            FontSize = 13,
            FontWeight = isTool ? Microsoft.UI.Text.FontWeights.Normal : Microsoft.UI.Text.FontWeights.Medium,
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"],
            TextWrapping = TextWrapping.Wrap,
            VerticalAlignment = VerticalAlignment.Center,
        });
        container.Children.Add(headRow);

        if (!isTool && detail.Length > 0)
        {
            var bodyRow = new Grid { Margin = new Thickness(7, 2, 0, 2) };
            bodyRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            bodyRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var guide = new Border { Width = 1, Background = borderBrush, Margin = new Thickness(0, 2, 16, 2) };
            var desc = new TextBlock { Text = detail, FontSize = 13, Foreground = subBrush, TextWrapping = TextWrapping.Wrap };
            Grid.SetColumn(desc, 1);
            bodyRow.Children.Add(guide);
            bodyRow.Children.Add(desc);
            container.Children.Add(bodyRow);
        }

        if (v.Steps.TryGetValue(id, out var existing))
        {
            var idx = v.ThoughtList.Children.IndexOf(existing);
            if (idx >= 0) v.ThoughtList.Children[idx] = container;
        }
        else
        {
            v.ThoughtList.Children.Add(container);
        }
        v.Steps[id] = container;
        EnsureThoughts(v);
    }

    private void EnsureThoughts(MessageView v)
    {
        if (v.ThoughtsContainer != null) return;

        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
        row.Children.Add(new Viewbox
        {
            Width = 14,
            Height = 14,
            VerticalAlignment = VerticalAlignment.Center,
            Child = new PathIcon { Data = Geom(RingIconPath), Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"] },
        });
        row.Children.Add(new TextBlock
        {
            Text = "Thoughts",
            FontSize = 13,
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
            VerticalAlignment = VerticalAlignment.Center,
        });
        v.ThoughtsChevron = new PathIcon
        {
            Data = Geom(ChevronDownPath),
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
        };
        row.Children.Add(new Viewbox { Width = 14, Height = 14, VerticalAlignment = VerticalAlignment.Center, Child = v.ThoughtsChevron });

        var toggle = new Button
        {
            Content = row,
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
            Padding = new Thickness(2, 4, 6, 4),
            MinHeight = 30,
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        toggle.Click += (_, _) =>
        {
            v.ThoughtsExpanded = !v.ThoughtsExpanded;
            v.ThoughtList.Visibility = v.ThoughtsExpanded ? Visibility.Visible : Visibility.Collapsed;
            if (v.ThoughtsChevron != null) v.ThoughtsChevron.Data = Geom(v.ThoughtsExpanded ? ChevronUpPath : ChevronDownPath);
        };

        v.ThoughtsContainer = new StackPanel { Spacing = 4, Margin = new Thickness(0, 2, 0, 6) };
        v.ThoughtsContainer.Children.Add(toggle);
        v.ThoughtsContainer.Children.Add(v.ThoughtList);
        v.Root.Children.Insert(1, v.ThoughtsContainer);
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
        saveContent.Children.Add(new Viewbox
        {
            Width = 15,
            Height = 15,
            VerticalAlignment = VerticalAlignment.Center,
            Child = new PathIcon
            {
                Width = 24,
                Height = 24,
                Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M16,12V4H17V2H7V4H8V12L6,14V16H11.2V22H12.8V16H18V14L16,12Z"),
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
            },
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
                    Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M19,21H8V7H19M19,5H8A2,2 0 0,0 6,7V21A2,2 0 0,0 8,23H19A2,2 0 0,0 21,21V7A2,2 0 0,0 19,5M16,1H4A2,2 0 0,0 2,3V17H4V3H16V1Z"),
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
                },
            },
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
                if (_activeNotebookId == null || _streaming != null) return;
                if (ChatList.Items.Count == 0 || !ReferenceEquals(ChatList.Items[^1], v.Root)) return;
                // Roll back the current turn's assistant bubble in UI before streaming the new one.
                DisposeViewArtifacts(v);
                ChatList.Items.RemoveAt(ChatList.Items.Count - 1);
                _views.Remove(v.Id);
                _flushedLength = 0;
                _chatStickToBottom = true;
                SetBusy(true);
                try
                {
                    var (_, err) = await Api.CallAsync("chat.regenerate", Args(("notebook_id", _activeNotebookId)));
                    if (err is { } e) ShowChatError(e);
                }
                finally
                {
                    SetBusy(false);
                    _streaming = null;
                    _flushTimer?.Stop();
                }
                await RefreshNotebookDataAsync();
            }));
        }
        Button MkIcon(string pathData, string label, bool active, string tip, Func<Task> act)
        {
            var iconView = new Viewbox
            {
                Width = 16,
                Height = 16,
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
                Child = new PathIcon
                {
                    Width = 24,
                    Height = 24,
                    Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), pathData),
                    Foreground = active
                        ? (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentBlueBrush"]
                        : (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
                },
            };
            object btnContent;
            if (label.Length > 0)
            {
                var sp = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
                sp.Children.Add(iconView);
                sp.Children.Add(new TextBlock
                {
                    Text = label,
                    FontSize = 12,
                    Foreground = active
                        ? (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentBlueBrush"]
                        : (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
                    VerticalAlignment = VerticalAlignment.Center,
                });
                btnContent = sp;
            }
            else
            {
                btnContent = iconView;
            }
            var b = new Button
            {
                Content = btnContent,
                MinWidth = 36,
                MinHeight = 36,
                Height = 36,
                Padding = label.Length > 0 ? new Thickness(12, 0, 12, 0) : new Thickness(0),
                CornerRadius = new CornerRadius(18),
                HorizontalContentAlignment = HorizontalAlignment.Center,
                VerticalContentAlignment = VerticalAlignment.Center,
                Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
                BorderThickness = new Thickness(0),
            };
            if (label.Length == 0) b.Width = 36;
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
        _flushedLength = 0;
        _chatStickToBottom = true;
        ScrollChatToBottomIfSticky();
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
                v.StreamView = new StreamingMarkdownView(
                    n => v.Citations.ContainsKey(n) ? CitationChip(v, n) : null,
                    14,
                    _math,
                    MathRenderHost);
                v.Body.Content = v.StreamView.Root;
                _streaming = v;
                _flushedLength = 0;
                _views[v.Id] = v;
                ChatList.Items.Add(v.Root);
                ScrollChatToBottomIfSticky();
                _flushTimer ??= CreateFlushTimer();
                _flushTimer.Start();
                break;
            }
            case "chat.thought_step":
                if (_streaming != null)
                {
                    UpdateStep(_streaming, Str(ev.Data, "step_id"), Str(ev.Data, "kind"), Str(ev.Data, "title"), Str(ev.Data, "detail"), Str(ev.Data, "state"));
                    ScrollChatToBottomIfSticky();
                }
                break;
            case "chat.thought_delta":
                // Raw internal CoT is hidden from display (Requirement 1).
                break;
            case "chat.text_delta":
                _streaming?.Text.Append(Str(ev.Data, "text"));
                break;
            case "chat.citation":
                if (_streaming != null)
                    _streaming.Citations[(int)Int(ev.Data, "no")] = (Str(ev.Data, "title"), (int)Int(ev.Data, "page"), Str(ev.Data, "quote"));
                break;
            case "chat.artifact":
            {
                var mid = Str(ev.Data, "message_id");
                var target = (mid.Length > 0 && _views.TryGetValue(mid, out var mv)) ? mv : _streaming;
                if (target != null && ev.Data.TryGetProperty("artifact", out var artEl) && artEl.ValueKind == JsonValueKind.Object)
                {
                    UpsertChatArtifact(target, artEl);
                    ScrollChatToBottomIfSticky();
                }
                break;
            }
            case "chat.followups":
                if (_views.TryGetValue(Str(ev.Data, "message_id"), out var fv)) AddFollowups(fv, Arr(ev.Data, "questions").Select(q => q.GetString() ?? ""));
                break;
            case "chat.finished":
            {
                if (_streaming is { } done)
                {
                    _flushedLength = done.Text.Length;
                    RenderFinal(done, done.Text.ToString());
                    ScrollChatToBottomIfSticky();
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
        var t = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(80) };
        t.Tick += (_, _) => { if (_streaming != null) FlushStream(_streaming); };
        return t;
    }

    private int _flushedLength;
    private void FlushStream(MessageView v)
    {
        if (v.Text.Length == _flushedLength) return;
        _flushedLength = v.Text.Length;
        if (v.StreamView != null)
        {
            v.StreamView.Update(v.Text.ToString());
        }
        else
        {
            v.StreamText.Text = v.Text.ToString();
        }
        ScrollChatToBottomIfSticky();
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
