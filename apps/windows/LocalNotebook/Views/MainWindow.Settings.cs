using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

public sealed partial class MainWindow
{
    private static JsonElement HeadersJson(string text)
    {
        var kv = new List<(string, object?)>();
        foreach (var raw in text.Split('\n', StringSplitOptions.RemoveEmptyEntries))
        {
            var line = raw.Trim();
            var i = line.IndexOf(':');
            if (i > 0) kv.Add((line[..i].Trim(), line[(i + 1)..].Trim()));
        }
        return Args(kv.ToArray());
    }

    private static string HeadersText(JsonElement obj)
    {
        if (obj.ValueKind != JsonValueKind.Object) return "";
        return string.Join("\n", obj.EnumerateObject().Select(p => $"{p.Name}: {p.Value.GetString()}"));
    }

    // ---------- model settings (profile.* / secret.*) ----------

    private async void OnOpenSettingsClick(object sender, RoutedEventArgs e)
    {
        var list = await CoreAsync("profile.list", Args(), "读取模型配置");
        var profiles = list is { } lr ? Arr(lr, "profiles").ToList() : new List<JsonElement>();
        var avEnabled = true;
        var mediaSet = await Api.CallAsync("settings.get", Args(("key", "media_processing")));
        if (mediaSet.Result is { } ms && ms.TryGetProperty("value", out var mv) && mv.ValueKind == JsonValueKind.Object)
        {
            if (mv.TryGetProperty("audio_video_enabled", out var avp) && avp.ValueKind == JsonValueKind.False)
                avEnabled = false;
        }

        string? profileId = null;
        var keychainRef = "";
        var hasSecret = false;

        var picker = new ComboBox { Header = "已保存的配置", HorizontalAlignment = HorizontalAlignment.Stretch };
        var protocolBox = new ComboBox { Header = "协议", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var name in Protocols) protocolBox.Items.Add(new ComboBoxItem { Content = name, Tag = name });
        var urlBox = new TextBox { Header = "接口地址 (Base URL)", PlaceholderText = "https://api.example.com/v1" };
        var modelBox = new ComboBox { Header = "模型 ID", IsEditable = true, HorizontalAlignment = HorizontalAlignment.Stretch };
        var discoverBtn = new Button { Content = "获取模型列表" };
        var keyBox = new PasswordBox { Header = "API Key" };
        var allowInsecureHttpBox = new CheckBox { Content = "确认允许非回环 HTTP 明文传输（仅用于可信本地网络）" };
        var activateChatBox = new CheckBox { Content = "设为当前默认对话模型", IsChecked = true };
        var visionBox = new CheckBox { Content = "用于处理图文（图片 / 扫描 PDF / 图文混合视觉读取）" };
        var audioBox = new CheckBox { Content = "用于处理音频" };
        var videoBox = new CheckBox { Content = "用于处理视频" };
        var audioVideoToggle = new ToggleSwitch { Header = "允许处理音视频文件（直接交给原生支持音视频的 AI 处理）", IsOn = avEnabled };
        var mediaRoutingSummary = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.8 };
        var thinkingBox = new ToggleSwitch { Header = "显示模型的思考过程（如果模型提供）", IsOn = true };
        var effortBox = new ComboBox { Header = "推理强度", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var (label, tag) in new[] { ("默认", ""), ("low", "low"), ("medium", "medium"), ("high", "high") }) effortBox.Items.Add(new ComboBoxItem { Content = label, Tag = tag });
        var ctxBox = new NumberBox { Header = "上下文窗口 (tokens)", Value = 32000, Minimum = 2000, Maximum = 2_000_000, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        var outBox = new NumberBox { Header = "单次最大输出 (tokens)", Value = 8000, Minimum = 256, Maximum = 200_000, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        var priceIn = new NumberBox { Header = "输入单价 ($ / 百万 tokens，可留空)", Value = double.NaN, Minimum = 0 };
        var priceOut = new NumberBox { Header = "输出单价 ($ / 百万 tokens，可留空)", Value = double.NaN, Minimum = 0 };
        var headersBox = new TextBox { Header = "额外请求头（每行 名称: 值；例如 anthropic-version: 2023-06-01）", AcceptsReturn = true, MinHeight = 60, TextWrapping = TextWrapping.Wrap };
        var caps = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.8 };
        var status = new TextBlock { TextWrapping = TextWrapping.Wrap };
        var newBtn = new Button { Content = "新建配置" };
        var deleteBtn = new Button { Content = "删除此配置" };

        void UpdateMediaSummary()
        {
            var chatModel = profiles.FirstOrDefault(p => Bool(p, "is_active"));
            string PickMod(string capKey, string emptyLabel)
            {
                if (chatModel.ValueKind == JsonValueKind.Object && Bool(Obj(chatModel, "capabilities"), capKey))
                    return Str(chatModel, "display_name");
                var m = profiles.FirstOrDefault(p => Bool(Obj(p, "capabilities"), capKey));
                return m.ValueKind == JsonValueKind.Object ? Str(m, "display_name") : emptyLabel;
            }
            var chatName = chatModel.ValueKind == JsonValueKind.Object ? Str(chatModel, "display_name") : "未配置";
            mediaRoutingSummary.Text =
                $"当前多模型分配：\n" +
                $"• 默认对话：{chatName}\n" +
                $"• 图文处理：{PickMod("supports_vision", "未指定（将尝试本地 OCR）")}\n" +
                $"• 音频处理：{(audioVideoToggle.IsOn ? PickMod("supports_audio", "未指定（请在模型配置中勾选）") : "已关闭")}\n" +
                $"• 视频处理：{(audioVideoToggle.IsOn ? PickMod("supports_video", "未指定（请在模型配置中勾选）") : "已关闭")}";
        }

        void Fill(JsonElement? p)
        {
            if (p is not { } prof)
            {
                profileId = null; keychainRef = ""; hasSecret = false;
                protocolBox.SelectedIndex = 0;
                urlBox.Text = ""; modelBox.Items.Clear(); modelBox.Text = "";
                activateChatBox.IsChecked = !profiles.Any(x => Bool(x, "is_active"));
                visionBox.IsChecked = false; audioBox.IsChecked = false; videoBox.IsChecked = false;
                thinkingBox.IsOn = true; effortBox.SelectedIndex = 0;
                ctxBox.Value = 32000; outBox.Value = 8000; priceIn.Value = double.NaN; priceOut.Value = double.NaN;
                headersBox.Text = ""; caps.Text = "";
                keyBox.Password = "";
                keyBox.PlaceholderText = "输入 API Key（本地模型可留空）";
                status.Text = "正在编辑新配置，填写并保存即可启用。";
                return;
            }
            profileId = Str(prof, "id");
            keychainRef = Str(prof, "keychain_ref");
            hasSecret = Bool(prof, "has_secret");
            var proto = Str(prof, "protocol");
            protocolBox.SelectedIndex = Math.Max(0, Array.IndexOf(Protocols, proto));
            urlBox.Text = Str(prof, "base_url");
            var savedModel = Str(prof, "model_id");
            modelBox.Items.Clear();
            if (savedModel.Length > 0) { modelBox.Items.Add(savedModel); modelBox.SelectedIndex = 0; }
            modelBox.Text = savedModel;
            activateChatBox.IsChecked = Bool(prof, "is_active");
            thinkingBox.IsOn = !prof.TryGetProperty("show_thinking", out var st) || st.ValueKind != JsonValueKind.False;
            var effort = Str(prof, "effort");
            effortBox.SelectedIndex = Math.Max(0, new[] { "", "low", "medium", "high" }.ToList().IndexOf(effort));
            ctxBox.Value = Int(prof, "context_window", 32000);
            outBox.Value = Int(prof, "max_output_tokens", 8000);
            priceIn.Value = prof.TryGetProperty("input_price_per_m", out var pi) && pi.ValueKind == JsonValueKind.Number ? pi.GetDouble() : double.NaN;
            priceOut.Value = prof.TryGetProperty("output_price_per_m", out var po) && po.ValueKind == JsonValueKind.Number ? po.GetDouble() : double.NaN;
            headersBox.Text = HeadersText(Obj(prof, "extra_headers"));
            keyBox.Password = "";
            keyBox.PlaceholderText = hasSecret ? "已保存，留空则保持不变" : "输入 API Key（本地模型可留空）";
            var c = Obj(prof, "capabilities");
            visionBox.IsChecked = c.ValueKind == JsonValueKind.Object && Bool(c, "supports_vision");
            audioBox.IsChecked = c.ValueKind == JsonValueKind.Object && Bool(c, "supports_audio");
            videoBox.IsChecked = c.ValueKind == JsonValueKind.Object && Bool(c, "supports_video");
            caps.Text = c.ValueKind == JsonValueKind.Object
                ? $"已探测的能力：工具调用 {(Bool(c, "tools") ? "支持" : "不支持")} · JSON Schema {(Bool(c, "json_schema") ? "支持" : "不支持")} · JSON 模式 {(Bool(c, "json_mode") ? "支持" : "不支持")} · 流式用量 {(Bool(c, "stream_usage") ? "支持" : "不支持")}"
                : "";
            status.Text = hasSecret ? "密钥状态：已保存在 Windows 凭据管理器。" : "密钥状态：未保存。";
        }

        void LoadPicker(string? select)
        {
            picker.Items.Clear();
            foreach (var p in profiles)
            {
                var c = Obj(p, "capabilities");
                var mods = new List<string>();
                if (Bool(c, "supports_vision")) mods.Add("图文");
                if (Bool(c, "supports_audio")) mods.Add("音频");
                if (Bool(c, "supports_video")) mods.Add("视频");
                var modTag = mods.Count > 0 ? $" [{string.Join("/", mods)}]" : "";
                var item = new ComboBoxItem { Content = $"{Str(p, "display_name")}{(Bool(p, "is_active") ? "（默认对话）" : "")}{modTag}", Tag = Str(p, "id") };
                picker.Items.Add(item);
                if (Str(p, "id") == select) picker.SelectedItem = item;
            }
            UpdateMediaSummary();
        }
        var active = profiles.FirstOrDefault(p => Bool(p, "is_active"));
        var initial = active.ValueKind == JsonValueKind.Object ? active : (profiles.Count > 0 ? profiles[0] : default);
        LoadPicker(initial.ValueKind == JsonValueKind.Object ? Str(initial, "id") : null);
        Fill(initial.ValueKind == JsonValueKind.Object ? initial : null);
        picker.SelectionChanged += (_, _) =>
        {
            if (picker.SelectedItem is ComboBoxItem { Tag: string id })
            {
                var p = profiles.FirstOrDefault(x => Str(x, "id") == id);
                if (p.ValueKind == JsonValueKind.Object) Fill(p);
            }
        };
        newBtn.Click += (_, _) => { picker.SelectedItem = null; Fill(null); };
        deleteBtn.Click += async (_, _) =>
        {
            if (profileId == null) return;
            if (await CoreAsync("profile.delete", Args(("profile_id", profileId)), "删除配置") == null) return;
            profiles.RemoveAll(x => Str(x, "id") == profileId);
            LoadPicker(null);
            Fill(null);
            status.Text = "配置已删除。";
        };
        discoverBtn.Click += async (_, _) =>
        {
            var baseUrl = urlBox.Text.Trim();
            if (string.IsNullOrEmpty(baseUrl))
            {
                status.Text = "请先填写接口地址 (Base URL)。";
                return;
            }
            var protocol = (protocolBox.SelectedItem as ComboBoxItem)?.Tag as string ?? Protocols[0];
            var apiKey = keyBox.Password.Trim();
            var res = await CoreAsync(
                "profile.discover_models",
                Args(
                    ("profile_id", profileId ?? ""),
                    ("base_url", baseUrl),
                    ("protocol", protocol),
                    ("api_key", apiKey)
                ),
                "获取模型列表"
            );
            if (res is not { } r) return;
            var current = modelBox.Text;
            modelBox.Items.Clear();
            foreach (var m in Arr(r, "models")) modelBox.Items.Add(Str(m, "id"));
            modelBox.Text = current;
            status.Text = $"找到 {modelBox.Items.Count} 个模型，可在“模型 ID”下拉框中选择。";
        };

        var modelRow = new Grid { ColumnSpacing = 8 };
        modelRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        modelRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        discoverBtn.VerticalAlignment = VerticalAlignment.Bottom;
        Grid.SetColumn(discoverBtn, 1);
        modelRow.Children.Add(modelBox);
        modelRow.Children.Add(discoverBtn);

        var top = new Grid { ColumnSpacing = 8 };
        top.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        top.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        top.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        newBtn.VerticalAlignment = deleteBtn.VerticalAlignment = VerticalAlignment.Bottom;
        Grid.SetColumn(newBtn, 1);
        Grid.SetColumn(deleteBtn, 2);
        top.Children.Add(picker);
        top.Children.Add(newBtn);
        top.Children.Add(deleteBtn);

        var advanced = new StackPanel { Spacing = 10 };
        advanced.Children.Add(thinkingBox);
        advanced.Children.Add(effortBox);
        advanced.Children.Add(ctxBox);
        advanced.Children.Add(outBox);
        advanced.Children.Add(priceIn);
        advanced.Children.Add(priceOut);
        advanced.Children.Add(headersBox);
        advanced.Children.Add(caps);

        var testBtn = new Button { Content = "测试连接" };
        testBtn.Click += async (_, _) =>
        {
            if (profileId == null)
            {
                status.Text = "请先点击“保存”保存配置，再测试连接。";
                return;
            }
            status.Text = "正在测试连接并探测模型能力…";
            var test = await CoreAsync("profile.test", Args(("profile_id", profileId)), "测试连接");
            if (test is { } tr)
            {
                status.Text = $"连接成功，模型回复：{Str(tr, "reply")}";
                var c = Obj(tr, "capabilities");
                caps.Text = $"已探测的能力：工具调用 {(Bool(c, "tools") ? "支持" : "不支持")} · JSON Schema {(Bool(c, "json_schema") ? "支持" : "不支持")} · JSON 模式 {(Bool(c, "json_mode") ? "支持" : "不支持")} · 流式用量 {(Bool(c, "stream_usage") ? "支持" : "不支持")}";
            }
            else
            {
                status.Text = "连接测试失败，详见底部提示。";
            }
        };

        audioVideoToggle.Toggled += async (_, _) =>
        {
            await Api.CallAsync("settings.set", Args(("key", "media_processing"), ("value", Args(("audio_video_enabled", audioVideoToggle.IsOn)))));
            UpdateMediaSummary();
        };

        var modalityBox = new StackPanel { Spacing = 4 };
        modalityBox.Children.Add(new TextBlock { Text = "此模型处理的模态（可配置多个模型分别负责不同模态）", FontSize = 12, Opacity = 0.85 });
        modalityBox.Children.Add(visionBox);
        modalityBox.Children.Add(audioBox);
        modalityBox.Children.Add(videoBox);

        var mediaSettingsBox = new StackPanel { Spacing = 8, Padding = new Thickness(10), CornerRadius = new CornerRadius(8), Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"] };
        mediaSettingsBox.Children.Add(new TextBlock { Text = "文件与媒体处理设置", FontSize = 13, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        mediaSettingsBox.Children.Add(audioVideoToggle);
        mediaSettingsBox.Children.Add(mediaRoutingSummary);

        var panel = new StackPanel { Spacing = 10, MinWidth = 480 };
        panel.Children.Add(top);
        panel.Children.Add(protocolBox);
        panel.Children.Add(urlBox);
        panel.Children.Add(modelRow);
        panel.Children.Add(keyBox);
        panel.Children.Add(allowInsecureHttpBox);
        panel.Children.Add(activateChatBox);
        panel.Children.Add(modalityBox);
        panel.Children.Add(new Expander { Header = "高级选项", Content = advanced, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch });
        panel.Children.Add(mediaSettingsBox);
        panel.Children.Add(testBtn);
        panel.Children.Add(status);

        var dialog = new ContentDialog
        {
            Title = "模型协议配置",
            Content = new ScrollViewer { Content = panel, MaxHeight = 620 },
            PrimaryButtonText = "保存",
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
                var modelId = (string.IsNullOrWhiteSpace(modelBox.Text) ? modelBox.SelectedItem as string : modelBox.Text)?.Trim() ?? "";
                if (url.Length == 0 || modelId.Length == 0)
                {
                    status.Text = "请填写接口地址和模型 ID。";
                    return;
                }
                var proto = (protocolBox.SelectedItem as ComboBoxItem)?.Tag as string ?? Protocols[0];
                var makeActive = activateChatBox.IsChecked == true;
                var up = new List<(string, object?)>
                {
                    ("display_name", $"{proto} / {modelId}"),
                    ("protocol", proto),
                    ("base_url", url),
                    ("model_id", modelId),
                    ("activate", makeActive),
                    ("allow_insecure_http", allowInsecureHttpBox.IsChecked == true),
                    ("show_thinking", thinkingBox.IsOn),
                    ("context_window", (int)ctxBox.Value),
                    ("max_output_tokens", (int)outBox.Value),
                    ("extra_headers", HeadersJson(headersBox.Text)),
                    ("supports_vision", visionBox.IsChecked == true),
                    ("supports_audio", audioBox.IsChecked == true),
                    ("supports_video", videoBox.IsChecked == true),
                };
                var effort = (effortBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "";
                if (effort.Length > 0) up.Add(("effort", effort));
                if (!double.IsNaN(priceIn.Value)) up.Add(("input_price_per_m", priceIn.Value));
                if (!double.IsNaN(priceOut.Value)) up.Add(("output_price_per_m", priceOut.Value));
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
                hasSecret = ex is { } er && Bool(er, "exists");
                keyBox.PlaceholderText = hasSecret ? "已保存，留空则保持不变" : "输入 API Key（本地模型可留空）";

                if (makeActive)
                    await CoreAsync("profile.activate", Args(("profile_id", profileId)), "启用模型配置");
                await Api.CallAsync("settings.set", Args(("key", "media_processing"), ("value", Args(("audio_video_enabled", audioVideoToggle.IsOn)))));
                var mark = hasSecret ? "密钥状态：已保存在 Windows 凭据管理器。" : "密钥状态：未保存。";
                status.Text = makeActive ? $"配置已保存并设为默认对话模型。{mark}" : $"配置已保存。{mark}";
                var relist = await CoreAsync("profile.list", Args(), "读取模型配置");
                if (relist is { } rl)
                {
                    profiles = Arr(rl, "profiles").ToList();
                    LoadPicker(profileId);
                }
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
        await RefreshNotebookDataAsync();
    }

    // ---------- privacy, onboarding ----------

    private static bool IsLocalEndpoint(string url) =>
        Uri.TryCreate(url, UriKind.Absolute, out var u) && (u.IsLoopback || u.Host == "localhost");

    /// <summary>Asks once before notebook content is sent to a model that is not on this machine.</summary>
    private async Task<bool> EnsurePrivacyAckAsync()
    {
        var ack = await Api.CallAsync("settings.get", Args(("key", "privacy.cloud_ack")));
        if (ack.Result is { } a && a.TryGetProperty("value", out var v) && v.ValueKind == JsonValueKind.True) return true;
        var list = await Api.CallAsync("profile.list", Args());
        var active = list.Result is { } l ? Arr(l, "profiles").FirstOrDefault(p => Bool(p, "is_active")) : default;
        if (active.ValueKind != JsonValueKind.Object || IsLocalEndpoint(Str(active, "base_url"))) return true;
        var ok = await ShowPrivacyAsync(true);
        if (ok) await Api.CallAsync("settings.set", Args(("key", "privacy.cloud_ack"), ("value", true)));
        return ok;
    }

    private async Task<bool> ShowPrivacyAsync(bool needConfirm)
    {
        var panel = new StackPanel { Spacing = 8, MaxWidth = 480 };
        panel.Children.Add(Line("• 你的来源、笔记和对话保存在这台电脑上。"));
        panel.Children.Add(Line("• 提问、生成产物时，被选中的来源片段、你的问题和相关对话会发送给你配置的模型服务商（接口地址不在本机时）。"));
        panel.Children.Add(Line("• API Key 只保存在 Windows 凭据管理器，不会写入文件或日志。"));
        panel.Children.Add(Line("• 诊断包只含数量、状态和错误码，不含文档内容、提示词、回答或密钥。"));
        var dlg = new ContentDialog { Title = "隐私说明", Content = panel, CloseButtonText = needConfirm ? "取消" : "关闭", XamlRoot = Content.XamlRoot };
        if (needConfirm) { dlg.PrimaryButtonText = "我知道了，继续"; dlg.DefaultButton = ContentDialogButton.Primary; }
        return await dlg.ShowAsync() == ContentDialogResult.Primary;
    }

    private async void OnShowPrivacyClick(object sender, RoutedEventArgs e) => await ShowPrivacyAsync(false);

    private async Task ShowOnboardingIfFirstRunAsync()
    {
        var seen = await Api.CallAsync("settings.get", Args(("key", "onboarding.done")));
        if (seen.Result is { } s && s.TryGetProperty("value", out var v) && v.ValueKind == JsonValueKind.True) return;
        var panel = new StackPanel { Spacing = 10, MaxWidth = 460 };
        panel.Children.Add(Line("欢迎使用 Local Notebook Agent。三步开始："));
        panel.Children.Add(Line("1. 配置模型：点击右上角“模型协议配置”，选择协议、填写接口地址、模型和 API Key，可为多个模型勾选负责的图文、音频或视频模态，点“保存”。"));
        panel.Children.Add(Line("2. 添加来源：左侧“+ 添加来源”可导入 PDF（含扫描件与图文混合）、Markdown、文本、Word/PPT/EPUB/ODT/RTF/HWP/CSV、图片、音视频及网页，并勾选要使用的来源。"));
        panel.Children.Add(Line("3. 提问或生成：在中间提问（回答带可点击的引用），或在右侧 Studio 生成测验、闪卡、导图、报告等。"));
        panel.Children.Add(Line("想要跨语言也能找得准，可以在“语义检索”里下载嵌入模型（可选）。"));
        var dlg = new ContentDialog { Title = "快速开始", Content = panel, PrimaryButtonText = "去配置模型", CloseButtonText = "稍后", DefaultButton = ContentDialogButton.Primary, XamlRoot = Content.XamlRoot };
        await Api.CallAsync("settings.set", Args(("key", "onboarding.done"), ("value", true)));
        if (await dlg.ShowAsync() == ContentDialogResult.Primary) OnOpenSettingsClick(this, new RoutedEventArgs());
    }
}
