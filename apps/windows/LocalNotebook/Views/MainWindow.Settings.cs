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
        var presetRes = await CoreAsync("profile.presets", Args(), L10n.T("settings.tab_models"));
        var presets = presetRes is { } pr ? Arr(pr, "presets").ToList() : new List<JsonElement>();
        var avEnabled = true;
        string visionProfileId = "", audioProfileId = "", videoProfileId = "";
        var mediaSet = await Api.CallAsync("settings.get", Args(("key", "media_processing")));
        if (mediaSet.Result is { } ms && ms.TryGetProperty("value", out var mv) && mv.ValueKind == JsonValueKind.Object)
        {
            if (mv.TryGetProperty("audio_video_enabled", out var avp) && avp.ValueKind == JsonValueKind.False)
                avEnabled = false;
            visionProfileId = Str(mv, "vision_profile_id");
            audioProfileId = Str(mv, "audio_profile_id");
            videoProfileId = Str(mv, "video_profile_id");
        }

        // Lecture repair limits ("lecture.limits"): content repair rounds + animation review/repair rounds
        var lectureRounds = 2.0;
        var lectureAnimRounds = 1.0;
        var limitsSet = await Api.CallAsync("settings.get", Args(("key", "lecture.limits")));
        if (limitsSet.Result is { } ls && ls.TryGetProperty("value", out var lv) && lv.ValueKind == JsonValueKind.Object)
        {
            if (lv.TryGetProperty("repair_rounds", out var rr) && rr.ValueKind == JsonValueKind.Number && rr.TryGetInt32(out var rrn))
                lectureRounds = Math.Clamp(rrn, 0, 20);
            if (lv.TryGetProperty("anim_repair_rounds", out var ar) && ar.ValueKind == JsonValueKind.Number && ar.TryGetInt32(out var arn))
                lectureAnimRounds = Math.Clamp(arn, 0, 20);
        }

        // Built-in max_token limits & Debug logging state
        int limLectureSlides = 32768, limLectureOutline = 16384, limStudioDefault = 0, limStudioAux = 8192, limAgentAux = 4096, limIngestIndex = 8192;
        var tokRes = await Api.CallAsync("settings.token_limits.get", Args());
        if (tokRes.Result is { } tr && tr.TryGetProperty("limits", out var tl) && tl.ValueKind == JsonValueKind.Object)
        {
            limLectureSlides = (int)Int(tl, "lecture_slides", 32768);
            limLectureOutline = (int)Int(tl, "lecture_outline", 16384);
            limStudioDefault = (int)Int(tl, "studio_default", 0);
            limStudioAux = (int)Int(tl, "studio_aux", 8192);
            limAgentAux = (int)Int(tl, "agent_aux", 4096);
            limIngestIndex = (int)Int(tl, "ingest_index", 8192);
        }
        var debugEnabled = false;
        var dbgRes = await Api.CallAsync("settings.debug.get", Args());
        if (dbgRes.Result is { } dr) debugEnabled = Bool(dr, "enabled");

        // ---- Tab 1: Appearance & Interface ----
        var langBox = new ComboBox { Header = L10n.T("settings.language"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var langOpts = new[] { (L10n.T("settings.lang_auto"), "auto"), (L10n.T("settings.lang_zh"), "zh-CN"), (L10n.T("settings.lang_en"), "en-US") };
        foreach (var (lbl, tag) in langOpts) langBox.Items.Add(new ComboBoxItem { Content = lbl, Tag = tag });
        langBox.SelectedIndex = Math.Max(0, Array.FindIndex(langOpts, x => x.Item2 == L10n.Prefs.Language));

        var themeBox = new ComboBox { Header = L10n.T("settings.theme"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var themeOpts = new[]
        {
            (L10n.T("settings.theme_dark"), "dark"),
            (L10n.T("settings.theme_light"), "light"),
            (L10n.T("settings.theme_system"), "system"),
            (L10n.T("settings.theme_midnight"), "midnight"),
            (L10n.T("settings.theme_forest"), "forest"),
            (L10n.T("settings.theme_warm_paper"), "warm_paper"),
            (L10n.T("settings.theme_rose_dusk"), "rose_dusk"),
        };
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

        var quizViewBox = new ComboBox { Header = L10n.T("settings.quiz_view_mode"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var quizViewOpts = new[] { (L10n.T("quiz.view_continuous"), "continuous"), (L10n.T("quiz.view_one"), "one_by_one") };
        foreach (var (lbl, tag) in quizViewOpts) quizViewBox.Items.Add(new ComboBoxItem { Content = lbl, Tag = tag });
        quizViewBox.SelectedIndex = Math.Max(0, Array.FindIndex(quizViewOpts, x => x.Item2 == L10n.Prefs.QuizViewMode));

        var quizSubmitBox = new ComboBox { Header = L10n.T("settings.quiz_submit_mode"), HorizontalAlignment = HorizontalAlignment.Stretch };
        var quizSubmitOpts = new[] { (L10n.T("quiz.submit_manual"), "manual"), (L10n.T("quiz.submit_auto"), "auto") };
        foreach (var (lbl, tag) in quizSubmitOpts) quizSubmitBox.Items.Add(new ComboBoxItem { Content = lbl, Tag = tag });
        quizSubmitBox.SelectedIndex = Math.Max(0, Array.FindIndex(quizSubmitOpts, x => x.Item2 == L10n.Prefs.QuizSubmitMode));

        var lectureRoundsBox = new NumberBox { Header = L10n.T("settings.lecture_repair_rounds"), Value = lectureRounds, Minimum = 0, Maximum = 20, SmallChange = 1, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        var lectureRoundsHint = new TextBlock { Text = L10n.T("settings.lecture_repair_rounds_hint"), TextWrapping = TextWrapping.Wrap, Opacity = 0.7, FontSize = 12 };
        var lectureAnimRoundsBox = new NumberBox { Header = L10n.T("settings.lecture_anim_repair_rounds"), Value = lectureAnimRounds, Minimum = 0, Maximum = 20, SmallChange = 1, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        var lectureAnimRoundsHint = new TextBlock { Text = L10n.T("settings.lecture_anim_repair_rounds_hint"), TextWrapping = TextWrapping.Wrap, Opacity = 0.7, FontSize = 12 };
        async Task SaveLectureLimitsAsync()
        {
            var cr = double.IsNaN(lectureRoundsBox.Value) ? 2 : (int)Math.Clamp(lectureRoundsBox.Value, 0, 20);
            var ar = double.IsNaN(lectureAnimRoundsBox.Value) ? 1 : (int)Math.Clamp(lectureAnimRoundsBox.Value, 0, 20);
            await Api.CallAsync("settings.set", Args(("key", "lecture.limits"), ("value", Args(("repair_rounds", cr), ("anim_repair_rounds", ar)))));
        }
        lectureRoundsBox.ValueChanged += async (_, _) => await SaveLectureLimitsAsync();
        lectureAnimRoundsBox.ValueChanged += async (_, _) => await SaveLectureLimitsAsync();

        // Built-in Max Token limits
        var tokenLimitsHeader = new TextBlock { Text = L10n.T("settings.token_limits_header"), FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Margin = new Thickness(0, 8, 0, 0) };
        NumberBox MakeTokenBox(string labelKey, int val) => new()
        {
            Header = L10n.T(labelKey),
            Value = val,
            Minimum = 0,
            Maximum = 1000000,
            SmallChange = 1024,
            LargeChange = 4096,
            SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact,
        };
        var nbLectureSlides = MakeTokenBox("settings.token_limit_lecture_slides", limLectureSlides);
        var nbLectureOutline = MakeTokenBox("settings.token_limit_lecture_outline", limLectureOutline);
        var nbStudioDefault = MakeTokenBox("settings.token_limit_studio_default", limStudioDefault);
        var nbStudioAux = MakeTokenBox("settings.token_limit_studio_aux", limStudioAux);
        var nbAgentAux = MakeTokenBox("settings.token_limit_agent_aux", limAgentAux);
        var nbIngestIndex = MakeTokenBox("settings.token_limit_ingest_index", limIngestIndex);
        var tokenLimitsStatus = new TextBlock { FontSize = 12, Opacity = 0.75, TextWrapping = TextWrapping.Wrap };
        var updatingTokenLimits = false;
        async Task SaveTokenLimitsAsync()
        {
            if (updatingTokenLimits) return;
            static int SafeInt(NumberBox b) => double.IsNaN(b.Value) ? 0 : (int)Math.Clamp(b.Value, 0, 1000000);
            await Api.CallAsync("settings.token_limits.set", Args(
                ("lecture_slides", SafeInt(nbLectureSlides)),
                ("lecture_outline", SafeInt(nbLectureOutline)),
                ("studio_default", SafeInt(nbStudioDefault)),
                ("studio_aux", SafeInt(nbStudioAux)),
                ("agent_aux", SafeInt(nbAgentAux)),
                ("ingest_index", SafeInt(nbIngestIndex))
            ));
        }
        foreach (var box in new[] { nbLectureSlides, nbLectureOutline, nbStudioDefault, nbStudioAux, nbAgentAux, nbIngestIndex })
            box.ValueChanged += async (_, _) => await SaveTokenLimitsAsync();

        var resetTokenLimitsBtn = new Button { Content = L10n.T("settings.token_limits_reset"), Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        resetTokenLimitsBtn.Click += async (_, _) =>
        {
            var r = await CoreAsync("settings.token_limits.reset", Args(), L10n.T("settings.token_limits_reset"));
            if (r is { } rr && rr.TryGetProperty("limits", out var tl) && tl.ValueKind == JsonValueKind.Object)
            {
                updatingTokenLimits = true;
                try
                {
                    nbLectureSlides.Value = Int(tl, "lecture_slides", 32768);
                    nbLectureOutline.Value = Int(tl, "lecture_outline", 16384);
                    nbStudioDefault.Value = Int(tl, "studio_default", 0);
                    nbStudioAux.Value = Int(tl, "studio_aux", 8192);
                    nbAgentAux.Value = Int(tl, "agent_aux", 4096);
                    nbIngestIndex.Value = Int(tl, "ingest_index", 8192);
                }
                finally { updatingTokenLimits = false; }
                tokenLimitsStatus.Text = L10n.T("settings.token_limits_reset_done");
            }
        };

        // Logging & Debug mode
        var debugHeader = new TextBlock { Text = L10n.T("settings.debug_section"), FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Margin = new Thickness(0, 8, 0, 0) };
        var debugModeToggle = new ToggleSwitch { Header = L10n.T("settings.debug_mode"), IsOn = debugEnabled };
        var debugModeHint = new TextBlock { Text = L10n.T("settings.debug_mode_hint"), TextWrapping = TextWrapping.Wrap, Opacity = 0.7, FontSize = 12 };
        debugModeToggle.Toggled += async (_, _) =>
        {
            await Api.CallAsync("settings.debug.set", Args(("enabled", debugModeToggle.IsOn)));
        };
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
        generalPanel.Children.Add(quizViewBox);
        generalPanel.Children.Add(quizSubmitBox);
        generalPanel.Children.Add(lectureRoundsBox);
        generalPanel.Children.Add(lectureRoundsHint);
        generalPanel.Children.Add(lectureAnimRoundsBox);
        generalPanel.Children.Add(lectureAnimRoundsHint);
        generalPanel.Children.Add(tokenLimitsHeader);
        generalPanel.Children.Add(nbLectureSlides);
        generalPanel.Children.Add(nbLectureOutline);
        generalPanel.Children.Add(nbStudioDefault);
        generalPanel.Children.Add(nbStudioAux);
        generalPanel.Children.Add(nbAgentAux);
        generalPanel.Children.Add(nbIngestIndex);
        generalPanel.Children.Add(resetTokenLimitsBtn);
        generalPanel.Children.Add(tokenLimitsStatus);
        generalPanel.Children.Add(debugHeader);
        generalPanel.Children.Add(debugModeToggle);
        generalPanel.Children.Add(debugModeHint);
        generalPanel.Children.Add(generalActions);

        async Task SaveUiPrefsAsync()
        {
            var prevEn = L10n.IsEnglish;
            L10n.Prefs.Language = (langBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "auto";
            L10n.Prefs.Theme = (themeBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "dark";
            L10n.Prefs.FontSize = (fontBox.SelectedItem as ComboBoxItem)?.Tag is int fsz ? fsz : 15;
            L10n.Prefs.FullscreenAnimation = fsAnimToggle.IsOn;
            L10n.Prefs.ExpandThoughtsByDefault = expandThoughtsToggle.IsOn;
            L10n.Prefs.ShowSuggestedQuestions = showSuggestionsToggle.IsOn;
            L10n.Prefs.DefaultStudioLang = (studioLangBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "";
            L10n.Prefs.QuizViewMode = (quizViewBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "continuous";
            L10n.Prefs.QuizSubmitMode = (quizSubmitBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "manual";
            L10n.Prefs.QuizModeConfigured = true;
            await Api.CallAsync("settings.set", Args(("key", "ui.preferences"), ("value", L10n.Prefs.ToJson())));
            ApplyUiPreferences();
            if (prevEn != L10n.IsEnglish && _activeNotebookId != null)
            {
                prefStatus.Text = L10n.T("settings.translating_ui");
                _ = Task.Run(async () =>
                {
                    await Api.CallAsync("i18n.translate_notebook_content", Args(("notebook_id", _activeNotebookId), ("target_lang", L10n.TargetLangCode)));
                    DispatcherQueue.TryEnqueue(async () => await RefreshNotebookDataAsync());
                });
            }
            else
            {
                prefStatus.Text = L10n.T("settings.saved");
            }
        }

        langBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();
        themeBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();
        fontBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();
        fsAnimToggle.Toggled += async (_, _) => await SaveUiPrefsAsync();
        expandThoughtsToggle.Toggled += async (_, _) => await SaveUiPrefsAsync();
        showSuggestionsToggle.Toggled += async (_, _) => await SaveUiPrefsAsync();
        studioLangBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();
        quizViewBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();
        quizSubmitBox.SelectionChanged += async (_, _) => await SaveUiPrefsAsync();

        // ---- Tab 3: AI Models ----
        string? profileId = null;
        var keychainRef = "";
        var hasSecret = false;
        // Apply is enabled only when the form differs from what was last loaded or saved;
        // `savedForm` is that snapshot. `modelBusy` covers a running apply / test / delete.
        var savedForm = "";
        var modelBusy = false;
        // set while Fill() selects a preset, so the auto-fill handler does not overwrite the loaded profile
        var suppressPresetFill = false;
        Action formChanged = () => { };
        ContentDialog? settingsDialog = null; // its primary button is "Apply" on the models tab
        var clearKeyBtn = new Button { Content = L10n.T("model.clear_key"), Style = (Style)Application.Current.Resources["PillButtonStyle"] };
        var testBtn = new Button { Content = L10n.T("model.test_connection"), Style = (Style)Application.Current.Resources["PillButtonStyle"] };

        var picker = new ComboBox { Header = L10n.T("model.saved_profiles"), HorizontalAlignment = HorizontalAlignment.Stretch, MinWidth = 0 };
        var presetBox = new ComboBox
        {
            Header = L10n.T("settings.provider_preset_quick_fill"),
            PlaceholderText = L10n.T("settings.choose_a_preset_to_auto"),
            HorizontalAlignment = HorizontalAlignment.Stretch,
        };
        foreach (var prItem in presets)
        {
            var pname = L10n.IsEnglish ? Str(prItem, "name_en") : Str(prItem, "name");
            presetBox.Items.Add(new ComboBoxItem { Content = pname, Tag = Str(prItem, "id") });
        }
        var protocolBox = new ComboBox { Header = L10n.T("model.protocol"), HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var name in Protocols) protocolBox.Items.Add(new ComboBoxItem { Content = name, Tag = name });
        var urlBox = new TextBox { Header = L10n.T("model.base_url"), PlaceholderText = "https://api.example.com/v1" };
        var modelBox = new ComboBox { Header = L10n.T("model.model_id"), IsEditable = true, HorizontalAlignment = HorizontalAlignment.Stretch, MinWidth = 0 };
        var discoverBtn = new Button { Content = L10n.T("model.fetch_models") };
        var keyBox = new PasswordBox { Header = L10n.T("tts.api_key") };
        var allowInsecureHttpBox = new CheckBox { Content = L10n.T("settings.allow_non_loopback_plaintext_http") };
        var activateChatBox = new CheckBox { Content = L10n.T("model.set_as_default_chat_model"), IsChecked = true };
        // read-only: the inputs the core detected for this profile (it probes them itself)
        var detectedBox = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.85 };
        // MiMo-only: shown when the selected preset is "mimo"
        var shrinkMediaCheck = new CheckBox { Content = L10n.T("model.shrink_media") };
        var shrinkMediaHint = new TextBlock { Text = L10n.T("model.shrink_media_hint"), TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.8 };
        var ffmpegStatusText = new TextBlock { Text = L10n.T("ffmpeg.checking"), TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.85 };
        var ffmpegDownloadBtn = new Button { Content = L10n.T("ffmpeg.download_btn"), Style = (Style)Application.Current.Resources["PillButtonStyle"], IsEnabled = false };
        var ffmpegRemoveBtn = new Button { Content = L10n.T("ffmpeg.remove_btn"), Style = (Style)Application.Current.Resources["PillButtonStyle"], Visibility = Visibility.Collapsed };
        var ffmpegButtons = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        ffmpegButtons.Children.Add(ffmpegDownloadBtn);
        ffmpegButtons.Children.Add(ffmpegRemoveBtn);
        var ffmpegRow = new StackPanel { Spacing = 6 };
        ffmpegRow.Children.Add(ffmpegStatusText);
        ffmpegRow.Children.Add(ffmpegButtons);
        var shrinkMediaPanel = new StackPanel { Spacing = 4, Visibility = Visibility.Collapsed };
        shrinkMediaPanel.Children.Add(shrinkMediaCheck);
        shrinkMediaPanel.Children.Add(shrinkMediaHint);
        shrinkMediaPanel.Children.Add(ffmpegRow);
        var audioVideoToggle = new ToggleSwitch { Header = L10n.T("route.enable_native_audio_video_processing"), IsOn = avEnabled };
        var mediaRoutingSummary = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.8 };
        var thinkingBox = new ToggleSwitch { Header = L10n.T("model.show_model_thinking_process"), IsOn = true };
        var effortBox = new ComboBox { Header = L10n.T("model.reasoning_effort"), HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var (label, tag) in new[] { (L10n.T("tts.default_voice"), ""), ("low", "low"), ("medium", "medium"), ("high", "high") })
            effortBox.Items.Add(new ComboBoxItem { Content = label, Tag = tag });
        var ctxBox = new NumberBox { Header = L10n.T("settings.context_window_tokens"), Value = 32000, Minimum = 2000, Maximum = 2_000_000, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        // empty or 0 = unlimited (sent as 0)
        var outBox = new NumberBox { Header = L10n.T("settings.max_output_tokens"), Value = double.NaN, Minimum = 0, Maximum = 200_000, PlaceholderText = L10n.T("settings.unlimited_hint"), SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        var parBox = new NumberBox { Header = L10n.T("settings.max_parallel"), Value = double.NaN, Minimum = 0, Maximum = 64, PlaceholderText = L10n.T("settings.unlimited_hint"), SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact };
        ToolTipService.SetToolTip(parBox, L10n.T("settings.max_parallel_tip"));
        ToolTipService.SetToolTip(outBox, L10n.T("settings.unlimited_hint"));
        var priceIn = new NumberBox { Header = L10n.T("settings.input_price_1m_tokens_optional"), Value = double.NaN, Minimum = 0 };
        var priceOut = new NumberBox { Header = L10n.T("settings.output_price_1m_tokens_optional"), Value = double.NaN, Minimum = 0 };
        var headersRows = new List<(Grid Row, TextBox KeyBox, TextBox ValBox)>();
        var headersListPanel = new StackPanel { Spacing = 6 };
        void AddHeaderRow(string initKey = "", string initVal = "")
        {
            var rowGrid = new Grid { ColumnSpacing = 6 };
            rowGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            rowGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1.4, GridUnitType.Star) });
            rowGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var kBox = new TextBox
            {
                Text = initKey,
                PlaceholderText = L10n.T("settings.header_name_e_g_x"),
            };
            var vBox = new TextBox
            {
                Text = initVal,
                PlaceholderText = L10n.T("model.value"),
            };
            var delBtn = new Button
            {
                Content = new FontIcon { Glyph = "\uE74D", FontSize = 13 },
                MinWidth = 34,
                Height = 32,
                Padding = new Thickness(6, 0, 6, 0),
                VerticalAlignment = VerticalAlignment.Center,
            };
            ToolTipService.SetToolTip(delBtn, L10n.T("common.delete"));
            var entry = (rowGrid, kBox, vBox);
            delBtn.Click += (_, _) =>
            {
                headersRows.Remove(entry);
                headersListPanel.Children.Remove(rowGrid);
                formChanged();
            };
            kBox.TextChanged += (_, _) => formChanged();
            vBox.TextChanged += (_, _) => formChanged();
            Grid.SetColumn(kBox, 0);
            Grid.SetColumn(vBox, 1);
            Grid.SetColumn(delBtn, 2);
            rowGrid.Children.Add(kBox);
            rowGrid.Children.Add(vBox);
            rowGrid.Children.Add(delBtn);
            headersRows.Add(entry);
            headersListPanel.Children.Add(rowGrid);
        }
        void LoadHeaderRows(JsonElement obj)
        {
            headersRows.Clear();
            headersListPanel.Children.Clear();
            if (obj.ValueKind != JsonValueKind.Object) return;
            foreach (var prop in obj.EnumerateObject())
                AddHeaderRow(prop.Name, prop.Value.GetString() ?? "");
            formChanged();
        }
        JsonElement BuildHeadersJson()
        {
            var kv = new List<(string, object?)>();
            foreach (var (_, kBox, vBox) in headersRows)
            {
                var k = kBox.Text.Trim();
                if (k.Length > 0) kv.Add((k, vBox.Text.Trim()));
            }
            return Args(kv.ToArray());
        }
        var addHeaderBtn = new Button
        {
            Content = L10n.T("model.add_header"),
            Style = (Style)Application.Current.Resources["PillButtonStyle"],
            Padding = new Thickness(10, 4, 10, 4),
            FontSize = 12,
        };
        addHeaderBtn.Click += (_, _) => AddHeaderRow();
        var headersHead = new Grid { ColumnSpacing = 8 };
        headersHead.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        headersHead.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        headersHead.Children.Add(new TextBlock
        {
            Text = L10n.T("settings.extra_headers"),
            FontSize = 13,
            VerticalAlignment = VerticalAlignment.Center,
        });
        Grid.SetColumn(addHeaderBtn, 1);
        headersHead.Children.Add(addHeaderBtn);
        var headersBox = new StackPanel { Spacing = 8 };
        headersBox.Children.Add(headersHead);
        headersBox.Children.Add(headersListPanel);
        var caps = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 12, Opacity = 0.8 };
        var status = new TextBlock { TextWrapping = TextWrapping.Wrap };
        var newBtn = new Button { Content = L10n.T("settings.new") };
        var deleteBtn = new Button { Content = L10n.T("settings.delete") };

        // ---- ffmpeg (used by the MiMo shrink option): status, app copy download and removal ----
        var ffmpegBusy = false;
        var ffmpegLast = default(JsonElement); // last ffmpeg.status / install / remove result
        void SyncFfmpegButtons()
        {
            var fromApp = Str(ffmpegLast, "source") == "app";
            ffmpegDownloadBtn.IsEnabled = !ffmpegBusy && !Bool(ffmpegLast, "installed") && Bool(ffmpegLast, "supported");
            ffmpegRemoveBtn.Visibility = fromApp ? Visibility.Visible : Visibility.Collapsed;
            ffmpegRemoveBtn.IsEnabled = !ffmpegBusy && fromApp;
        }
        void ShowFfmpeg(JsonElement s)
        {
            ffmpegLast = s;
            var installed = Bool(s, "installed");
            var source = Str(s, "source");
            var label = !installed ? L10n.T("ffmpeg.not_installed")
                : source == "app" ? L10n.T("ffmpeg.installed_app")
                : source == "system" ? L10n.T("ffmpeg.installed_system")
                : L10n.T("ffmpeg.installed");
            var version = Str(s, "version");
            if (installed && version.Length > 0) label += " · " + version;
            if (!installed && !Bool(s, "supported")) label += " · " + L10n.T("ffmpeg.unsupported");
            // while a download runs its progress line stays
            if (!ffmpegBusy) ffmpegStatusText.Text = label;
            SyncFfmpegButtons();
        }
        async Task RefreshFfmpegAsync()
        {
            var (s, err) = await Api.CallAsync("ffmpeg.status", Args());
            if (s is { } st) ShowFfmpeg(st);
            else ffmpegStatusText.Text = L10n.Fmt("ffmpeg.op_failed", err?.Message ?? "");
        }
        async Task InstallFfmpegAsync()
        {
            if (ffmpegBusy) return;
            ffmpegBusy = true;
            ffmpegStatusText.Text = L10n.T("ffmpeg.downloading_starting");
            SyncFfmpegButtons();
            // ffmpeg.install holds the call until the download finishes, so it runs off the UI thread;
            // progress arrives as "ffmpeg.download_progress" events (OnFfmpegEvent)
            _ = Task.Run(async () =>
            {
                var (s, err) = await Api.CallAsync("ffmpeg.install", Args());
                DispatcherQueue.TryEnqueue(() =>
                {
                    ffmpegBusy = false;
                    if (s is { } st) ShowFfmpeg(st);
                    else
                    {
                        SyncFfmpegButtons();
                        ffmpegStatusText.Text = L10n.Fmt("ffmpeg.op_failed", err?.Message ?? "");
                    }
                });
            });
        }
        void OnFfmpegEvent(Interop.ApiEventDto ev)
        {
            if (ev.Event != "ffmpeg.download_progress") return;
            var done = Int(ev.Data, "downloaded");
            var total = Int(ev.Data, "total"); // 0 when the size is unknown
            DispatcherQueue.TryEnqueue(() =>
            {
                if (!ffmpegBusy) return;
                ffmpegStatusText.Text = total > 0
                    ? L10n.Fmt("ffmpeg.downloading", done / 1_000_000.0, total / 1_000_000.0)
                    : L10n.Fmt("ffmpeg.downloading_unknown", done / 1_000_000.0);
            });
        }
        ffmpegDownloadBtn.Click += async (_, _) => await InstallFfmpegAsync();
        ffmpegRemoveBtn.Click += async (_, _) =>
        {
            if (ffmpegBusy) return;
            var (s, err) = await Api.CallAsync("ffmpeg.remove", Args());
            if (s is { } st) ShowFfmpeg(st);
            else ffmpegStatusText.Text = L10n.Fmt("ffmpeg.op_failed", err?.Message ?? "");
        };

        void UpdateMediaSummary()
        {
            var chatModel = profiles.FirstOrDefault(p => Bool(p, "is_active"));
            string PickMod(string explicitId, string capKey, string emptyLabel)
            {
                if (!string.IsNullOrEmpty(explicitId))
                {
                    var exp = profiles.FirstOrDefault(p => Str(p, "id") == explicitId && Bool(Obj(p, "capabilities"), capKey));
                    if (exp.ValueKind == JsonValueKind.Object) return Str(exp, "display_name");
                }
                if (chatModel.ValueKind == JsonValueKind.Object && Bool(Obj(chatModel, "capabilities"), capKey))
                    return Str(chatModel, "display_name");
                var m = profiles.FirstOrDefault(p => Bool(Obj(p, "capabilities"), capKey));
                return m.ValueKind == JsonValueKind.Object ? Str(m, "display_name") : emptyLabel;
            }
            var chatName = chatModel.ValueKind == JsonValueKind.Object ? Str(chatModel, "display_name") : (L10n.T("route.not_configured"));
            var routeOff = L10n.T("route.disabled");
            mediaRoutingSummary.Text = L10n.Fmt("route.summary",
                chatName,
                PickMod(visionProfileId, "supports_vision", L10n.T("route.vision_none")),
                audioVideoToggle.IsOn ? PickMod(audioProfileId, "supports_audio", L10n.T("route.unassigned")) : routeOff,
                audioVideoToggle.IsOn ? PickMod(videoProfileId, "supports_video", L10n.T("route.unassigned")) : routeOff);
        }

        async Task SaveMediaSettingsAsync()
        {
            var args = new List<(string, object?)> { ("audio_video_enabled", audioVideoToggle.IsOn) };
            if (!string.IsNullOrEmpty(visionProfileId)) args.Add(("vision_profile_id", visionProfileId));
            if (!string.IsNullOrEmpty(audioProfileId)) args.Add(("audio_profile_id", audioProfileId));
            if (!string.IsNullOrEmpty(videoProfileId)) args.Add(("video_profile_id", videoProfileId));
            await Api.CallAsync("settings.set", Args(("key", "media_processing"), ("value", Args(args.ToArray()))));
        }

        // "工具调用 ✓ · 结构化输出 ✓ · 识图" or the reason the profile cannot be used
        string CapsLine(JsonElement prof)
        {
            var c = prof.TryGetProperty("capabilities", out var cc) ? cc : prof;
            if (c.ValueKind != JsonValueKind.Object) return "";
            var reason = Str(prof, "unsupported_reason");
            if (reason.Length > 0) return "⚠ " + reason;
            if (Bool(c, "embedding_only")) return L10n.T("model.embedding_only");
            if (!c.TryGetProperty("probed_at", out var pa) || pa.ValueKind != JsonValueKind.Number)
                return L10n.T("model.probe_pending");
            var parts = new List<string>
            {
                L10n.T("model.cap_tools") + (Bool(c, "tools") ? " ✓" : " ✗"),
                L10n.T("model.cap_structured") + (Bool(c, "json_schema") || Bool(c, "json_mode") ? " ✓" : " ✗"),
            };
            return string.Join(" · ", parts);
        }

        string CurrentModelId() =>
            (string.IsNullOrWhiteSpace(modelBox.Text) ? modelBox.SelectedItem as string : modelBox.Text)?.Trim() ?? "";
        // id of the selected preset, "" when none
        string PresetId() => (presetBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "";
        void SetPreset(string id)
        {
            suppressPresetFill = true;
            try { presetBox.SelectedItem = presetBox.Items.OfType<ComboBoxItem>().FirstOrDefault(i => i.Tag as string == id); }
            finally { suppressPresetFill = false; }
        }
        void UpdateShrinkVisibility() =>
            shrinkMediaPanel.Visibility = PresetId() == "mimo" ? Visibility.Visible : Visibility.Collapsed;
        static string Num(double v) => double.IsNaN(v) ? "" : v.ToString(System.Globalization.CultureInfo.InvariantCulture);
        // a limit box: empty, 0 or anything below 1 means unlimited (0); otherwise the value, capped at max
        static int LimitArg(double v, double max = int.MaxValue) => !double.IsFinite(v) || v < 1 ? 0 : (int)Math.Min(v, max);
        // a stored limit: 0 (or missing) shows as an empty box
        static double LimitValue(JsonElement prof, string name) { var v = Int(prof, name, 0); return v > 0 ? v : double.NaN; }
        string FormSnapshot() => string.Join("", new[]
        {
            (protocolBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "",
            urlBox.Text.Trim(), CurrentModelId(), keyBox.Password.Length > 0 ? "key" : "",
            allowInsecureHttpBox.IsChecked == true ? "1" : "0", activateChatBox.IsChecked == true ? "1" : "0",
            thinkingBox.IsOn ? "1" : "0", (effortBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "",
            Num(ctxBox.Value), LimitArg(outBox.Value).ToString(System.Globalization.CultureInfo.InvariantCulture),
            LimitArg(parBox.Value, 64).ToString(System.Globalization.CultureInfo.InvariantCulture), Num(priceIn.Value), Num(priceOut.Value),
            BuildHeadersJson().GetRawText(),
            PresetId(), shrinkMediaCheck.IsChecked == true ? "1" : "0",
        });
        bool FormDirty() => profileId == null || FormSnapshot() != savedForm;
        void SyncModelButtons()
        {
            var complete = urlBox.Text.Trim().Length > 0 && CurrentModelId().Length > 0;
            if (settingsDialog != null) settingsDialog.IsPrimaryButtonEnabled = !modelBusy && complete && FormDirty();
            testBtn.IsEnabled = !modelBusy && profileId != null;
            clearKeyBtn.IsEnabled = !modelBusy && profileId != null && hasSecret;
            deleteBtn.IsEnabled = !modelBusy && profileId != null;
        }
        void MarkFormSaved() { savedForm = FormSnapshot(); SyncModelButtons(); }
        formChanged = SyncModelButtons;

        void Fill(JsonElement? p)
        {
            FillForm(p);
            UpdateShrinkVisibility();
            MarkFormSaved();
        }
        void FillForm(JsonElement? p)
        {
            if (p is not { } prof)
            {
                profileId = null; keychainRef = ""; hasSecret = false;
                protocolBox.SelectedIndex = 0;
                urlBox.Text = ""; modelBox.Items.Clear(); modelBox.Text = "";
                allowInsecureHttpBox.IsChecked = false;
                activateChatBox.IsChecked = !profiles.Any(x => Bool(x, "is_active"));
                SetPreset(""); shrinkMediaCheck.IsChecked = false;
                thinkingBox.IsOn = true; effortBox.SelectedIndex = 0;
                ctxBox.Value = 32000; outBox.Value = double.NaN; parBox.Value = double.NaN; priceIn.Value = double.NaN; priceOut.Value = double.NaN;
                LoadHeaderRows(default); caps.Text = "";
                detectedBox.Text = L10n.T("model.detected_not_yet");
                keyBox.Password = "";
                keyBox.PlaceholderText = L10n.T("model.enter_api_key_optional_for");
                status.Text = L10n.T("settings.editing_new_profile_fill_in");
                return;
            }
            profileId = Str(prof, "id");
            keychainRef = Str(prof, "keychain_ref");
            hasSecret = Bool(prof, "has_secret");
            SetPreset(Str(prof, "preset"));
            shrinkMediaCheck.IsChecked = Bool(prof, "shrink_media");
            var proto = Str(prof, "protocol");
            protocolBox.SelectedIndex = Math.Max(0, Array.IndexOf(Protocols, proto));
            urlBox.Text = Str(prof, "base_url");
            // the profile was saved with this URL, so a plaintext one was confirmed back then
            allowInsecureHttpBox.IsChecked = urlBox.Text.StartsWith("http://", StringComparison.OrdinalIgnoreCase)
                && !IsLocalEndpoint(urlBox.Text);
            var savedModel = Str(prof, "model_id");
            modelBox.Items.Clear();
            if (savedModel.Length > 0) { modelBox.Items.Add(savedModel); modelBox.SelectedIndex = 0; }
            modelBox.Text = savedModel;
            activateChatBox.IsChecked = Bool(prof, "is_active");
            thinkingBox.IsOn = !prof.TryGetProperty("show_thinking", out var st) || st.ValueKind != JsonValueKind.False;
            var effort = Str(prof, "effort");
            effortBox.SelectedIndex = Math.Max(0, new[] { "", "low", "medium", "high" }.ToList().IndexOf(effort));
            ctxBox.Value = Int(prof, "context_window", 32000);
            outBox.Value = LimitValue(prof, "max_output_tokens");
            parBox.Value = LimitValue(prof, "max_parallel");
            priceIn.Value = prof.TryGetProperty("input_price_per_m", out var pi) && pi.ValueKind == JsonValueKind.Number ? pi.GetDouble() : double.NaN;
            priceOut.Value = prof.TryGetProperty("output_price_per_m", out var po) && po.ValueKind == JsonValueKind.Number ? po.GetDouble() : double.NaN;
            LoadHeaderRows(Obj(prof, "extra_headers"));
            keyBox.Password = "";
            keyBox.PlaceholderText = hasSecret
                ? (L10n.T("settings.saved_leave_blank_to_keep"))
                : (L10n.T("model.enter_api_key_optional_for"));
            caps.Text = CapsLine(prof);
            ShowDetected(prof);
            status.Text = hasSecret
                ? (L10n.T("settings.key_status_saved_in_windows"))
                : (L10n.T("model.key_status_not_saved"));
        }

        // Rebuilding the list re-selects the current profile; that must not run Fill() and
        // overwrite whatever has been typed into the form since the profile was loaded.
        var rebuildingPicker = false;
        void LoadPicker(string? select)
        {
            rebuildingPicker = true;
            try { RebuildPicker(select); }
            finally { rebuildingPicker = false; }
        }
        // picker label: name, default/disabled tags and the detected input tags
        string PickerLabel(JsonElement p)
        {
            var c = Obj(p, "capabilities");
            var mods = new List<string>();
            if (Bool(c, "supports_vision")) mods.Add(L10n.T("model.vision"));
            if (Bool(c, "supports_audio")) mods.Add(L10n.T("model.audio"));
            if (Bool(c, "supports_video")) mods.Add(L10n.T("model.video"));
            var modTag = mods.Count > 0 ? $" [{string.Join("/", mods)}]" : "";
            var defTag = Bool(p, "is_active") ? (L10n.T("model.default")) : "";
            if (p.TryGetProperty("eligible", out var el) && el.ValueKind == JsonValueKind.False)
                defTag += L10n.T("model.disabled_tag");
            return $"{Str(p, "display_name")}{defTag}{modTag}";
        }
        void RebuildPicker(string? select)
        {
            picker.Items.Clear();
            foreach (var p in profiles)
            {
                var item = new ComboBoxItem { Content = PickerLabel(p), Tag = Str(p, "id") };
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
            if (rebuildingPicker) return;
            if (picker.SelectedItem is ComboBoxItem { Tag: string id })
            {
                var p = profiles.FirstOrDefault(x => Str(x, "id") == id);
                if (p.ValueKind == JsonValueKind.Object) Fill(p);
            }
        };
        newBtn.Click += (_, _) => { picker.SelectedItem = null; Fill(null); };
        // Picking a preset auto-fills the form but stays selected: it is saved with the profile
        // and is not cleared by later edits. Selection made by Fill() does not auto-fill.
        presetBox.SelectionChanged += (_, _) =>
        {
            UpdateShrinkVisibility();
            if (suppressPresetFill) return;
            var prItem = presetBox.SelectedItem is ComboBoxItem { Tag: string pid }
                ? presets.FirstOrDefault(x => Str(x, "id") == pid)
                : default;
            if (prItem.ValueKind == JsonValueKind.Object) ApplyPresetFields(prItem);
            formChanged();
        };
        void ApplyPresetFields(JsonElement prItem)
        {
            var proto = Str(prItem, "protocol");
            protocolBox.SelectedIndex = Math.Max(0, Array.IndexOf(Protocols, proto));
            urlBox.Text = Str(prItem, "base_url");
            modelBox.Items.Clear();
            foreach (var sm in Arr(prItem, "suggested_models"))
            {
                var s = sm.GetString();
                if (!string.IsNullOrEmpty(s)) modelBox.Items.Add(s);
            }
            var defModel = Str(prItem, "default_model");
            if (defModel.Length > 0)
            {
                if (modelBox.Items.Count > 0) modelBox.SelectedIndex = 0;
                modelBox.Text = defModel;
            }
            ctxBox.Value = Int(prItem, "context_window", 32000);
            outBox.Value = LimitValue(prItem, "max_output_tokens");
            parBox.Value = double.NaN;
            LoadHeaderRows(Obj(prItem, "extra_headers"));
            var pname = L10n.IsEnglish ? Str(prItem, "name_en") : Str(prItem, "name");
            status.Text = L10n.Fmt("model.applied_preset_enter_api_key", pname);
        }
        // a second click within a few seconds confirms (no nested dialog inside this one)
        string? deleteArmedFor = null;
        deleteBtn.Click += async (_, _) =>
        {
            if (profileId == null || modelBusy) return;
            if (deleteArmedFor != profileId)
            {
                var armed = deleteArmedFor = profileId;
                deleteBtn.Content = L10n.T("model.confirm_delete");
                await Task.Delay(4000);
                if (deleteArmedFor == armed) { deleteArmedFor = null; deleteBtn.Content = L10n.T("settings.delete"); }
                return;
            }
            deleteArmedFor = null;
            deleteBtn.Content = L10n.T("settings.delete");
            modelBusy = true; SyncModelButtons();
            try
            {
                if (await CoreAsync("profile.delete", Args(("profile_id", profileId)), L10n.T("common.delete")) == null) return;
                profiles.RemoveAll(x => Str(x, "id") == profileId);
                LoadPicker(null);
                Fill(null);
                status.Text = L10n.T("model.profile_deleted");
            }
            finally { modelBusy = false; SyncModelButtons(); }
        };
        // every edit re-evaluates whether there is something to apply
        urlBox.TextChanged += (_, _) => formChanged();
        modelBox.SelectionChanged += (_, _) => formChanged();
        modelBox.TextSubmitted += (_, _) => formChanged();
        modelBox.KeyUp += (_, _) => formChanged();
        modelBox.LostFocus += (_, _) => formChanged();
        keyBox.PasswordChanged += (_, _) => formChanged();
        protocolBox.SelectionChanged += (_, _) => formChanged();
        effortBox.SelectionChanged += (_, _) => formChanged();
        foreach (var cb in new[] { allowInsecureHttpBox, activateChatBox, shrinkMediaCheck })
        {
            cb.Checked += (_, _) => formChanged();
            cb.Unchecked += (_, _) => formChanged();
        }
        thinkingBox.Toggled += (_, _) => formChanged();
        foreach (var nb in new[] { ctxBox, outBox, parBox, priceIn, priceOut })
            nb.ValueChanged += (_, _) => formChanged();
        discoverBtn.Click += async (_, _) =>
        {
            var baseUrl = urlBox.Text.Trim();
            if (string.IsNullOrEmpty(baseUrl))
            {
                status.Text = L10n.T("model.please_enter_base_url_first");
                return;
            }
            var protocol = (protocolBox.SelectedItem as ComboBoxItem)?.Tag as string ?? Protocols[0];
            var apiKey = keyBox.Password.Trim();
            var res = await CoreAsync("profile.discover_models", Args(("profile_id", profileId ?? ""), ("base_url", baseUrl), ("protocol", protocol), ("api_key", apiKey)), L10n.T("model.fetch_models"));
            if (res is not { } r) return;
            var current = modelBox.Text;
            modelBox.Items.Clear();
            foreach (var m in Arr(r, "models")) modelBox.Items.Add(Str(m, "id"));
            modelBox.Text = current;
            status.Text = L10n.Fmt("settings.found_models", modelBox.Items.Count);
        };

        // Shows what the core detected for the stored `prof`; "not yet" until it has been probed.
        void ShowDetected(JsonElement prof)
        {
            var c = Obj(prof, "capabilities");
            if (c.ValueKind != JsonValueKind.Object || !c.TryGetProperty("probed_at", out var pa) || pa.ValueKind != JsonValueKind.Number)
            {
                detectedBox.Text = L10n.T("model.detected_not_yet");
                return;
            }
            ShowDetectedFlags(Bool(c, "supports_vision"), Bool(c, "supports_audio"), Bool(c, "supports_video"));
        }
        void ShowDetectedFlags(bool vision, bool audio, bool video) =>
            detectedBox.Text = L10n.Fmt("model.detected_inputs", vision ? "✓" : "✗", audio ? "✓" : "✗", video ? "✓" : "✗");

        async Task ReloadProfilesAsync()
        {
            var relist = await CoreAsync("profile.list", Args(), L10n.T("settings.tab_models"));
            if (relist is not { } rl) return;
            profiles = Arr(rl, "profiles").ToList();
            LoadPicker(profileId);
        }

        // profile.updated (the core finished a background probe or profile.test): re-read that one
        // profile and refresh only read-only views. Form fields, savedForm and dirty state are untouched.
        async Task RefreshProfileReadOnlyAsync(string id)
        {
            var (res, _) = await Api.CallAsync("profile.list", Args());
            if (res is not { } rl) return;
            var fresh = Arr(rl, "profiles").FirstOrDefault(x => Str(x, "id") == id);
            if (fresh.ValueKind != JsonValueKind.Object) return;
            var idx = profiles.FindIndex(x => Str(x, "id") == id);
            if (idx < 0) return;
            profiles[idx] = fresh;
            // picker tags and the media routing summary; LoadPicker's guard keeps Fill() from running
            LoadPicker(profileId);
            // the edited profile's own read-only lines (profileId is read now, after the await)
            if (id == profileId)
            {
                caps.Text = CapsLine(fresh);
                ShowDetected(fresh);
            }
        }
        void OnProfileUpdatedEvent(Interop.ApiEventDto ev)
        {
            if (ev.Event != "profile.updated" || ev.Topic != "profiles") return;
            var id = Str(ev.Data, "profile_id");
            if (id.Length == 0) return;
            DispatcherQueue.TryEnqueue(async () =>
            {
                try { await RefreshProfileReadOnlyAsync(id); }
                catch (Exception ex) { ShowStatus(InfoBarSeverity.Error, L10n.T("app.ui_refresh_failed"), ex.Message); }
            });
        }

        async Task SaveModelAsync()
        {
            if (modelBusy) return;
            var url = urlBox.Text.Trim();
            var modelId = CurrentModelId();
            if (url.Length == 0 || modelId.Length == 0)
            {
                status.Text = L10n.T("model.please_fill_in_base_url");
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
                ("context_window", double.IsNaN(ctxBox.Value) ? 32000 : (int)ctxBox.Value),
                ("max_output_tokens", LimitArg(outBox.Value)),
                ("max_parallel", LimitArg(parBox.Value, 64)),
                ("extra_headers", BuildHeadersJson()),
                ("preset", PresetId()),
                ("shrink_media", PresetId() == "mimo" && shrinkMediaCheck.IsChecked == true),
            };
            var effort = (effortBox.SelectedItem as ComboBoxItem)?.Tag as string ?? "";
            if (effort.Length > 0) up.Add(("effort", effort));
            if (!double.IsNaN(priceIn.Value)) up.Add(("input_price_per_m", priceIn.Value));
            if (!double.IsNaN(priceOut.Value)) up.Add(("output_price_per_m", priceOut.Value));
            if (profileId != null) up.Add(("id", profileId));
            // saving only stores; capabilities are measured by "test connection"
            if (keyBox.Password.Length > 0) up.Add(("api_key", keyBox.Password));
            status.Text = L10n.T("model.probing");
            modelBusy = true; SyncModelButtons();
            try
            {
                var (saved, saveErr) = await Api.CallAsync("profile.upsert", Args(up.ToArray()));
                if (saveErr is { } || saved is not { } sv)
                {
                    status.Text = saveErr is { Code: "CAPABILITY_MISSING" } ce
                        ? L10n.Fmt("model.refused", L10n.CapabilityMissing(ce.Message))
                        : L10n.Fmt("model.save_failed", saveErr?.Message ?? "");
                    return;
                }
                profileId = Str(sv, "id");
                keychainRef = Str(sv, "keychain_ref");
                hasSecret = Bool(sv, "has_secret");
                keyBox.Password = string.Empty;
                keyBox.PlaceholderText = hasSecret
                    ? (L10n.T("settings.saved_leave_blank_to_keep"))
                    : (L10n.T("model.enter_api_key_optional_for"));
                caps.Text = CapsLine(sv);
                if (makeActive) await CoreAsync("profile.activate", Args(("profile_id", profileId)), L10n.T("common.apply"));
                await SaveMediaSettingsAsync();
                await ReloadProfilesAsync();
                // the form now matches what is stored; Apply stays off until something changes
                MarkFormSaved();
                status.Text = L10n.T("model.saved_checked");
            }
            finally
            {
                modelBusy = false; SyncModelButtons();
            }
        }

        clearKeyBtn.Click += async (_, _) =>
        {
            if (modelBusy) return;
            if (profileId == null || keychainRef.Length == 0) { status.Text = L10n.T("settings.no_key_to_clear"); return; }
            modelBusy = true; SyncModelButtons();
            try
            {
                if (await CoreAsync("secret.clear", Args(("keychain_ref", keychainRef)), L10n.T("model.clear_key")) == null) return;
                hasSecret = false;
                keyBox.PlaceholderText = L10n.T("model.enter_api_key_optional_for");
                status.Text = L10n.T("model.api_key_cleared");
                await ReloadProfilesAsync();
            }
            finally { modelBusy = false; SyncModelButtons(); }
        };

        testBtn.Click += async (_, _) =>
        {
            if (modelBusy) return;
            if (profileId == null) { status.Text = L10n.T("settings.save_the_profile_before_testing"); return; }
            // the test runs against what is stored, so unapplied edits would not be tested
            if (FormDirty()) { status.Text = L10n.T("model.apply_before_test"); return; }
            status.Text = L10n.T("model.testing_connection");
            modelBusy = true; SyncModelButtons();
            try
            {
                var (test, testErr) = await Api.CallAsync("profile.test", Args(("profile_id", profileId)));
                var detected = default(JsonElement);
                if (test is { } tr)
                {
                    detected = Obj(tr, "detected");
                    var msg = L10n.Fmt("model.connected", Str(tr, "reply"));
                    foreach (var f in Arr(tr, "media_failed"))
                    {
                        var kind = Str(f, "kind") switch
                        {
                            "vision" => L10n.T("settings.vision"),
                            "audio" => L10n.T("model.audio"),
                            _ => L10n.T("model.video"),
                        };
                        msg += "\n" + L10n.Fmt("model.media_probe_failed", kind, Str(f, "detail"));
                    }
                    status.Text = msg;
                }
                else
                {
                    status.Text = testErr is { Code: "CAPABILITY_MISSING" } ce
                        ? L10n.Fmt("model.refused", L10n.CapabilityMissing(ce.Message))
                        : (L10n.Fmt("model.connection_test_failed", testErr?.Message ?? ""));
                }
                // the test result carries what was just detected; otherwise show the stored profile
                // (the form was unchanged before the test, so it stays "applied")
                await ReloadProfilesAsync();
                var p = profiles.FirstOrDefault(x => Str(x, "id") == profileId);
                if (p.ValueKind == JsonValueKind.Object) caps.Text = CapsLine(p);
                if (detected.ValueKind == JsonValueKind.Object)
                    ShowDetectedFlags(Bool(detected, "vision"), Bool(detected, "audio"), Bool(detected, "video"));
                else if (p.ValueKind == JsonValueKind.Object) ShowDetected(p);
                MarkFormSaved();
            }
            finally { modelBusy = false; SyncModelButtons(); }
        };

        audioVideoToggle.Toggled += async (_, _) =>
        {
            await SaveMediaSettingsAsync();
            UpdateMediaSummary();
        };

        var modelRow = new Grid { ColumnSpacing = 8 };
        modelRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star), MinWidth = 0 });
        modelRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        discoverBtn.VerticalAlignment = VerticalAlignment.Bottom;
        Grid.SetColumn(discoverBtn, 1);
        modelRow.Children.Add(modelBox);
        modelRow.Children.Add(discoverBtn);

        var top = new Grid { ColumnSpacing = 8 };
        top.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star), MinWidth = 0 });
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
        advanced.Children.Add(parBox);
        advanced.Children.Add(priceIn);
        advanced.Children.Add(priceOut);
        advanced.Children.Add(headersBox);
        advanced.Children.Add(caps);

        var modalityBox = new StackPanel { Spacing = 4 };
        modalityBox.Children.Add(detectedBox);
        modalityBox.Children.Add(shrinkMediaPanel);

        var mediaSettingsBox = new StackPanel { Spacing = 8, Padding = new Thickness(10), CornerRadius = new CornerRadius(8), Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"] };
        mediaSettingsBox.Children.Add(audioVideoToggle);
        mediaSettingsBox.Children.Add(mediaRoutingSummary);

        var modelActions = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        modelActions.Children.Add(testBtn);
        modelActions.Children.Add(clearKeyBtn);

        var modelsPanel = new StackPanel { Spacing = 10, Padding = new Thickness(4, 8, 4, 8) };
        modelsPanel.Children.Add(modelActions);
        modelsPanel.Children.Add(status);
        modelsPanel.Children.Add(top);
        modelsPanel.Children.Add(presetBox);
        modelsPanel.Children.Add(protocolBox);
        modelsPanel.Children.Add(urlBox);
        modelsPanel.Children.Add(modelRow);
        modelsPanel.Children.Add(keyBox);
        modelsPanel.Children.Add(allowInsecureHttpBox);
        modelsPanel.Children.Add(activateChatBox);
        modelsPanel.Children.Add(modalityBox);
        modelsPanel.Children.Add(new Expander { Header = L10n.T("model.advanced_options"), Content = advanced, HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch });
        modelsPanel.Children.Add(mediaSettingsBox);

        // ---- Tab 4: Semantic Search (Embeddings) ----
        var embStatus = new TextBlock { TextWrapping = TextWrapping.Wrap };
        var embProgress = new ProgressBar { Maximum = 100, Visibility = Visibility.Collapsed };
        var embDownload = new Button { Content = L10n.T("emb.download_enable"), Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"] };
        var embCancel = new Button { Content = L10n.T("common.cancel"), Visibility = Visibility.Collapsed };
        var embRebuild = new Button { Content = L10n.T("emb.rebuild_index"), Style = (Style)Application.Current.Resources["PillButtonStyle"] };
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
                embStatus.Text = L10n.Fmt("emb.active_model_indexed_chunks", Str(activeModel, "model_name"), done, total);
            else
                embStatus.Text = Bool(d, "installed")
                    ? (L10n.T("emb.model_downloaded_but_not_enabled"))
                    : (L10n.Fmt("emb.status_not_enabled", mb));
            embDownload.Content = Bool(d, "installed")
                ? (L10n.T("emb.enable_downloaded_model"))
                : (L10n.Fmt("emb.download_size", mb));
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
            if (r is { } rr) embStatus.Text = L10n.Fmt("emb.queued_rebuild", Int(rr, "sources_queued"));
        };
        await RefreshEmbAsync();
        _embRefresh = RefreshEmbAsync;

        var embRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        embRow.Children.Add(embDownload);
        embRow.Children.Add(embCancel);
        embRow.Children.Add(embRebuild);

        var embPanel = new StackPanel { Spacing = 10, Padding = new Thickness(4, 8, 4, 8) };
        embPanel.Children.Add(Line(L10n.T("settings.semantic_search_finds_relevant_passages")));
        embPanel.Children.Add(embStatus);
        embPanel.Children.Add(embProgress);
        embPanel.Children.Add(embRow);

        var pivot = new Pivot { MinWidth = 540 };
        pivot.Items.Add(new PivotItem { Header = L10n.T("settings.tab_appearance"), Content = new ScrollViewer { Content = appearancePanel, MaxHeight = 560, HorizontalScrollMode = ScrollMode.Disabled, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled } });
        pivot.Items.Add(new PivotItem { Header = L10n.T("settings.tab_general"), Content = new ScrollViewer { Content = generalPanel, MaxHeight = 560, HorizontalScrollMode = ScrollMode.Disabled, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled } });
        pivot.Items.Add(new PivotItem { Header = L10n.T("settings.tab_models"), Content = new ScrollViewer { Content = modelsPanel, MaxHeight = 560, HorizontalScrollMode = ScrollMode.Disabled, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled } });
        pivot.Items.Add(new PivotItem { Header = L10n.T("settings.tab_embedding"), Content = new ScrollViewer { Content = embPanel, MaxHeight = 560, HorizontalScrollMode = ScrollMode.Disabled, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled } });

        var dialog = new ContentDialog
        {
            Title = L10n.T("settings.title"),
            Content = pivot,
            CloseButtonText = L10n.T("common.close"),
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = Content.XamlRoot,
        };
        settingsDialog = dialog;
        // the dialog's default maximum width (548) is narrower than the tabs; wider content was cut off
        dialog.Resources["ContentDialogMaxWidth"] = 760d;
        // "Apply" sits with the dialog buttons and only applies to the models tab
        void SyncSaveButton()
        {
            dialog.PrimaryButtonText = pivot.SelectedIndex == 2 ? L10n.T("common.apply") : "";
            SyncModelButtons();
        }
        pivot.SelectionChanged += (_, _) =>
        {
            SyncSaveButton();
            if (pivot.SelectedIndex == 2) _ = RefreshFfmpegAsync(); // models tab
        };
        SyncSaveButton();
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true; // keep the dialog open
            var deferral = args.GetDeferral();
            try { await SaveModelAsync(); }
            finally
            {
                deferral.Complete();
                // ContentDialog restores its own button state when the deferral ends
                DispatcherQueue.TryEnqueue(SyncModelButtons);
            }
        };

        // ffmpeg download progress arrives as core events while the dialog is open
        Action<Interop.ApiEventDto> onFfmpegEvent = OnFfmpegEvent;
        // profile.updated refreshes the read-only model views while the dialog is open
        Action<Interop.ApiEventDto> onProfileEvent = OnProfileUpdatedEvent;
        App.Core.EventReceived += onFfmpegEvent;
        App.Core.EventReceived += onProfileEvent;
        try { await dialog.ShowAsync(); }
        finally
        {
            App.Core.EventReceived -= onFfmpegEvent;
            App.Core.EventReceived -= onProfileEvent;
        }
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
        panel.Children.Add(Line(L10n.T("privacy.local_storage")));
        panel.Children.Add(Line(L10n.T("privacy.provider_send")));
        panel.Children.Add(Line(L10n.T("privacy.api_keys")));
        panel.Children.Add(Line(L10n.T("privacy.diagnostics")));
        var dlg = new ContentDialog
        {
            Title = L10n.T("nb.privacy"),
            Content = panel,
            CloseButtonText = needConfirm ? L10n.T("common.cancel") : L10n.T("common.close"),
            XamlRoot = Content.XamlRoot,
        };
        if (needConfirm)
        {
            dlg.PrimaryButtonText = L10n.T("dlg.got_it_continue");
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
        panel.Children.Add(Line(L10n.T("onboarding.welcome")));
        panel.Children.Add(Line(L10n.T("onboarding.step1")));
        panel.Children.Add(Line(L10n.T("onboarding.step2")));
        panel.Children.Add(Line(L10n.T("onboarding.step3")));
        var dlg = new ContentDialog
        {
            Title = L10n.T("dlg.quick_start"),
            Content = panel,
            PrimaryButtonText = L10n.T("dlg.open_settings"),
            CloseButtonText = L10n.T("dlg.later"),
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = Content.XamlRoot,
        };
        await Api.CallAsync("settings.set", Args(("key", "onboarding.done"), ("value", true)));
        if (await dlg.ShowAsync() == ContentDialogResult.Primary) OnOpenSettingsClick(this, new RoutedEventArgs());
    }
}
