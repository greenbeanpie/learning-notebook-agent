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

        void Fill(JsonElement? p)
        {
            if (p is not { } prof)
            {
                profileId = null; keychainRef = ""; hasSecret = false;
                protocolBox.SelectedIndex = 0;
                urlBox.Text = ""; modelBox.Items.Clear(); modelBox.Text = "";
                thinkingBox.IsOn = true; effortBox.SelectedIndex = 0;
                ctxBox.Value = 32000; outBox.Value = 8000; priceIn.Value = double.NaN; priceOut.Value = double.NaN;
                headersBox.Text = ""; caps.Text = "";
                keyBox.Password = "";
                keyBox.PlaceholderText = "输入 API Key（本地模型可留空）";
                status.Text = "尚未配置模型。";
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
            caps.Text = c.ValueKind == JsonValueKind.Object
                ? $"已探测的能力：工具调用 {(Bool(c, "tools") ? "✔" : "✖")} · JSON Schema {(Bool(c, "json_schema") ? "✔" : "✖")} · JSON 模式 {(Bool(c, "json_mode") ? "✔" : "✖")} · 流式用量 {(Bool(c, "stream_usage") ? "✔" : "✖")}"
                : "";
            status.Text = hasSecret ? "密钥状态：已保存在 Windows 凭据管理器。" : "密钥状态：未保存。";
        }

        void LoadPicker(string? select)
        {
            picker.Items.Clear();
            foreach (var p in profiles)
            {
                var item = new ComboBoxItem { Content = $"{Str(p, "display_name")}{(Bool(p, "is_active") ? "（使用中）" : "")}", Tag = Str(p, "id") };
                picker.Items.Add(item);
                if (Str(p, "id") == select) picker.SelectedItem = item;
            }
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
            if (profileId == null) { status.Text = "请先点击“保存并测试”，再获取模型列表。"; return; }
            var res = await CoreAsync("profile.discover_models", Args(("profile_id", profileId)), "获取模型列表");
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

        var panel = new StackPanel { Spacing = 10, MinWidth = 480 };
        panel.Children.Add(top);
        panel.Children.Add(protocolBox);
        panel.Children.Add(urlBox);
        panel.Children.Add(modelRow);
        panel.Children.Add(keyBox);
        panel.Children.Add(allowInsecureHttpBox);
        panel.Children.Add(new Expander { Header = "高级选项", Content = advanced, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch });
        panel.Children.Add(status);

        var dialog = new ContentDialog
        {
            Title = "模型协议配置",
            Content = new ScrollViewer { Content = panel, MaxHeight = 620 },
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
                var modelId = (string.IsNullOrWhiteSpace(modelBox.Text) ? modelBox.SelectedItem as string : modelBox.Text)?.Trim() ?? "";
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
                    ("allow_insecure_http", allowInsecureHttpBox.IsChecked == true),
                    ("show_thinking", thinkingBox.IsOn),
                    ("context_window", (int)ctxBox.Value),
                    ("max_output_tokens", (int)outBox.Value),
                    ("extra_headers", HeadersJson(headersBox.Text)),
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

                await CoreAsync("profile.activate", Args(("profile_id", profileId)), "启用模型配置");
                var mark = hasSecret ? "密钥状态：已保存在 Windows 凭据管理器。" : "密钥状态：未保存。";
                status.Text = $"{mark} 正在测试连接并探测模型能力…";
                var test = await CoreAsync("profile.test", Args(("profile_id", profileId)), "测试连接");
                if (test is { } tr)
                {
                    status.Text = $"{mark} 连接成功，模型回复：{Str(tr, "reply")}";
                    var c = Obj(tr, "capabilities");
                    caps.Text = $"已探测的能力：工具调用 {(Bool(c, "tools") ? "✔" : "✖")} · JSON Schema {(Bool(c, "json_schema") ? "✔" : "✖")} · JSON 模式 {(Bool(c, "json_mode") ? "✔" : "✖")} · 流式用量 {(Bool(c, "stream_usage") ? "✔" : "✖")}";
                }
                else
                {
                    status.Text = $"{mark} 连接测试失败，详见底部提示。";
                }
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
        panel.Children.Add(Line("1. 配置模型：点击右上角“模型协议配置”，选择协议、填写接口地址、模型和 API Key，点“保存并测试”。"));
        panel.Children.Add(Line("2. 添加来源：左侧“+ 添加来源”可导入 PDF、Markdown、文本、网页，或直接粘贴文本，并勾选要使用的来源。"));
        panel.Children.Add(Line("3. 提问或生成：在中间提问（回答带可点击的引用），或在右侧 Studio 生成测验、闪卡、导图、报告等。"));
        panel.Children.Add(Line("想要跨语言也能找得准，可以在“语义检索”里下载嵌入模型（可选）。"));
        var dlg = new ContentDialog { Title = "快速开始", Content = panel, PrimaryButtonText = "去配置模型", CloseButtonText = "稍后", DefaultButton = ContentDialogButton.Primary, XamlRoot = Content.XamlRoot };
        await Api.CallAsync("settings.set", Args(("key", "onboarding.done"), ("value", true)));
        if (await dlg.ShowAsync() == ContentDialogResult.Primary) OnOpenSettingsClick(this, new RoutedEventArgs());
    }
}
