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
        public FlowWrapPanel Followups = new() { HorizontalSpacing = 8, VerticalSpacing = 8, Margin = new Thickness(0, 6, 0, 0) };
        public bool Streaming;
    }

    private MessageView? _streaming;
    private DispatcherTimer? _flushTimer;
    private readonly Dictionary<string, MessageView> _views = new();
    private readonly Services.WebContentHost _math = new();
    private string? _lastUserText;
    private ImageAttachments _chatImages = null!;

    /// <summary>Picture attachments for the chat box: button, paste, drop, thumbnail strip.</summary>
    private void InitChatImages()
    {
        _chatImages = new ImageAttachments(
            () => WinRT.Interop.WindowNative.GetWindowHandle(this),
            () => Content.XamlRoot,
            msg => ShowStatus(InfoBarSeverity.Warning, L10n.T("img.title"), msg));
        ChatAttachStripHost.Content = _chatImages.Strip;
        ChatAttachBtnHost.Content = _chatImages.AttachButton;
        _chatImages.Hook(ChatPaneBorder, ChatInputBox);
    }

    // Unsent chat input (text + attachments) per notebook, kept while the app runs.
    private readonly Dictionary<string, (string Text, IReadOnlyList<PendingItem> Items)> _chatDrafts = new();

    /// <summary>Stores the chat box of notebook <paramref name="from"/> and shows the draft of <paramref name="to"/>.</summary>
    private void SwapChatDraft(string? from, string? to)
    {
        if (from == to || _chatImages == null) return;
        if (from != null)
        {
            if (ChatInputBox.Text.Length > 0 || _chatImages.HasItems)
                _chatDrafts[from] = (ChatInputBox.Text, _chatImages.Snapshot());
            else
                _chatDrafts.Remove(from);
        }
        if (to != null && _chatDrafts.Remove(to, out var d))
        {
            ChatInputBox.Text = d.Text;
            _chatImages.Restore(d.Items);
        }
        else
        {
            ChatInputBox.Text = string.Empty;
            _chatImages.Clear();
        }
    }

    private bool _hintShown;
    private ScrollViewer? _chatScrollViewer;
    private bool _chatStickToBottom = true;
    private bool _scrollPending;
    private double _lastScrollableHeight;
    private double _lastVerticalOffset;
    private bool _chatScrollHooked;

    /// The ListView's inner ScrollViewer. It is looked up again when the cached one has left the
    /// visual tree (the template is re-applied, e.g. after the list is rebuilt for another notebook),
    /// otherwise following the latest message silently stops working.
    private ScrollViewer? EnsureChatScrollViewer()
    {
        if (_chatScrollViewer is { XamlRoot: null })
        {
            _chatScrollViewer = null;
        }
        if (_chatScrollViewer != null) return _chatScrollViewer;
        _chatScrollViewer = FindDescendant<ScrollViewer>(ChatList);
        if (_chatScrollViewer != null)
        {
            _lastScrollableHeight = _chatScrollViewer.ScrollableHeight;
            _lastVerticalOffset = _chatScrollViewer.VerticalOffset;
            _chatScrollViewer.ViewChanged += OnChatViewChanged;
        }
        if (!_chatScrollHooked)
        {
            // New content (a streamed delta, an item that is measured late) grows the extent
            // without moving the offset, and ViewChanged is not raised for that: follow it here.
            // The scroll viewer is looked up here too, so a late-applied template is still found.
            _chatScrollHooked = true;
            ChatList.LayoutUpdated += (_, _) =>
            {
                if (!_chatStickToBottom || _scrollPending) return;
                var sv = EnsureChatScrollViewer();
                if (sv != null && sv.ScrollableHeight - sv.VerticalOffset > 0.5) ScrollChatToBottomIfSticky();
            };
            // What the user does with the list is observed directly (also when the event is
            // handled by the inner ScrollViewer), because the scroll offset alone cannot tell a
            // user scroll from a layout change.
            ChatList.AddHandler(UIElement.PointerWheelChangedEvent, new Microsoft.UI.Xaml.Input.PointerEventHandler((_, e) =>
            {
                MarkUserScroll();
                if (e.GetCurrentPoint(ChatList).Properties.MouseWheelDelta > 0) _chatStickToBottom = false;
            }), true);
            ChatList.AddHandler(UIElement.PointerPressedEvent, new Microsoft.UI.Xaml.Input.PointerEventHandler((_, _) => _userDragging = true), true);
            Microsoft.UI.Xaml.Input.PointerEventHandler released = (_, _) => { _userDragging = false; MarkUserScroll(); };
            ChatList.AddHandler(UIElement.PointerReleasedEvent, released, true);
            ChatList.AddHandler(UIElement.PointerCanceledEvent, released, true);
            ChatList.AddHandler(UIElement.PointerCaptureLostEvent, released, true);
            ChatList.AddHandler(UIElement.KeyDownEvent, new Microsoft.UI.Xaml.Input.KeyEventHandler((_, e) =>
            {
                if (e.Key is Windows.System.VirtualKey.Up or Windows.System.VirtualKey.PageUp or Windows.System.VirtualKey.Home)
                {
                    MarkUserScroll();
                    _chatStickToBottom = false;
                }
            }), true);
        }
        return _chatScrollViewer;
    }

    private bool _userDragging;
    private DateTime _userScrollUntil = DateTime.MinValue;

    /// The user touched the list (wheel, finger, scroll bar, keys); inertia scrolling after the
    /// touch is still theirs for a moment.
    private void MarkUserScroll() => _userScrollUntil = DateTime.UtcNow.AddMilliseconds(900);

    /// "Following" is decided only by what the user does. The offset moving up counts as the user
    /// scrolling up only while the user is actually interacting with the list: layout changes
    /// (parallel agents rewriting their step lists, items re-measured, the extent shrinking and
    /// growing again between two events) also move the offset and must never switch following off.
    /// Reaching the bottom by scrolling down (or by our own scroll, which only goes down) resumes it.
    private void OnChatViewChanged(object? sender, ScrollViewerViewChangedEventArgs e)
    {
        var sv = _chatScrollViewer;
        if (sv == null) return;
        var sh = sv.ScrollableHeight;
        var vo = sv.VerticalOffset;
        var movedUp = vo < _lastVerticalOffset - 1.0;
        var userActive = _userDragging || DateTime.UtcNow < _userScrollUntil;
        if (movedUp && userActive)
        {
            _chatStickToBottom = false;
        }
        else if (!movedUp && sh - vo <= 36.0)
        {
            _chatStickToBottom = true;
        }
        else if (_chatStickToBottom && sh - vo > 0.5)
        {
            ScrollChatToBottomIfSticky();
        }
        _lastScrollableHeight = sh;
        _lastVerticalOffset = vo;
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
        // Normal priority: at Low it can starve while several agents keep the UI thread busy
        // with updates, which is exactly when following must keep working.
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Normal, () =>
        {
            _scrollPending = false;
            if (!_chatStickToBottom) return;
            var sv = EnsureChatScrollViewer();
            if (sv == null)
            {
                if (ChatList.Items.Count > 0) ChatList.ScrollIntoView(ChatList.Items[^1]);
                return;
            }
            // The items added just before this call may not be measured yet: measure first so the
            // bottom we scroll to is the real one.
            try { sv.UpdateLayout(); } catch { }
            if (sv.ScrollableHeight - sv.VerticalOffset <= 0.5) return;
            sv.ChangeView(null, sv.ScrollableHeight, null, true);
            _lastScrollableHeight = sv.ScrollableHeight;
            _lastVerticalOffset = sv.ScrollableHeight;
            // Content that lays out after this (images, markdown blocks) is caught by the
            // LayoutUpdated / ViewChanged follow-ups while following is on.
        });
    }

    // ---------- rendering ----------

    private MessageView NewView(string id, string role)
    {
        var v = new MessageView { Id = id, Role = role };
        var isUser = role == "user";
        var header = new TextBlock { Text = isUser ? L10n.T("chat.you") : L10n.T("chat.assistant"), FontSize = 12, Opacity = 0.6 };
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
            ? $"{c.Title}{(c.Page > 0 ? $" · {L10n.Fmt("cite.page", c.Page)}" : "")} ({L10n.T("cite.click_hint")})"
            : L10n.T("cite.header");
        var messageId = v.Id;
        return Md.SuperscriptCitation(no, tip, anchor => _ = ShowCitationPopoverAsync(anchor, messageId, no));
    }

    private void RenderFinal(MessageView v, string markdown)
    {
        v.Streaming = false;
        v.StreamView = null;
        v.Body.Content = v.Role == "user"
            ? new TextBlock { Text = markdown, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true }
            : Md.Render(markdown, n => v.Citations.ContainsKey(n) ? CitationChip(v, n) : null, L10n.Prefs.FontSize, _math, MathRenderHost);
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
            if (v.Role == "user" && Arr(m, "attachments").ToList() is { Count: > 0 } atts)
                v.Root.Children.Insert(1, ImageAttachments.Gallery(atts, () => Content.XamlRoot, PromoteAttachmentAsync));
            if (v.Role == "assistant")
            {
                FillThoughts(v, Arr(m, "thoughts"));
                foreach (var art in Arr(m, "artifacts")) UpsertChatArtifact(v, art);
                var status = Str(m, "status");
                var code = Str(m, "error_code");
                if (status is "INCOMPLETE" or "FAILED" or "CANCELLED")
                {
                    var why = status == "CANCELLED" ? L10n.T("studio.cancelled") : code.Length > 0 && Advice(code).Length > 0 ? Advice(code) : L10n.T("chat.incomplete");
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
        panel.Children.Add(new TextBlock { Text = L10n.T("chat.empty_title"), FontSize = 16, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        panel.Children.Add(Line(L10n.T("chat.empty_step1")));
        panel.Children.Add(Line(L10n.T("chat.empty_step2")));
        panel.Children.Add(Line(L10n.T("chat.empty_step3")));
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
            Text = L10n.T("studio.thoughts"),
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
        saveContent.Children.Add(new TextBlock { Text = L10n.T("chat.save_to_note"), FontSize = 12, Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"], VerticalAlignment = VerticalAlignment.Center });
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
        ToolTipService.SetToolTip(saveBtn, L10n.T("chat.save_to_note_tip"));
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(saveBtn, L10n.T("chat.save_to_note_tip"));
        saveBtn.Click += async (_, _) =>
        {
            if (await CoreAsync("note.from_message", Args(("message_id", v.Id)), L10n.T("note.op_save")) != null)
            {
                ShowStatus(InfoBarSeverity.Success, L10n.T("chat.saved_title"), L10n.T("chat.saved_msg"));
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
        ToolTipService.SetToolTip(copyBtn, L10n.T("chat.copy_answer"));
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(copyBtn, L10n.T("chat.copy_answer"));
        copyBtn.Click += (_, _) =>
        {
            var dp = new DataPackage();
            dp.SetText(content);
            Clipboard.SetContent(dp);
            ShowStatus(InfoBarSeverity.Success, L10n.T("chat.copied_title"), L10n.T("chat.copied_msg"));
        };
        bar.Children.Add(copyBtn);
        if (isLast)
        {
            bar.Children.Add(Mk(L10n.T("chat.regenerate"), L10n.T("chat.regenerate_tip"), async () =>
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
            feedback > 0 ? L10n.T("chat.liked") : "", feedback > 0, L10n.T("chat.like_tip"), async () =>
        {
            await CoreAsync("chat.set_feedback", Args(("message_id", v.Id), ("feedback", 1)), L10n.T("chat.op_feedback"));
            await RefreshNotebookDataAsync();
        }));
        bar.Children.Add(MkIcon(
            "M19,15H23V3H19M15,3H6C5.17,3 4.46,3.5 4.16,4.22L1.14,11.27C1.05,11.5 1,11.74 1,12V14A2,2 0 0,0 3,16H9.31L8.36,20.57C8.34,20.67 8.33,20.77 8.33,20.88C8.33,21.3 8.5,21.67 8.77,21.94L9.83,23L16.41,16.41C16.78,16.05 17,15.55 17,15V5A2,2 0 0,0 15,3Z",
            feedback < 0 ? L10n.T("chat.disliked") : "", feedback < 0, L10n.T("chat.dislike_tip"), async () =>
        {
            await CoreAsync("chat.set_feedback", Args(("message_id", v.Id), ("feedback", -1)), L10n.T("chat.op_feedback"));
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
        _chatImages?.SetEnabled(!busy);
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
        if (text.Length == 0 && !_chatImages.HasItems) return;
        var images = _chatImages.ToArgs();
        var shown = _chatImages.Snapshot();
        ChatInputBox.Text = string.Empty;
        _chatImages.Clear();
        await SendChatAsync(text, images, shown);
    }

    /// <summary>Adds a one-off attachment of a message to the notebook's sources.</summary>
    private async Task PromoteAttachmentAsync(string attachmentId)
    {
        if (_activeNotebookId == null) return;
        var res = await CoreAsync("attachment.promote", Args(("notebook_id", _activeNotebookId), ("attachment_id", attachmentId)), L10n.T("ctx.promote"));
        if (res != null)
        {
            ShowStatus(InfoBarSeverity.Success, L10n.T("ctx.promote"), L10n.T("ctx.promoted"));
            await RefreshNotebookDataAsync();
        }
    }

    private async void OnStopChatClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId != null) await CoreAsync("chat.cancel", Args(("notebook_id", _activeNotebookId)), L10n.T("chat.stop"));
    }

    private async Task SendChatAsync(string text, JsonElement? images = null, IReadOnlyList<PendingItem>? shown = null)
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
        if (shown is { Count: > 0 }) user.Root.Children.Insert(1, ImageAttachments.PendingGallery(shown, () => Content.XamlRoot));
        ChatList.Items.Add(user.Root);
        _flushedLength = 0;
        _chatStickToBottom = true;
        ScrollChatToBottomIfSticky();
        SetBusy(true);
        try
        {
            var (_, err) = await Api.CallAsync("chat.send",
                ImageAttachments.Merge(Args(("notebook_id", _activeNotebookId), ("text", text)), images));
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
        ShowStatus(InfoBarSeverity.Error, L10n.T("chat.failed_title"), advice.Length > 0 ? $"{advice} ({e.Code})" : $"{e.Message} ({e.Code})");
    }

    private async void OnClearChatClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        if (!await ConfirmAsync(L10n.T("nb.clear_chat"), L10n.T("chat.clear_confirm"), L10n.T("chat.clear_btn"))) return;
        await CoreAsync("chat.clear", Args(("notebook_id", _activeNotebookId)), L10n.T("nb.clear_chat"));
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

    /// <summary>Sanitizes OCR/PDF dot-leader noise and collapses redundant whitespace in citation excerpts.</summary>
    private static string SanitizeCitationExcerpt(string raw)
    {
        if (string.IsNullOrWhiteSpace(raw)) return "";
        var cleaned = System.Text.RegularExpressions.Regex.Replace(raw, @"[·.…•․…]{3,}", " ");
        cleaned = System.Text.RegularExpressions.Regex.Replace(cleaned, @"(\s*[.·]\s*){4,}", " ");
        cleaned = System.Text.RegularExpressions.Regex.Replace(cleaned, @"\s{2,}", " ").Trim();
        return cleaned;
    }

    /// <summary>Detects whether a citation text snippet is unclear (e.g., dot leaders, garbled OCR, figure/image page).</summary>
    private static bool IsCitationExcerptUnclear(string raw)
    {
        if (string.IsNullOrWhiteSpace(raw)) return true;
        var meaningful = 0;
        var dotOrNoise = 0;
        foreach (var ch in raw)
        {
            if (char.IsLetterOrDigit(ch) || ch >= 0x4E00 && ch <= 0x9FFF) meaningful++;
            else if (ch is '.' or '·' or '…' or '•' or '_' or '-') dotOrNoise++;
        }
        if (meaningful < 22) return true;
        var total = meaningful + dotOrNoise;
        if (total > 0 && (double)dotOrNoise / total > 0.32) return true;
        return false;
    }

    /// <summary>
    /// Shows a compact Wikipedia-style Reference Card Flyout anchored to the clicked citation pill.
    /// When the text excerpt is unclear/figure-only on a PDF, opens the PDF directly and jumps to that exact page.
    /// </summary>
    private async Task ShowCitationPopoverAsync(FrameworkElement anchor, string messageId, int no)
    {
        var detail = await CoreAsync("chat.citation_detail", Args(("message_id", messageId), ("citation_no", no)), L10n.T("cite.header"));
        if (detail is not { } d) return;

        var removed = Bool(d, "source_removed");
        var page = Int(d, "page_start");
        var sourceTitle = Str(d, "source_title");
        var sectionPath = Str(d, "section_path");
        var rawContent = Str(d, "content");
        var cleanExcerpt = SanitizeCitationExcerpt(rawContent);
        var isUnclear = IsCitationExcerptUnclear(rawContent);
        var sourceId = !removed ? await FindSourceIdByTitleAsync(sourceTitle) : "";
        var isPdf = sourceTitle.EndsWith(".pdf", StringComparison.OrdinalIgnoreCase);

        // For unclear text or image/figure PDF pages, immediately display the PDF and jump to the exact page!
        if (isUnclear && isPdf && !removed && sourceId.Length > 0 && page > 0)
        {
            await ShowSourcePreviewAsync(sourceId, (int)page, null, sourceTitle);
        }

        var flyout = new Flyout
        {
            Placement = Microsoft.UI.Xaml.Controls.Primitives.FlyoutPlacementMode.BottomEdgeAlignedLeft,
        };

        var card = new StackPanel { Spacing = 10, MinWidth = 330, MaxWidth = 400 };

        // Header row: document icon + "Source Reference" + page pill
        var headRow = new Grid { ColumnSpacing = 8 };
        headRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        headRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        var headLeft = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
        headLeft.Children.Add(new Viewbox
        {
            Width = 16,
            Height = 16,
            Child = new PathIcon
            {
                Width = 24,
                Height = 24,
                Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(
                    typeof(Microsoft.UI.Xaml.Media.Geometry),
                    "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20Z"),
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"],
            },
        });
        headLeft.Children.Add(new TextBlock
        {
            Text = L10n.T("cite.header"),
            FontFamily = Md.BodyFontFamily,
            FontSize = 14,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
            VerticalAlignment = VerticalAlignment.Center,
        });
        headRow.Children.Add(headLeft);

        if (page > 0)
        {
            var pagePill = new Border
            {
                Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["PanelBgBrush"],
                BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["StrongBorderBrush"],
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(10),
                Padding = new Thickness(8, 2, 8, 2),
                Child = new TextBlock
                {
                    Text = L10n.Fmt("cite.page", page),
                    FontFamily = Md.BodyFontFamily,
                    FontSize = 11.5,
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                },
            };
            Grid.SetColumn(pagePill, 1);
            headRow.Children.Add(pagePill);
        }
        card.Children.Add(headRow);

        // Clickable source title link (matches reference card blue link styling)
        var titleLink = new HyperlinkButton
        {
            Content = new TextBlock
            {
                Text = sectionPath.Length > 0 ? $"{sourceTitle} · {sectionPath}" : sourceTitle,
                FontFamily = Md.BodyFontFamily,
                FontSize = 13.5,
                FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                TextWrapping = TextWrapping.Wrap,
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentBlueBrush"],
            },
            Padding = new Thickness(0),
            IsEnabled = !removed && sourceId.Length > 0,
        };
        titleLink.Click += async (_, _) =>
        {
            flyout.Hide();
            if (sourceId.Length > 0)
                await ShowSourcePreviewAsync(sourceId, page > 0 ? (int)page : null, isUnclear ? null : cleanExcerpt, sourceTitle);
        };
        card.Children.Add(titleLink);

        if (removed)
        {
            card.Children.Add(new TextBlock
            {
                Text = L10n.T("cite.removed"),
                FontFamily = Md.BodyFontFamily,
                FontSize = 12,
                TextWrapping = TextWrapping.Wrap,
                Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 180, 120)),
            });
        }

        // Body: either clean excerpt OR clear notice that PDF jumped directly to page N
        var bodyText = isUnclear && isPdf && page > 0
            ? L10n.Fmt("cite.unclear_pdf", page)
            : (cleanExcerpt.Length > 260 ? cleanExcerpt[..260] + "…" : cleanExcerpt);
        if (bodyText.Length > 0)
        {
            card.Children.Add(new TextBlock
            {
                Text = bodyText,
                FontFamily = Md.BodyFontFamily,
                FontSize = 13,
                LineHeight = 22,
                TextWrapping = TextWrapping.Wrap,
                MaxLines = 6,
                TextTrimming = TextTrimming.CharacterEllipsis,
                IsTextSelectionEnabled = true,
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
            });
        }

        // Footer action button to open PDF / source at exact page
        if (!removed && sourceId.Length > 0)
        {
            var jumpBtn = new Button
            {
                Content = page > 0 ? L10n.Fmt("cite.open_page", page) : L10n.T("cite.open_source"),
                Style = (Style)Application.Current.Resources["PillButtonStyle"],
                MinHeight = 30,
                Padding = new Thickness(12, 4, 12, 4),
                FontSize = 12,
                HorizontalAlignment = HorizontalAlignment.Right,
            };
            jumpBtn.Click += async (_, _) =>
            {
                flyout.Hide();
                await ShowSourcePreviewAsync(sourceId, page > 0 ? (int)page : null, isUnclear ? null : cleanExcerpt, sourceTitle);
            };
            card.Children.Add(jumpBtn);
        }

        flyout.Content = card;
        flyout.ShowAt(anchor);
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
