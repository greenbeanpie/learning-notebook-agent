using LocalNotebook.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Text.Json;
using Windows.Storage.Pickers;
using Windows.System;
using WinRT.Interop;

namespace LocalNotebook.Views;

public sealed partial class MainWindow : Window
{
    private static readonly string[] Protocols = { "chat_completion", "responses", "anthropic", "gemini" };

    private string? _activeNotebookId;
    private bool _loadingPicker;
    private bool _refreshing;
    private bool _refreshAgain;

    public MainWindow()
    {
        InitializeComponent();
        App.Core.EventReceived += OnCoreEvent;
        Closed += (_, _) => App.Core.Dispose();
    }

    // ---------- helpers ----------

    /// <summary>Builds a JSON object without reflection (NativeAOT safe).</summary>
    private static JsonElement Args(params (string Key, object? Value)[] kv)
    {
        using var ms = new MemoryStream();
        using (var w = new Utf8JsonWriter(ms))
        {
            w.WriteStartObject();
            foreach (var (k, v) in kv)
            {
                switch (v)
                {
                    case null: w.WriteNull(k); break;
                    case string s: w.WriteString(k, s); break;
                    case bool b: w.WriteBoolean(k, b); break;
                    case int i: w.WriteNumber(k, i); break;
                    case string[] arr:
                        w.WriteStartArray(k);
                        foreach (var a in arr) w.WriteStringValue(a);
                        w.WriteEndArray();
                        break;
                    case JsonElement je:
                        w.WritePropertyName(k);
                        je.WriteTo(w);
                        break;
                    default: throw new ArgumentException($"unsupported arg type for {k}");
                }
            }
            w.WriteEndObject();
        }
        using var doc = JsonDocument.Parse(ms.ToArray());
        return doc.RootElement.Clone();
    }

    private void ShowStatus(InfoBarSeverity severity, string title, string message)
    {
        StatusBar.Severity = severity;
        StatusBar.Title = title;
        StatusBar.Message = message;
        StatusBar.IsOpen = true;
    }

    /// <summary>Calls the core; any error is shown in the status bar instead of being swallowed.</summary>
    private async Task<JsonElement?> CoreAsync(string method, JsonElement args, string what)
    {
        var resp = await App.Core.CallAsync(method, args);
        if (resp.Error is { } err)
        {
            ShowStatus(InfoBarSeverity.Error, $"{what}失败", $"{err.Message}（{err.Code}）");
            return null;
        }
        return resp.Result ?? Args();
    }

    private static string Str(JsonElement e, string name) =>
        e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() ?? "" : "";

    private static TextBlock Line(string text) => new()
    {
        Text = text,
        TextWrapping = TextWrapping.Wrap,
        IsTextSelectionEnabled = true,
    };

    // ---------- loading ----------

    public void ReportStartupFailure(string message) =>
        ShowStatus(InfoBarSeverity.Error, "核心启动失败", message);

    public async Task OnCoreReadyAsync()
    {
        var list = await CoreAsync("notebook.list", Args(), "读取笔记本");
        if (list is { } res && res.TryGetProperty("notebooks", out var arr) && arr.GetArrayLength() > 0)
        {
            var first = arr[0];
            _activeNotebookId = Str(first, "id");
            NotebookTitleText.Text = Str(first, "title") is { Length: > 0 } t ? t : "未命名笔记本";
        }
        else
        {
            var created = await CoreAsync("notebook.create", Args(("title", "未命名笔记本")), "创建笔记本");
            if (created is { } cr)
            {
                _activeNotebookId = Str(cr, "id");
                NotebookTitleText.Text = Str(cr, "title");
            }
        }
        await RefreshNotebookPickerAsync();
        await RefreshNotebookDataAsync();
        await WarnIfNoModelAsync();
    }

    private async Task WarnIfNoModelAsync()
    {
        var profiles = await CoreAsync("profile.list", Args(), "读取模型配置");
        if (profiles is { } p && p.TryGetProperty("profiles", out var arr))
        {
            var ok = false;
            foreach (var prof in arr.EnumerateArray())
            {
                if (prof.TryGetProperty("is_active", out var a) && a.ValueKind == JsonValueKind.True) ok = true;
            }
            if (!ok)
            {
                ShowStatus(InfoBarSeverity.Warning, "尚未配置模型", "请点击右上角“模型协议配置”，填写协议、地址、模型与 API Key。");
            }
            else if (StatusBar.Title == "尚未配置模型")
            {
                StatusBar.IsOpen = false; // configured now; don't leave the stale warning up
            }
        }
    }

    private async Task RefreshNotebookPickerAsync()
    {
        var list = await CoreAsync("notebook.list", Args(), "读取笔记本");
        if (list is not { } res || !res.TryGetProperty("notebooks", out var arr)) return;
        _loadingPicker = true;
        try
        {
            NotebookPicker.Items.Clear();
            foreach (var nb in arr.EnumerateArray())
            {
                var id = Str(nb, "id");
                var item = new ComboBoxItem { Content = $"{Str(nb, "emoji")} {Str(nb, "title")}".Trim(), Tag = id };
                NotebookPicker.Items.Add(item);
                if (id == _activeNotebookId) NotebookPicker.SelectedItem = item;
            }
        }
        finally
        {
            _loadingPicker = false;
        }
    }

    private async void OnNotebookPicked(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingPicker || NotebookPicker.SelectedItem is not ComboBoxItem item || item.Tag is not string id) return;
        if (id == _activeNotebookId) return;
        _activeNotebookId = id;
        var nb = await CoreAsync("notebook.get", Args(("notebook_id", id)), "读取笔记本");
        NotebookTitleText.Text = nb is { } n && Str(n, "title") is { Length: > 0 } t ? t : "未命名笔记本";
        await RefreshNotebookDataAsync();
    }

    private async Task RefreshNotebookDataAsync()
    {
        if (_activeNotebookId == null) return;
        // Events arrive in bursts; coalesce so only one refresh runs at a time and the last one wins.
        if (_refreshing)
        {
            _refreshAgain = true;
            return;
        }
        _refreshing = true;
        try
        {
            do
            {
                _refreshAgain = false;
                await RefreshOnceAsync();
            } while (_refreshAgain);
        }
        finally
        {
            _refreshing = false;
        }
    }

    private async Task RefreshOnceAsync()
    {
        var nid = _activeNotebookId!;
        var param = Args(("notebook_id", nid));

        var summary = await CoreAsync("notebook.get", param, "读取笔记本");
        if (summary is { } sm)
        {
            var md = Str(sm, "summary_md");
            SummaryText.Text = md.Length > 0 ? md : "请添加来源或直接提问。";
        }

        var src = await CoreAsync("source.list", param, "读取来源");
        SourcesList.Items.Clear();
        if (src is { } sr && sr.TryGetProperty("sources", out var sArr))
        {
            foreach (var s in sArr.EnumerateArray())
            {
                var id = Str(s, "id");
                var status = Str(s, "status");
                var failure = Str(s, "error_detail");
                var cb = new CheckBox
                {
                    Content = new TextBlock
                    {
                        Text = failure.Length > 0
                            ? $"{Str(s, "title")} [{status}] — {failure}"
                            : $"{Str(s, "title")} [{status}]",
                        TextWrapping = TextWrapping.Wrap,
                    },
                    IsChecked = s.TryGetProperty("is_selected", out var sel) && sel.ValueKind == JsonValueKind.True,
                    Tag = id,
                };
                cb.Checked += OnSourceCheckChanged;
                cb.Unchecked += OnSourceCheckChanged;
                var menu = new MenuFlyout();
                if (status == "FAILED")
                {
                    var retry = new MenuFlyoutItem { Text = "重试导入", Tag = id };
                    retry.Click += OnRetrySourceClick;
                    menu.Items.Add(retry);
                }
                var del = new MenuFlyoutItem { Text = "删除来源", Tag = id };
                del.Click += OnDeleteSourceClick;
                menu.Items.Add(del);
                // A visible "⋯" button opens the menu: right-click alone is hard to discover.
                var more = new Button
                {
                    Content = "⋯",
                    Flyout = menu,
                    Padding = new Thickness(8, 2, 8, 2),
                    VerticalAlignment = VerticalAlignment.Top,
                };
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(more, "更多操作");
                var row = new Grid { ColumnSpacing = 4 };
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                Grid.SetColumn(more, 1);
                row.Children.Add(cb);
                row.Children.Add(more);
                SourcesList.Items.Add(row);
            }
        }

        var notes = await CoreAsync("note.list", param, "读取笔记");
        if (notes is { } nr && nr.TryGetProperty("notes", out var nArr))
        {
            foreach (var n in nArr.EnumerateArray())
            {
                SourcesList.Items.Add(Line($"📝 {Str(n, "title")}"));
            }
        }

        var chat = await CoreAsync("chat.history", param, "读取对话");
        ChatList.Items.Clear();
        if (chat is { } cr && cr.TryGetProperty("messages", out var mArr))
        {
            foreach (var m in mArr.EnumerateArray())
            {
                ChatList.Items.Add(Line($"{Str(m, "role")}: {Str(m, "content")}"));
            }
            if (ChatList.Items.Count > 0) ChatList.ScrollIntoView(ChatList.Items[^1]);
        }

        var art = await CoreAsync("studio.list", param, "读取 Studio 产物");
        ArtifactsList.Items.Clear();
        if (art is { } ar && ar.TryGetProperty("artifacts", out var aArr))
        {
            foreach (var a in aArr.EnumerateArray())
            {
                var line = $"{Str(a, "title")} ({Str(a, "status")})";
                var err = Str(a, "error_detail");
                if (err.Length > 0) line += $" — {err}";
                ArtifactsList.Items.Add(Line(line));
            }
        }
    }

    private void OnCoreEvent(ApiEventDto ev)
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (ev.Event is "source.status" or "studio.status")
            {
                await RefreshNotebookDataAsync();
            }
        });
    }

    // ---------- notebook / sources / notes ----------

    private async void OnNewNotebookClick(object sender, RoutedEventArgs e)
    {
        var res = await CoreAsync("notebook.create", Args(("title", "新笔记本")), "新建笔记本");
        if (res is { } r)
        {
            _activeNotebookId = Str(r, "id");
            NotebookTitleText.Text = Str(r, "title") is { Length: > 0 } t ? t : "新笔记本";
            await RefreshNotebookPickerAsync();
            await RefreshNotebookDataAsync();
        }
    }

    private async void OnAddSourceClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var picker = new FileOpenPicker();
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        picker.FileTypeFilter.Add(".pdf");
        picker.FileTypeFilter.Add(".md");
        picker.FileTypeFilter.Add(".txt");
        var file = await picker.PickSingleFileAsync();
        if (file == null) return;

        await CoreAsync("source.add_files", Args(("notebook_id", _activeNotebookId), ("paths", new[] { file.Path })), "添加来源");
        await RefreshNotebookDataAsync();
    }

    private async void OnSourceCheckChanged(object sender, RoutedEventArgs e)
    {
        if (sender is not CheckBox cb || cb.Tag is not string id) return;
        await CoreAsync("source.set_selected", Args(("source_id", id), ("selected", cb.IsChecked == true)), "更新来源选择");
    }

    private async void OnRetrySourceClick(object sender, RoutedEventArgs e)
    {
        if (sender is not MenuFlyoutItem mi || mi.Tag is not string id) return;
        await CoreAsync("source.retry", Args(("source_id", id)), "重试导入");
        await RefreshNotebookDataAsync();
    }

    private async void OnDeleteSourceClick(object sender, RoutedEventArgs e)
    {
        if (sender is not MenuFlyoutItem mi || mi.Tag is not string id) return;
        await CoreAsync("source.remove", Args(("source_id", id)), "删除来源");
        await RefreshNotebookDataAsync();
    }

    private async void OnCreateNoteClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        await CoreAsync("note.save", Args(("notebook_id", _activeNotebookId), ("title", "新笔记"), ("body_md", "")), "创建笔记");
        await RefreshNotebookDataAsync();
    }

    // ---------- chat / studio ----------

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
        if (_activeNotebookId == null || string.IsNullOrWhiteSpace(ChatInputBox.Text)) return;
        var text = ChatInputBox.Text.Trim();
        ChatInputBox.Text = string.Empty;
        ChatList.Items.Add(Line($"user: {text}"));
        SendBtn.IsEnabled = false;
        try
        {
            await CoreAsync("chat.send", Args(("notebook_id", _activeNotebookId), ("text", text)), "发送消息");
        }
        finally
        {
            SendBtn.IsEnabled = true;
        }
        await RefreshNotebookDataAsync();
    }

    private async void OnStudioTileClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null || sender is not Button btn || btn.Tag is not string kind) return;
        var res = await CoreAsync("studio.create", Args(("notebook_id", _activeNotebookId), ("artifact_type", kind), ("config", Args())), $"生成{btn.Content}");
        if (res != null) ShowStatus(InfoBarSeverity.Success, "已提交", $"{btn.Content}生成任务已加入队列。");
        await RefreshNotebookDataAsync();
    }

    // ---------- model settings (profile.* / secret.*) ----------

    private async void OnOpenSettingsClick(object sender, RoutedEventArgs e)
    {
        string? profileId = null;
        string keychainRef = "";
        string protocol = Protocols[0], baseUrl = "", model = "";
        var hasSecret = false;

        var list = await CoreAsync("profile.list", Args(), "读取模型配置");
        if (list is { } lr && lr.TryGetProperty("profiles", out var arr))
        {
            foreach (var p in arr.EnumerateArray())
            {
                if (p.TryGetProperty("is_active", out var a) && a.ValueKind == JsonValueKind.True)
                {
                    profileId = Str(p, "id");
                    keychainRef = Str(p, "keychain_ref");
                    protocol = Str(p, "protocol");
                    baseUrl = Str(p, "base_url");
                    model = Str(p, "model_id");
                    hasSecret = p.TryGetProperty("has_secret", out var hs) && hs.ValueKind == JsonValueKind.True;
                    break;
                }
            }
        }

        var protocolBox = new ComboBox { Header = "协议", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var name in Protocols)
        {
            var item = new ComboBoxItem { Content = name, Tag = name };
            protocolBox.Items.Add(item);
            if (name == protocol) protocolBox.SelectedItem = item;
        }
        if (protocolBox.SelectedItem == null) protocolBox.SelectedIndex = 0;
        var urlBox = new TextBox { Header = "接口地址 (Base URL)", Text = baseUrl, PlaceholderText = "https://api.example.com/v1" };
        var modelBox = new TextBox { Header = "模型 ID", Text = model };
        var keyBox = new PasswordBox
        {
            Header = "API Key",
            PlaceholderText = hasSecret ? "已保存，留空则保持不变" : "输入 API Key（本地模型可留空）",
        };
        var status = new TextBlock
        {
            TextWrapping = TextWrapping.Wrap,
            Text = profileId == null
                ? "尚未配置模型。"
                : hasSecret ? "密钥状态：已保存在 Windows 凭据管理器。" : "密钥状态：未保存。",
        };

        var panel = new StackPanel { Spacing = 10, MinWidth = 440 };
        panel.Children.Add(protocolBox);
        panel.Children.Add(urlBox);
        panel.Children.Add(modelBox);
        panel.Children.Add(keyBox);
        panel.Children.Add(status);

        var dialog = new ContentDialog
        {
            Title = "模型协议配置",
            Content = panel,
            PrimaryButtonText = "保存并测试",
            SecondaryButtonText = "清除密钥",
            CloseButtonText = "关闭",
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = Content.XamlRoot,
        };

        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true; // keep the dialog open to show the result
            var deferral = args.GetDeferral();
            try
            {
                var url = urlBox.Text.Trim();
                var modelId = modelBox.Text.Trim();
                if (url.Length == 0 || modelId.Length == 0)
                {
                    status.Text = "请填写接口地址和模型 ID。";
                    return;
                }
                var proto = (protocolBox.SelectedItem as ComboBoxItem)?.Tag as string ?? Protocols[0];

                var up = new List<(string, object?)>
                {
                    ("display_name", $"{proto} / {modelId}"),
                    ("protocol", proto),
                    ("base_url", url),
                    ("model_id", modelId),
                    ("activate", true),
                };
                if (profileId != null) up.Add(("id", profileId));
                var saved = await CoreAsync("profile.upsert", Args(up.ToArray()), "保存模型配置");
                if (saved is not { } sv) { status.Text = "保存失败，详见底部提示。"; return; }
                profileId = Str(sv, "id");
                keychainRef = Str(sv, "keychain_ref");

                if (keyBox.Password.Length > 0)
                {
                    var set = await CoreAsync("secret.set", Args(("keychain_ref", keychainRef), ("secret", keyBox.Password)), "保存 API Key");
                    if (set == null) { status.Text = "API Key 保存失败，详见底部提示。"; return; }
                    keyBox.Password = string.Empty;
                }
                var ex = await CoreAsync("secret.exists", Args(("keychain_ref", keychainRef)), "检查 API Key");
                hasSecret = ex is { } er && er.TryGetProperty("exists", out var ev) && ev.ValueKind == JsonValueKind.True;
                keyBox.PlaceholderText = hasSecret ? "已保存，留空则保持不变" : "输入 API Key（本地模型可留空）";

                await CoreAsync("profile.activate", Args(("profile_id", profileId)), "启用模型配置");
                status.Text = (hasSecret ? "密钥状态：已保存在 Windows 凭据管理器。" : "密钥状态：未保存。") + " 正在测试连接…";
                var test = await CoreAsync("profile.test", Args(("profile_id", profileId)), "测试连接");
                var mark = hasSecret ? "密钥状态：已保存在 Windows 凭据管理器。" : "密钥状态：未保存。";
                status.Text = test is { } tr
                    ? $"{mark} 连接成功，模型回复：{Str(tr, "reply")}"
                    : $"{mark} 连接测试失败，详见底部提示。";
            }
            finally
            {
                deferral.Complete();
            }
        };

        dialog.SecondaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            var deferral = args.GetDeferral();
            try
            {
                if (profileId == null || keychainRef.Length == 0) { status.Text = "没有可清除的密钥。"; return; }
                var cleared = await CoreAsync("secret.clear", Args(("keychain_ref", keychainRef)), "清除 API Key");
                if (cleared == null) { status.Text = "清除失败，详见底部提示。"; return; }
                hasSecret = false;
                keyBox.PlaceholderText = "输入 API Key（本地模型可留空）";
                status.Text = "密钥状态：已清除。";
            }
            finally
            {
                deferral.Complete();
            }
        };

        await dialog.ShowAsync();
        await WarnIfNoModelAsync();
    }
}
