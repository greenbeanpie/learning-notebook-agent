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

    // ---------- unified settings (Appearance, General, AI Models, Semantic Search) ----------

    private async void OnOpenSettingsClick(object sender, RoutedEventArgs e)
    {
        var list = await CoreAsync("profile.list", Args(), L10n.T("settings.tab_models"));
        var profiles = list is { } lr ? Arr(lr, "profiles").ToList() : new List<JsonElement>();
        var avEnabled = true;
        var mediaSet = await Api.CallAsync("settings.get", Args(("key", "media_processing")));
        if (mediaSet.Result is { } ms && ms.TryGetProperty("value", out var mv) && mv.ValueKind == JsonValueKind.Object)
        {
            if (mv.TryGetProperty("audio_video_enabled", out var avp) && avp.ValueKind == JsonValueKind.False)
                avEnabled = false;
        }

        // ---- Tab 1: Appearance & Interface ----
        var langBox = new ComboBox { Header = L10n.T("settings.language"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var langOpts = new[] { (L10n.T("settings.lang_auto"), "auto"), (L10n.T("settings.lang_zh"), "zh-CN"), (L10n.T("settings.lang_en"), "en-US") };
        foreach (var (lbl, tag) in langOpts) langBox.Items.Add(new ComboBoxItem { Content = lbl, Tag = tag });
        langBox.SelectedIndex = Math.Max(0, Array.FindIndex(langOpts, x => x.Item2 == L10n.Prefs.Language));

        var themeBox = new ComboBox { Header = L10n.T("settings.theme"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var themeOpts = new[] { (L10n.T("settings.theme_dark"), "dark"), (L10n.T("settings.theme_light"), "light"), (L10n.T("settings.theme_system"), "system") };
        foreach (var (lbl, tag) in themeOpts) themeBox.Items.Add(new ComboBoxItem { Content = lbl, Tag = tag });
        themeBox.SelectedIndex = Math.Max(0, Array.FindIndex(themeOpts, x => x.Item2 == L10n.Prefs.Theme));

        var fontBox = new ComboBox { Header = L10n.T("settings.font_size"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var fontOpts = new[] { (L10n.T("settings.font_compact"), 14), (L10n.T("settings.font_normal"), 15), (L10n.T("settings.font_large"), 17) };
        foreach (var (lbl, val) in fontOpts) fontBox.Items.Add(new ComboBoxItem { Content = lbl, Tag = val });
        fontBox.SelectedIndex = Math.Max(0, Array.FindIndex(fontOpts, x => x.Item2 == L10n.Prefs.FontSize));

        var fsAnimToggle = new ToggleSwitch { Header = L10n.T("settings.fullscreen_anim"), IsOn = L10n.Prefs.FullscreenAnimation };
        var prefStatus = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.85 };

        var appearancePanel = new StackPanel { Spacing = 12, Padding = new Thickness(4, 8, 4, 8) };
        appearancePanel.Children.Add(langBox);
        appearancePanel.Children.Add(themeBox);
        appearancePanel.Children.Add(fontBox);
        appearancePanel.Children.Add(fsAnimToggle);
        appearancePanel.Children.Add(prefStatus);

        // ---- Tab 2: General & Features ----
        var expandThoughtsToggle = new ToggleSwitch { Header = L10n.T("settings.expand_thoughts"), IsOn = L10n.Prefs.ExpandThoughtsByDefault };
        var showSuggestionsToggle = new ToggleSwitch { Header = L10n.T("settings.show_suggestions"), IsOn = L10n.Prefs.ShowSuggestedQuestions };
        var studioLangBox = new ComboBox { Header = L10n.T("settings.default_studio_lang"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var studioLangOpts = new[] { (L10n.T("settings.studio_lang_source"), ""), (L10n.T("settings.lang_zh"), "zh-CN"), (L10n.T("settings.lang_en"), "en") };
        foreach (var (lbl, tag) in studioLangOpts) studioLangBox.Items.Add(new ComboBoxItem { Content = lbl, Tag = tag });
        studioLangBox.SelectedIndex = Math.Max(0, Array.FindIndex(studioLangOpts, x => x.Item2 == L10n.Prefs.DefaultStudioLang));

        var diagBtn = new Button { Content = L10n.T("settings.diag_btn"), Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        diagBtn.Click += OnExportDiagnosticsClick;
        var privacyBtn = new Button { Content = L10n.T("settings.privacy_btn"), Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        privacyBtn.Click += async (_, _) => await ShowPrivacyAsync(false);
        var generalActions = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Margin = new Thickness(0, 6, 0, 0) };
        generalActions.Children.Add(privacyBtn);
        generalActions.Children.Add(diagBtn);

        var generalPanel = new StackPanel { Spacing = 12, Padding = new Thickness(4, 8, 4, 8) };
        generalPanel.Children.Add(expandThoughtsToggle);
        generalPanel.Children.Add(showSuggestionsToggle);
        generalPanel.Children.Add(studioLangBox);
        generalPanel.Children.Add(generalActions);

        async Task SaveUiPrefsAsync()
        {
            L10n.Prefs.Language = (langBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "auto";
            L10n.Prefs.Theme = (themeBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "dark";
            L10n.Prefs.FontSize = (fontBox.SelectedItem as ComboBoxItem)?.Tag is int fsz ? fsz : 15;
            L10n.Prefs.FullscreenAnimation = fsAnimToggle.IsOn;
            L10n.Prefs.ExpandThoughtsByDefault = expandThoughtsToggle.IsOn;
            L10n.Prefs.ShowSuggestedQuestions = showSuggestionsToggle.IsOn;
            L10n.Prefs.DefaultStudioLang = (studioLangBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "";
            await Api.CallAsync("settings.set", Args(("key", "ui.preferences"), ("value", L10n.Prefs.ToJson())));
            ApplyUiPreferences();
            prefStatus.Text = L10n.T("settings.saved");
        }

        langBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();
        themeBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();
        fontBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();
        fsAnimToggle.Toggled += async (_, _) => await SaveUiPrefsAsync();
        expandThoughtsToggle.Toggled += async (_, _) => await SaveUiPrefsAsync();
        showSuggestionsToggle.Toggled += async (_, _) => await SaveUiPrefsAsync();
        studioLangBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();

        // ---- Tab 3: AI Models ----
        string? profileId = null;
        var keychainRef = "";
        var hasSecret = false;

        var picker = new ComboBox { Header = L10n.IsEnglish ? "Saved Profiles" : "已保存的配置", HorizontalAlignment = HorizontalAlignment.Stretch };
        var protocolBox = new ComboBox { Header = L10n.IsEnglish ? "Protocol" : "协议", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var name in Protocols) protocolBox.Items.Add(new ComboBoxItem { Content = name, Tag = name });
        var urlBox = new TextBox { Header = L10n.IsEnglish ? "Base URL" : "接口地址 (Base URL)", PlaceholderText = "https://api.example.com/v1" };
        var modelBox = new ComboBox { Header = L10n.IsEnglish ? "Model ID" : "模型 ID", IsEditable = true, HorizontalAlignment = HorizontalAlignment.Stretch };
        var discoverBtn = new Button { Content = L10n.IsEnglish ? "Fetch Models" : "获取模型列表" };
        var keyBox = new PasswordBox { Header = "API Key" };
        var allowInsecureHttpBox = new CheckBox { Content = L10n.IsEnglish ? "Allow non-loopback plaintext HTTP (trusted local network only)" : "确认允许非回环 HTTP 明文传输（仅用于可信本地网络）" };
        var activateChatBox = new CheckBox { Content = L10n.IsEnglish ? "Set as default chat model" : "设为当前默认对话模型", IsChecked = true };
        var visionBox = new CheckBox { Content = L10n.IsEnglish ? "Use for vision (images / scanned PDFs)" : "用于处理图文（图片 / 扫描 PDF / 图文混合视觉读取）" };
        var audioBox = new CheckBox { Content = L10n.IsEnglish ? "Use for audio" : "用于处理音频" };
        var videoBox = new CheckBox { Content = L10n.IsEnglish ? "Use for video" : "用于处理视频" };
        var audioVideoToggle = new ToggleSwitch { Header = L10n.IsEnglish ? "Enable native audio/video processing" : "允许处理音视频文件（直接交给原生支持音视频的 AI 处理）", IsOn = avEnabled };
        var mediaRoutingSummary = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.8 };
        var thinkingBox = new ToggleSwitch { Header = L10n.IsEnglish ? "Show model thinking process" : "显示模型的思考过程（如果模型提供）", IsOn = true };
        var effortBox = new ComboBox { Header = L10n.IsEnglish ? "Reasoning Effort" : "推理强度", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var (label, tag) in new[] { (L10n.IsEnglish ? "Default" : "默认", ""), ("low", "low"), ("medium", "medium"), ("high", "high") })
            effortBox.Items.Add(new ComboBoxItem { Content = label, Tag = tag });
        var ctxBox = new NumberBox { Header = L10n.IsEnglish ? "Context Window (tokens)" : "上下文窗口 (tokens)", Value = 32000, Minimum = 2000, Maximum = 2_000_000, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        var outBox = new NumberBox { Header = L10n.IsEnglish ? "Max Output Tokens" : "单次最大输出 (tokens)", Value = 8000, Minimum = 256, Maximum = 200_000, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        var priceIn = new NumberBox { Header = L10n.IsEnglish ? "Input price ($ / 1M tokens, optional)" : "输入单价 ($ / 百万 tokens，可留空)", Value = double.NaN, Minimum = 0 };
        var priceOut = new NumberBox { Header = L10n.IsEnglish ? "Output price ($ / 1M tokens, optional)" : "输出单价 ($ / 百万 tokens，可留空)", Value = double.NaN, Minimum = 0 };
        var headersBox = new TextBox { Header = L10n.IsEnglish ? "Extra headers (Key: Value per line)" : "额外请求头（每行 名称: 值）", AcceptsReturn = true, MinHeight = 60, TextWrapping = TextWrapping.Wrap };
        var caps = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.8 };
        var status = new TextBlock { TextWrapping = TextWrapping.Wrap };
        var newBtn = new Button { Content = L10n.IsEnglish ? "New" : "新建配置" };
        var deleteBtn = new Button { Content = L10n.IsEnglish ? "Delete" : "删除此配置" };

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
            var chatName = chatModel.ValueKind == JsonValueKind.Object ? Str(chatModel, "display_name") : (L10n.IsEnglish ? "Not configured" : "未配置");
            mediaRoutingSummary.Text = L10n.IsEnglish
                ? $"Current model routing:\n• Chat: {chatName}\n• Vision: {PickMod("supports_vision", "None (local OCR fallback)")}\n• Audio: {(audioVideoToggle.IsOn ? PickMod("supports_audio", "Not assigned") : "Disabled")}\n• Video: {(audioVideoToggle.IsOn ? PickMod("supports_video", "Not assigned") : "Disabled")}"
                : $"当前多模型分配：\n• 默认对话：{chatName}\n• 图文处理：{PickMod("supports_vision", "未指定（将尝试本地 OCR）")}\n• 音频处理：{(audioVideoToggle.IsOn ? PickMod("supports_audio", "未指定") : "已关闭")}\n• 视频处理：{(audioVideoToggle.IsOn ? PickMod("supports_video", "未指定") : "已关闭")}";
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
                keyBox.PlaceholderText = L10n.IsEnglish ? "Enter API Key (optional for local models)" : "输入 API Key（本地模型可留空）";
                status.Text = L10n.IsEnglish ? "Editing new profile. Fill in details and save." : "正在编辑新配置，填写并保存即可启用。";
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
            keyBox.PlaceholderText = hasSecret
                ? (L10n.IsEnglish ? "Saved (leave blank to keep)" : "已保存，留空则保持不变")
                : (L10n.IsEnglish ? "Enter API Key (optional for local models)" : "输入 API Key（本地模型可留空）");
            var c = Obj(prof, "capabilities");
            visionBox.IsChecked = c.ValueKind == JsonValueKind.Object && Bool(c, "supports_vision");
            audioBox.IsChecked = c.ValueKind == JsonValueKind.Object && Bool(c, "supports_audio");
            videoBox.IsChecked = c.ValueKind == JsonValueKind.Object && Bool(c, "supports_video");
            caps.Text = "";
            status.Text = hasSecret
                ? (L10n.IsEnglish ? "Key status: saved in Windows Credential Manager." : "密钥状态：已保存在 Windows 凭据管理器。")
                : (L10n.IsEnglish ? "Key status: not saved." : "密钥状态：未保存。");
        }

        void LoadPicker(string? select)
        {
            picker.Items.Clear();
            foreach (var p in profiles)
            {
                var c = Obj(p, "capabilities");
                var mods = new List<string>();
                if (Bool(c, "supports_vision")) mods.Add(L10n.IsEnglish ? "Vision" : "图文");
                if (Bool(c, "supports_audio")) mods.Add(L10n.IsEnglish ? "Audio" : "音频");
                if (Bool(c, "supports_video")) mods.Add(L10n.IsEnglish ? "Video" : "视频");
                var modTag = mods.Count > 0 ? $" [{string.Join("/", mods)}]" : "";
                var defTag = Bool(p, "is_active") ? (L10n.IsEnglish ? " (Default)" : "（默认对话）") : "";
                var item = new ComboBoxItem { Content = $"{Str(p, "display_name")}{defTag}{modTag}", Tag = Str(p, "id") };
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
            if (await CoreAsync("profile.delete", Args(("profile_id", profileId)), L10n.T("common.delete")) == null) return;
            profiles.RemoveAll(x => Str(x, "id") == profileId);
            LoadPicker(null);
            Fill(null);
            status.Text = L10n.IsEnglish ? "Profile deleted." : "配置已删除。";
        };
        discoverBtn.Click += async (_, _) =>
        {
            var baseUrl = urlBox.Text.Trim();
            if (string.IsNullOrEmpty(baseUrl))
            {
                status.Text = L10n.IsEnglish ? "Please enter Base URL first." : "请先填写接口地址 (Base URL)。";
                return;
            }
            var protocol = (protocolBox.SelectedItem as ComboBoxItem)?.Tag as string ?? Protocols[0];
            var apiKey = keyBox.Password.Trim();
            var res = await CoreAsync("profile.discover_models", Args(("profile_id", profileId ?? ""), ("base_url", baseUrl), ("protocol", protocol), ("api_key", apiKey)), "Fetch Models");
            if (res is not { } r) return;
            var current = modelBox.Text;
            modelBox.Items.Clear();
            foreach (var m in Arr(r, "models")) modelBox.Items.Add(Str(m, "id"));
            modelBox.Text = current;
            status.Text = L10n.IsEnglish ? $"Found {modelBox.Items.Count} models." : $"找到 {modelBox.Items.Count} 个模型，可在“模型 ID”下拉框中选择。";
        };

        var saveModelBtn = new Button { Content = L10n.T("common.save"), Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"] };
        var clearKeyBtn = new Button { Content = L10n.IsEnglish ? "Clear Key" : "清除密钥", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        var testBtn = new Button { Content = L10n.IsEnglish ? "Test Connection" : "测试连接", Style = (Style)Application.Current.Resources["PillButtonStyle"] };

        saveModelBtn.Click += async (_, _) =>
        {
            var url = urlBox.Text.Trim();
            var modelId = (string.IsNullOrWhiteSpace(modelBox.Text) ? modelBox.SelectedItem as string : modelBox.Text)?.Trim() ?? "";
            if (url.Length == 0 || modelId.Length == 0)
            {
                status.Text = L10n.IsEnglish ? "Please fill in Base URL and Model ID." : "请填写接口地址和模型 ID。";
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
            var saved = await CoreAsync("profile.upsert", Args(up.ToArray()), L10n.T("common.save"));
            if (saved is not { } sv) { status.Text = L10n.IsEnglish ? "Failed to save profile." : "保存失败。"; return; }
            profileId = Str(sv, "id");
            keychainRef = Str(sv, "keychain_ref");
            if (keyBox.Password.Length > 0)
            {
                var set = await CoreAsync("secret.set", Args(("keychain_ref", keychainRef), ("secret", keyBox.Password)), "API Key");
                if (set == null) return;
                keyBox.Password = string.Empty;
            }
            var ex = await CoreAsync("secret.exists", Args(("keychain_ref", keychainRef)), "API Key");
            hasSecret = ex is { } er && Bool(er, "exists");
            keyBox.PlaceholderText = hasSecret
                ? (L10n.IsEnglish ? "Saved (leave blank to keep)" : "已保存，留空则保持不变")
                : (L10n.IsEnglish ? "Enter API Key (optional for local models)" : "输入 API Key（本地模型可留空）");
            if (makeActive) await CoreAsync("profile.activate", Args(("profile_id", profileId)), L10n.T("common.save"));
            await Api.CallAsync("settings.set", Args(("key", "media_processing"), ("value", Args(("audio_video_enabled", audioVideoToggle.IsOn)))));
            status.Text = L10n.IsEnglish ? "Model profile saved." : "模型配置已保存。";
            var relist = await CoreAsync("profile.list", Args(), L10n.T("settings.tab_models"));
            if (relist is { } rl)
            {
                profiles = Arr(rl, "profiles").ToList();
                LoadPicker(profileId);
            }
        };

        clearKeyBtn.Click += async (_, _) =>
        {
            if (profileId == null || keychainRef.Length == 0) { status.Text = L10n.IsEnglish ? "No key to clear." : "没有可清除的密钥。"; return; }
            if (await CoreAsync("secret.clear", Args(("keychain_ref", keychainRef)), "Clear Key") == null) return;
            hasSecret = false;
            keyBox.PlaceholderText = L10n.IsEnglish ? "Enter API Key (optional for local models)" : "输入 API Key（本地模型可留空）";
            status.Text = L10n.IsEnglish ? "API key cleared." : "密钥已清除。";
        };

        testBtn.Click += async (_, _) =>
        {
            if (profileId == null) { status.Text = L10n.IsEnglish ? "Save the profile before testing." : "请先保存配置，再测试连接。"; return; }
            status.Text = L10n.IsEnglish ? "Testing connection..." : "正在测试连接…";
            var test = await CoreAsync("profile.test", Args(("profile_id", profileId)), "Test");
            status.Text = test is { } tr
                ? (L10n.IsEnglish ? $"Connected: {Str(tr, "reply")}" : $"连接成功：{Str(tr, "reply")}")
                : (L10n.IsEnglish ? "Connection test failed." : "连接测试失败。");
        };

        audioVideoToggle.Toggled += async (_, _) =>
        {
            await Api.CallAsync("settings.set", Args(("key", "media_processing"), ("value", Args(("audio_video_enabled", audioVideoToggle.IsOn)))));
            UpdateMediaSummary();
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

        var modalityBox = new StackPanel { Spacing = 4 };
        modalityBox.Children.Add(visionBox);
        modalityBox.Children.Add(audioBox);
        modalityBox.Children.Add(videoBox);

        var mediaSettingsBox = new StackPanel { Spacing = 8, Padding = new Thickness(10), CornerRadius = new CornerRadius(8), Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"] };
        mediaSettingsBox.Children.Add(audioVideoToggle);
        mediaSettingsBox.Children.Add(mediaRoutingSummary);

        var modelActions = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        modelActions.Children.Add(saveModelBtn);
        modelActions.Children.Add(testBtn);
        modelActions.Children.Add(clearKeyBtn);

        var modelsPanel = new StackPanel { Spacing = 10, Padding = new Thickness(4, 8, 4, 8) };
        modelsPanel.Children.Add(top);
        modelsPanel.Children.Add(protocolBox);
        modelsPanel.Children.Add(urlBox);
        modelsPanel.Children.Add(modelRow);
        modelsPanel.Children.Add(keyBox);
        modelsPanel.Children.Add(allowInsecureHttpBox);
        modelsPanel.Children.Add(activateChatBox);
        modelsPanel.Children.Add(modalityBox);
        modelsPanel.Children.Add(new Expander { Header = L10n.IsEnglish ? "Advanced Options" : "高级选项", Content = advanced, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch });
        modelsPanel.Children.Add(mediaSettingsBox);
        modelsPanel.Children.Add(modelActions);
        modelsPanel.Children.Add(status);

        // ---- Tab 4: Semantic Search (Embeddings) ----
        var embStatus = new TextBlock { TextWrapping = TextWrapping.Wrap };
        var embProgress = new ProgressBar { Maximum = 100, Visibility = Visibility.Collapsed };
        var embDownload = new Button { Content = L10n.IsEnglish ? "Download & Enable" : "下载并启用", Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"] };
        var embCancel = new Button { Content = L10n.T("common.cancel"), Visibility = Visibility.Collapsed };
        var embRebuild = new Button { Content = L10n.IsEnglish ? "Rebuild Index" : "重建索引", Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        _embProgress = embProgress; _embStatus = embStatus; _embDownload = embDownload; _embCancel = embCancel;

        async Task RefreshEmbAsync()
        {
            var es = await CoreAsync("embedding.status", Args(), L10n.T("settings.tab_embedding"));
            var dm = await CoreAsync("embedding.default_model", Args(), L10n.T("settings.tab_embedding"));
            var dl = await CoreAsync("embedding.download_status", Args(), L10n.T("settings.tab_embedding"));
            if (es is not { } s || dm is not { } d) return;
            var running = dl is { } x && Bool(x, "running");
            var activeModel = Obj(s, "active_model");
            var total = Int(s, "total_chunks");
            var done = Int(s, "embedded_chunks");
            var mb = Int(d, "total_bytes") / 1_000_000;
            if (activeModel.ValueKind == JsonValueKind.Object)
                embStatus.Text = L10n.IsEnglish
                    ? $"Active model: {Str(activeModel, "model_name")}. Indexed chunks: {done}/{total}."
                    : $"当前模型：{Str(activeModel, "model_name")}。已建立索引的片段：{done}/{total}。";
            else
                embStatus.Text = Bool(d, "installed")
                    ? (L10n.IsEnglish ? "Model downloaded but not enabled." : "模型文件已下载，尚未启用。")
                    : (L10n.IsEnglish ? $"Semantic search not enabled. Default model is ~{mb} MB." : $"尚未启用语义检索。默认模型约 {mb} MB。");
            embDownload.Content = Bool(d, "installed")
                ? (L10n.IsEnglish ? "Enable Downloaded Model" : "启用已下载的模型")
                : (L10n.IsEnglish ? $"Download & Enable (~{mb} MB)" : $"下载并启用（约 {mb} MB）");
            embDownload.IsEnabled = !running;
            embCancel.Visibility = running ? Visibility.Visible : Visibility.Collapsed;
            embProgress.Visibility = running ? Visibility.Visible : Visibility.Collapsed;
            embRebuild.IsEnabled = activeModel.ValueKind == JsonValueKind.Object;
        }
        embDownload.Click += async (_, _) =>
        {
            if (await CoreAsync("embedding.download", Args(), L10n.T("settings.tab_embedding")) != null)
            {
                embProgress.Value = 0;
                await RefreshEmbAsync();
            }
        };
        embCancel.Click += async (_, _) => { await CoreAsync("embedding.download_cancel", Args(), L10n.T("common.cancel")); };
        embRebuild.Click += async (_, _) =>
        {
            var r = await CoreAsync("embedding.rebuild", Args(("force", true)), L10n.T("settings.tab_embedding"));
            if (r is { } rr) embStatus.Text = L10n.IsEnglish ? $"Queued {Int(rr, "sources_queued")} sources for rebuild." : $"已排队重建 {Int(rr, "sources_queued")} 个来源。";
        };
        await RefreshEmbAsync();
        _embRefresh = RefreshEmbAsync;

        var embRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        embRow.Children.Add(embDownload);
        embRow.Children.Add(embCancel);
        embRow.Children.Add(embRebuild);

        var embPanel = new StackPanel { Spacing = 10, Padding = new Thickness(4, 8, 4, 8) };
        embPanel.Children.Add(Line(L10n.IsEnglish
            ? "Semantic search finds relevant passages across different phrasings or languages using a local embedding model."
            : "语义检索让提问和来源语言不同、或措辞不同时也能找到相关内容。它使用本地嵌入模型，不会上传你的内容。"));
        embPanel.Children.Add(embStatus);
        embPanel.Children.Add(embProgress);
        embPanel.Children.Add(embRow);

        var pivot = new Pivot { MinWidth = 540 };
        pivot.Items.Add(new PivotItem { Header = L10n.T("settings.tab_appearance"), Content = new ScrollViewer { Content = appearancePanel, MaxHeight = 560 } });
        pivot.Items.Add(new PivotItem { Header = L10n.T("settings.tab_general"), Content = new ScrollViewer { Content = generalPanel, MaxHeight = 560 } });
        pivot.Items.Add(new PivotItem { Header = L10n.T("settings.tab_models"), Content = new ScrollViewer { Content = modelsPanel, MaxHeight = 560 } });
        pivot.Items.Add(new PivotItem { Header = L10n.T("settings.tab_embedding"), Content = new ScrollViewer { Content = embPanel, MaxHeight = 560 } });

        var dialog = new ContentDialog
        {
            Title = L10n.T("settings.title"),
            Content = pivot,
            CloseButtonText = L10n.T("common.close"),
            XamlRoot = Content.XamlRoot,
        };

        await dialog.ShowAsync();
        _embRefresh = null;
        _embProgress = null; _embStatus = null; _embDownload = null; _embCancel = null;
        await WarnIfNoModelAsync();
        await RefreshNotebookDataAsync();
    }

    // ---------- privacy, onboarding ----------

    private static bool IsLocalEndpoint(string url) =>
        Uri.TryCreate(url, UriKind.Absolute, out var u) && (u.IsLoopback || u.Host == "localhost");

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
        if (L10n.IsEnglish)
        {
            panel.Children.Add(Line("• Your sources, notes, and conversations are stored locally on this computer."));
            panel.Children.Add(Line("• When asking questions or generating Studio artifacts, selected excerpts and prompts are sent to your configured model provider."));
            panel.Children.Add(Line("• API keys are stored securely in Windows Credential Manager and never written to plain files."));
            panel.Children.Add(Line("• Diagnostic exports contain only counts, statuses, and error codes—never document contents or keys."));
        }
        else
        {
            panel.Children.Add(Line("• 你的来源、笔记和对话保存在这台电脑上。"));
            panel.Children.Add(Line("• 提问、生成产物时，被选中的来源片段、你的问题和相关对话会发送给你配置的模型服务商（接口地址不在本机时）。"));
            panel.Children.Add(Line("• API Key 只保存在 Windows 凭据管理器，不会写入文件或日志。"));
            panel.Children.Add(Line("• 诊断包只含数量、状态和错误码，不含文档内容、提示词、回答或密钥。"));
        }
        var dlg = new ContentDialog
        {
            Title = L10n.T("nb.privacy"),
            Content = panel,
            CloseButtonText = needConfirm ? L10n.T("common.cancel") : L10n.T("common.close"),
            XamlRoot = Content.XamlRoot,
        };
        if (needConfirm)
        {
            dlg.PrimaryButtonText = L10n.IsEnglish ? "Got it, continue" : "我知道了，继续";
            dlg.DefaultButton = ContentDialogButton.Primary;
        }
        return await dlg.ShowAsync() == ContentDialogResult.Primary;
    }

    private async void OnShowPrivacyClick(object sender, RoutedEventArgs e) => await ShowPrivacyAsync(false);

    private async Task ShowOnboardingIfFirstRunAsync()
    {
        var seen = await Api.CallAsync("settings.get", Args(("key", "onboarding.done")));
        if (seen.Result is { } s && s.TryGetProperty("value", out var v) && v.ValueKind == JsonValueKind.True) return;
        var panel = new StackPanel { Spacing = 10, MaxWidth = 460 };
        if (L10n.IsEnglish)
        {
            panel.Children.Add(Line("Welcome to Local Notebook Agent. Get started in 3 steps:"));
            panel.Children.Add(Line("1. Configure a model: Open Settings → AI Models to enter your protocol, Base URL, model ID, and API key."));
            panel.Children.Add(Line("2. Add sources: Click '+ Add source' on the left to import PDFs, Markdown, Office docs, images, audio/video, or web URLs."));
            panel.Children.Add(Line("3. Ask or generate: Ask questions with clickable citations in the center, or generate quizzes, flashcards, mind maps, and reports in Studio."));
        }
        else
        {
            panel.Children.Add(Line("欢迎使用 Local Notebook Agent。三步开始："));
            panel.Children.Add(Line("1. 配置模型：点击右上角“设置”→“AI 模型”，选择协议并填写接口地址、模型和 API Key，点“保存”。"));
            panel.Children.Add(Line("2. 添加来源：左侧“+ 添加来源”可导入 PDF、Markdown、文档、图片、音视频及网页，并勾选要使用的来源。"));
            panel.Children.Add(Line("3. 提问或生成：在中间提问（回答带可点击的引用），或在右侧 Studio 生成测验、闪卡、思维导图、报告等。"));
        }
        var dlg = new ContentDialog
        {
            Title = L10n.IsEnglish ? "Quick Start" : "快速开始",
            Content = panel,
            PrimaryButtonText = L10n.IsEnglish ? "Open Settings" : "打开设置",
            CloseButtonText = L10n.IsEnglish ? "Later" : "稍后",
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = Content.XamlRoot,
        };
        await Api.CallAsync("settings.set", Args(("key", "onboarding.done"), ("value", true)));
        if (await dlg.ShowAsync() == ContentDialogResult.Primary) OnOpenSettingsClick(this, new RoutedEventArgs());
    }
}
