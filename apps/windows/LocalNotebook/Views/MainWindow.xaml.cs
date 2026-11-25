using LocalNotebook.Interop;
using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Text.Json;
using Windows.Storage.Pickers;
using WinRT.Interop;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

public sealed partial class MainWindow : Window
{
    private static readonly string[] Protocols = { "chat_completion", "responses", "anthropic", "gemini" };

    private string? _activeNotebookId;
    private string _activeNotebookTitle = "";
    private bool _loadingPicker;
    private bool _refreshing;
    private bool _refreshAgain;
    private readonly HashSet<string> _autoTranslatedSummaries = new();

    public MainWindow()
    {
        InitializeComponent();
        ApplyLocalization();
        InitChatImages();
        App.Core.EventReceived += OnCoreEvent;
        Closed += (_, _) =>
        {
            _activeArtifactPane?.Dispose();
            _math.Dispose();
            App.Core.Dispose();
        };
    }

    // ---------- helpers ----------

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
        var (result, err) = await Api.CallAsync(method, args);
        if (err is { } e)
        {
            var advice = Advice(e.Code);
            // a model refused for missing tool calling / structured output says exactly what is missing
            var detail = e.Code == "CAPABILITY_MISSING" && e.Message.Length > 0 ? e.Message : (advice.Length > 0 ? advice : e.Message);
            ShowStatus(InfoBarSeverity.Error, L10n.Fmt("common.failed", what), detail);
            return null;
        }
        return result;
    }

    private static TextBlock Line(string text) => new()
    {
        Text = text,
        TextWrapping = TextWrapping.Wrap,
        IsTextSelectionEnabled = true,
    };

    private async Task<string?> PromptTextAsync(string title, string label, string initial = "", bool multiline = false, string? primary = null)
    {
        var box = new TextBox
        {
            Header = label,
            Text = initial,
            AcceptsReturn = multiline,
            TextWrapping = multiline ? TextWrapping.Wrap : TextWrapping.NoWrap,
            MinWidth = 420,
            MinHeight = multiline ? 220 : 0,
        };
        var dlg = new ContentDialog { Title = title, Content = box, PrimaryButtonText = primary ?? L10n.T("common.ok"), CloseButtonText = L10n.T("common.cancel"), DefaultButton = ContentDialogButton.Primary, XamlRoot = Content.XamlRoot };
        return await dlg.ShowAsync() == ContentDialogResult.Primary ? box.Text : null;
    }

    private async Task<bool> ConfirmAsync(string title, string message, string? primary = null)
    {
        var dlg = new ContentDialog { Title = title, Content = Line(message), PrimaryButtonText = primary ?? L10n.T("common.ok"), CloseButtonText = L10n.T("common.cancel"), DefaultButton = ContentDialogButton.Close, XamlRoot = Content.XamlRoot };
        return await dlg.ShowAsync() == ContentDialogResult.Primary;
    }

    private async Task InfoAsync(string title, UIElement content)
    {
        var dlg = new ContentDialog { Title = title, Content = content, CloseButtonText = L10n.T("common.close"), XamlRoot = Content.XamlRoot };
        await dlg.ShowAsync();
    }

    private async Task<string?> PickSaveFileAsync(string suggestedName, string typeName, string extension)
    {
        var picker = new FileSavePicker { SuggestedFileName = suggestedName };
        picker.FileTypeChoices.Add(typeName, new List<string> { extension });
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        var file = await picker.PickSaveFileAsync();
        return file?.Path;
    }

    private async Task<string?> PickOpenFileAsync(params string[] extensions)
    {
        var picker = new FileOpenPicker();
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        foreach (var ext in extensions) picker.FileTypeFilter.Add(ext);
        var file = await picker.PickSingleFileAsync();
        return file?.Path;
    }

    // ---------- preferences & localization ----------

    private async Task LoadUiPreferencesAsync()
    {
        var res = await Api.CallAsync("settings.get", Args(("key", "ui.preferences")));
        if (res.Result is { } r && r.TryGetProperty("value", out var v) && v.ValueKind == JsonValueKind.Object)
        {
            L10n.Prefs = UiPreferences.FromJson(v);
        }
        ApplyUiPreferences();
    }

    private static Windows.UI.Color HexColor(string hex)
    {
        var s = hex.TrimStart('#');
        if (s.Length == 8)
            return Windows.UI.Color.FromArgb(
                Convert.ToByte(s[0..2], 16),
                Convert.ToByte(s[2..4], 16),
                Convert.ToByte(s[4..6], 16),
                Convert.ToByte(s[6..8], 16));
        return Windows.UI.Color.FromArgb(
            255,
            Convert.ToByte(s[0..2], 16),
            Convert.ToByte(s[2..4], 16),
            Convert.ToByte(s[4..6], 16));
    }

    private static void SetThemeBrush(string key, string hex)
    {
        if (Application.Current.Resources.TryGetValue(key, out var obj) && obj is Microsoft.UI.Xaml.Media.SolidColorBrush brush)
            brush.Color = HexColor(hex);
    }

    private void ApplyUiPreferences()
    {
        var t = L10n.Prefs.Theme;
        var theme = t switch
        {
            "light" or "warm_paper" => ElementTheme.Light,
            "system" => ElementTheme.Default,
            _ => ElementTheme.Dark,
        };
        if (RootGrid != null) RootGrid.RequestedTheme = theme;

        var isSysLight = t == "system" && Application.Current.RequestedTheme == ApplicationTheme.Light;
        var paletteKey = isSysLight ? "light" : (t == "system" ? "dark" : t);
        switch (paletteKey)
        {
            case "light":
                SetThemeBrush("CanvasBgBrush", "#F4F6F9");
                SetThemeBrush("PanelBgBrush", "#FFFFFF");
                SetThemeBrush("ElevatedBgBrush", "#F0F3F8");
                SetThemeBrush("HoverBgBrush", "#E5EAF2");
                SetThemeBrush("SubtleBorderBrush", "#14000000");
                SetThemeBrush("StrongBorderBrush", "#CBD5E1");
                SetThemeBrush("TextPrimaryBrush", "#1F2328");
                SetThemeBrush("TextSecondaryBrush", "#4B5563");
                SetThemeBrush("TextMutedBrush", "#6B7280");
                SetThemeBrush("AccentBlueBrush", "#1A73E8");
                SetThemeBrush("AccentBlueTextBrush", "#FFFFFF");
                break;
            case "midnight":
                SetThemeBrush("CanvasBgBrush", "#0D1117");
                SetThemeBrush("PanelBgBrush", "#161B26");
                SetThemeBrush("ElevatedBgBrush", "#1F2737");
                SetThemeBrush("HoverBgBrush", "#293347");
                SetThemeBrush("SubtleBorderBrush", "#24818CF8");
                SetThemeBrush("StrongBorderBrush", "#38455E");
                SetThemeBrush("TextPrimaryBrush", "#E6EDF5");
                SetThemeBrush("TextSecondaryBrush", "#B8C4D8");
                SetThemeBrush("TextMutedBrush", "#7D8DA6");
                SetThemeBrush("AccentBlueBrush", "#818CF8");
                SetThemeBrush("AccentBlueTextBrush", "#0D1117");
                break;
            case "forest":
                SetThemeBrush("CanvasBgBrush", "#0F1614");
                SetThemeBrush("PanelBgBrush", "#17221F");
                SetThemeBrush("ElevatedBgBrush", "#202E2A");
                SetThemeBrush("HoverBgBrush", "#2A3C37");
                SetThemeBrush("SubtleBorderBrush", "#1F6EE7B7");
                SetThemeBrush("StrongBorderBrush", "#354F47");
                SetThemeBrush("TextPrimaryBrush", "#E4EFEA");
                SetThemeBrush("TextSecondaryBrush", "#B6CCC3");
                SetThemeBrush("TextMutedBrush", "#7F9A90");
                SetThemeBrush("AccentBlueBrush", "#6EE7B7");
                SetThemeBrush("AccentBlueTextBrush", "#0F1614");
                break;
            case "warm_paper":
                SetThemeBrush("CanvasBgBrush", "#F5EFE6");
                SetThemeBrush("PanelBgBrush", "#FFFDF8");
                SetThemeBrush("ElevatedBgBrush", "#EFE7DA");
                SetThemeBrush("HoverBgBrush", "#E5DAC9");
                SetThemeBrush("SubtleBorderBrush", "#1F5C4630");
                SetThemeBrush("StrongBorderBrush", "#CBBBA4");
                SetThemeBrush("TextPrimaryBrush", "#2C2218");
                SetThemeBrush("TextSecondaryBrush", "#5B4B3A");
                SetThemeBrush("TextMutedBrush", "#83715D");
                SetThemeBrush("AccentBlueBrush", "#B45309");
                SetThemeBrush("AccentBlueTextBrush", "#FFFFFF");
                break;
            case "rose_dusk":
                SetThemeBrush("CanvasBgBrush", "#161217");
                SetThemeBrush("PanelBgBrush", "#211B23");
                SetThemeBrush("ElevatedBgBrush", "#2C242F");
                SetThemeBrush("HoverBgBrush", "#392F3D");
                SetThemeBrush("SubtleBorderBrush", "#1FF4A5C4");
                SetThemeBrush("StrongBorderBrush", "#4E3F54");
                SetThemeBrush("TextPrimaryBrush", "#F1E8F2");
                SetThemeBrush("TextSecondaryBrush", "#CABCC9");
                SetThemeBrush("TextMutedBrush", "#958496");
                SetThemeBrush("AccentBlueBrush", "#F4A5C4");
                SetThemeBrush("AccentBlueTextBrush", "#161217");
                break;
            default:
                SetThemeBrush("CanvasBgBrush", "#131314");
                SetThemeBrush("PanelBgBrush", "#1E1F20");
                SetThemeBrush("ElevatedBgBrush", "#282A2C");
                SetThemeBrush("HoverBgBrush", "#333537");
                SetThemeBrush("SubtleBorderBrush", "#14FFFFFF");
                SetThemeBrush("StrongBorderBrush", "#3C4043");
                SetThemeBrush("TextPrimaryBrush", "#E3E3E3");
                SetThemeBrush("TextSecondaryBrush", "#C4C7C5");
                SetThemeBrush("TextMutedBrush", "#8E918F");
                SetThemeBrush("AccentBlueBrush", "#A8C7FA");
                SetThemeBrush("AccentBlueTextBrush", "#131314");
                break;
        }

        ChatInputBox.FontSize = L10n.Prefs.FontSize;
        ApplyLocalization();
    }

    private void ApplyLocalization()
    {
        _chatImages?.ApplyLocalization();
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(NotebookPicker, L10n.T("nb.switch"));
        NotebookPicker.PlaceholderText = L10n.T("nb.switch");
        NewNotebookBtn.Content = L10n.T("nb.new");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(SettingsBtn, L10n.T("settings.title"));
        SettingsBtnText.Text = L10n.T("settings.title");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(NotebookMenuBtn, L10n.T("nb.menu"));
        RenameNotebookItem.Text = L10n.T("nb.rename");
        ExportLnaItem.Text = L10n.T("nb.export_lna");
        ExportMdItem.Text = L10n.T("nb.export_md");
        ImportLnaItem.Text = L10n.T("nb.import_lna");
        ClearChatItem.Text = L10n.T("nb.clear_chat");
        DeleteNotebookItem.Text = L10n.T("nb.delete");
        ExportDiagItem.Text = L10n.T("nb.export_diag");
        PrivacyItem.Text = L10n.T("nb.privacy");

        SourcesHeaderText.Text = L10n.T("sources.title");
        AddSourceBtn.Content = L10n.T("sources.add");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(AddSourceBtn, L10n.T("sources.add"));
        AddFileMenuItem.Text = L10n.T("sources.add_file");
        AddUrlMenuItem.Text = L10n.T("sources.add_url");
        AddPasteMenuItem.Text = L10n.T("sources.add_paste");
        CreateNoteBtn.Content = L10n.T("sources.create_note");

        ChatInputBox.PlaceholderText = L10n.T("chat.placeholder");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(SendBtn, L10n.T("chat.send"));
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(StopBtn, L10n.T("chat.stop"));
        ToolTipService.SetToolTip(StopBtn, L10n.T("chat.stop"));
        TileAudioText.Text = L10n.T("studio.audio");
        TileSlidesText.Text = L10n.T("studio.slides");
        TileMindmapText.Text = L10n.T("studio.mindmap");
        TileReportText.Text = L10n.T("studio.report");
        TileFlashcardsText.Text = L10n.T("studio.flashcards");
        TileQuizText.Text = L10n.T("studio.quiz");
        TileInfographicText.Text = L10n.T("studio.infographic");
        TileLectureText.Text = L10n.T("studio.lecture");
    }

    // ---------- startup ----------

    public void ReportStartupFailure(string message) =>
        ShowStatus(InfoBarSeverity.Error, L10n.T("app.startup_failed"), message);

    public async Task OnCoreReadyAsync()
    {
        await LoadUiPreferencesAsync();
        var list = await CoreAsync("notebook.list", Args(), L10n.T("nb.op_load"));
        if (list is { } res && Arr(res, "notebooks").FirstOrDefault() is { ValueKind: JsonValueKind.Object } first)
        {
            _activeNotebookId = Str(first, "id");
            _activeNotebookTitle = Str(first, "title");
            SetTitleDisplay();
            await RefreshNotebookPickerAsync();
            await RefreshNotebookDataAsync();
        }
        else
        {
            ShowNoNotebookState();
            await ShowCreateNotebookDialogAsync();
        }
        if (!await WarnIfNoModelAsync()) await ShowOnboardingIfFirstRunAsync();
    }

    /// <summary>Returns true when a model is configured.</summary>
    private async Task<bool> WarnIfNoModelAsync()
    {
        var profiles = await CoreAsync("profile.list", Args(), L10n.T("nb.op_load_profiles"));
        if (profiles is not { } p) return false;
        var ok = Arr(p, "profiles").Any(prof => Bool(prof, "is_active"));
        if (!ok)
        {
            ShowStatus(InfoBarSeverity.Warning, L10n.T("settings.no_model_title"), L10n.T("settings.no_model_msg"));
        }
        else if (StatusBar.Title == L10n.T("settings.no_model_title"))
        {
            StatusBar.IsOpen = false;
        }
        return ok;
    }

    // ---------- notebooks ----------

    private async Task RefreshNotebookPickerAsync()
    {
        var list = await CoreAsync("notebook.list", Args(), L10n.T("nb.op_load"));
        if (list is not { } res) return;
        _loadingPicker = true;
        try
        {
            NotebookPicker.Items.Clear();
            foreach (var nb in Arr(res, "notebooks"))
            {
                var id = Str(nb, "id");
                var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
                row.Children.Add(new Viewbox
                {
                    Width = 16,
                    Height = 16,
                    VerticalAlignment = VerticalAlignment.Center,
                    Child = new PathIcon
                    {
                        Width = 24,
                        Height = 24,
                        Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M18,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V4A2,2 0 0,0 18,2M18,20H6V4H8V12L10.5,9.75L13,12V4H18V20Z"),
                        Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentBlueBrush"],
                    },
                });
                row.Children.Add(new TextBlock { Text = Str(nb, "title"), VerticalAlignment = VerticalAlignment.Center });
                var item = new ComboBoxItem { Content = row, Tag = id };
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
        await SwitchNotebookAsync(id);
    }

    private async Task SwitchNotebookAsync(string id)
    {
        CloseSourcePreview();
        CloseArtifactInline();
        SwapChatDraft(_activeNotebookId, id);
        _activeNotebookId = id;
        _streaming = null;
        var nb = await CoreAsync("notebook.get", Args(("notebook_id", id)), L10n.T("nb.op_load"));
        _activeNotebookTitle = nb is { } n ? Str(n, "title") : "";
        SetTitleDisplay();
        await RefreshNotebookPickerAsync();
        await RefreshNotebookDataAsync();
    }

    private async void OnNewNotebookClick(object sender, RoutedEventArgs e) => await ShowCreateNotebookDialogAsync();

    private void OnRenameNotebookClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        NotebookTitleText.Focus(FocusState.Programmatic);
        NotebookTitleText.SelectAll();
    }

    // ---------- inline title editing ----------

    private bool _titleCommitting;

    private void SetTitleDisplay()
    {
        NotebookTitleText.IsEnabled = _activeNotebookId != null;
        NotebookTitleText.PlaceholderText = L10n.T("nb.none_hint");
        NotebookTitleText.Text = _activeNotebookId != null ? _activeNotebookTitle : "";
        ToolTipService.SetToolTip(NotebookTitleText, L10n.T("nb.title_edit_hint"));
    }

    private void OnTitleBoxPointerEntered(object sender, Microsoft.UI.Xaml.Input.PointerRoutedEventArgs e)
    {
        if (NotebookTitleText.FocusState == FocusState.Unfocused && NotebookTitleText.IsEnabled)
            NotebookTitleText.BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["StrongBorderBrush"];
    }

    private void OnTitleBoxPointerExited(object sender, Microsoft.UI.Xaml.Input.PointerRoutedEventArgs e)
    {
        if (NotebookTitleText.FocusState == FocusState.Unfocused)
            NotebookTitleText.BorderBrush = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent);
    }

    private void OnTitleBoxGotFocus(object sender, RoutedEventArgs e)
    {
        NotebookTitleText.BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["AccentBlueBrush"];
        NotebookTitleText.SelectAll();
    }

    private async void OnTitleBoxLostFocus(object sender, RoutedEventArgs e)
    {
        NotebookTitleText.BorderBrush = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent);
        await CommitTitleAsync();
    }

    private void OnTitleBoxKeyDown(object sender, Microsoft.UI.Xaml.Input.KeyRoutedEventArgs e)
    {
        if (e.Key == Windows.System.VirtualKey.Enter)
        {
            e.Handled = true;
            ChatInputBox.Focus(FocusState.Programmatic); // moves focus away -> LostFocus commits
        }
        else if (e.Key == Windows.System.VirtualKey.Escape)
        {
            e.Handled = true;
            NotebookTitleText.Text = _activeNotebookTitle; // revert; commit becomes a no-op
            ChatInputBox.Focus(FocusState.Programmatic);
        }
    }

    private async Task CommitTitleAsync()
    {
        if (_titleCommitting || _activeNotebookId == null) return;
        var name = NotebookTitleText.Text.Trim();
        if (name.Length == 0 || name == _activeNotebookTitle)
        {
            NotebookTitleText.Text = _activeNotebookTitle;
            return;
        }
        _titleCommitting = true;
        try
        {
            if (await CoreAsync("notebook.rename", Args(("notebook_id", _activeNotebookId), ("title", name)), L10n.T("nb.op_rename")) != null)
            {
                _activeNotebookTitle = name;
                await RefreshNotebookPickerAsync();
            }
            else NotebookTitleText.Text = _activeNotebookTitle;
        }
        finally { _titleCommitting = false; }
    }

    // ---------- create-notebook dialog ----------

    private bool _createDialogOpen;

    private void ShowNoNotebookState()
    {
        SwapChatDraft(_activeNotebookId, null);
        _activeNotebookId = null;
        _activeNotebookTitle = "";
        _streaming = null;
        CloseSourcePreview();
        CloseArtifactInline();
        SetTitleDisplay();
        _loadingPicker = true;
        try { NotebookPicker.Items.Clear(); } finally { _loadingPicker = false; }
        RenderSources(null, null);
        ChatList.Items.Clear();
        SummaryText.Text = "";
        SuggestedPanel.Children.Clear();
        using var empty = JsonDocument.Parse("{\"artifacts\":[]}");
        RenderArtifacts(empty.RootElement.Clone());
    }

    /// <summary>Modal dialog: name, details, optional source files. Returns true when a notebook was created.</summary>
    private async Task<bool> ShowCreateNotebookDialogAsync()
    {
        if (_createDialogOpen) return false;
        _createDialogOpen = true;
        try
        {
            var nameBox = new TextBox { Header = L10n.T("nb.dlg_name"), PlaceholderText = L10n.T("nb.dlg_name_ph"), MinWidth = 460, MaxLength = 120 };
            var descHint = new TextBlock { Text = L10n.T("nb.dlg_ai_hint"), TextWrapping = TextWrapping.Wrap, Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"] };
            var files = new List<string>();
            var fileList = new StackPanel { Spacing = 4 };
            var addBtn = new Button { Content = L10n.T("nb.dlg_add_files") };
            var srcHeader = new TextBlock { Text = L10n.T("nb.dlg_sources"), Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"] };
            var panel = new StackPanel { Spacing = 12 };
            panel.Children.Add(nameBox);
            panel.Children.Add(descHint);
            panel.Children.Add(srcHeader);
            panel.Children.Add(addBtn);
            panel.Children.Add(fileList);

            var dlg = new ContentDialog
            {
                Title = L10n.T("nb.new"),
                Content = new ScrollViewer { Content = panel, MaxHeight = 520 },
                PrimaryButtonText = L10n.T("common.create"),
                CloseButtonText = L10n.T("common.cancel"),
                DefaultButton = ContentDialogButton.Primary,
                IsPrimaryButtonEnabled = false,
                XamlRoot = Content.XamlRoot,
            };
            nameBox.TextChanged += (_, _) => dlg.IsPrimaryButtonEnabled = !string.IsNullOrWhiteSpace(nameBox.Text);
            void Rebuild()
            {
                fileList.Children.Clear();
                foreach (var f in files.ToList())
                {
                    var row = new Grid { ColumnSpacing = 8 };
                    row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                    row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                    var t = new TextBlock { Text = System.IO.Path.GetFileName(f), TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center };
                    ToolTipService.SetToolTip(t, f);
                    var rm = new Button { Content = "✕", Padding = new Thickness(8, 2, 8, 2) };
                    Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(rm, L10n.T("common.delete"));
                    var captured = f;
                    rm.Click += (_, _) => { files.Remove(captured); Rebuild(); };
                    Grid.SetColumn(rm, 1);
                    row.Children.Add(t);
                    row.Children.Add(rm);
                    fileList.Children.Add(row);
                }
            }
            addBtn.Click += async (_, _) =>
            {
                var picked = await PickOpenFilesAsync();
                foreach (var f in picked) if (!files.Contains(f)) files.Add(f);
                Rebuild();
            };

            if (await dlg.ShowAsync() != ContentDialogResult.Primary) return false;

            var args = Args(("title", nameBox.Text.Trim()));
            var created = await CoreAsync("notebook.create", args, L10n.T("nb.op_create"));
            if (created is not { } cr) return false;
            var id = Str(cr, "id");
            await SwitchNotebookAsync(id);
            if (files.Count > 0)
            {
                ShowStatus(InfoBarSeverity.Informational, L10n.T("sources.op_add"), string.Join(", ", files.Select(f => System.IO.Path.GetFileName(f))));
                if (await CoreAsync("source.add_files", Args(("notebook_id", id), ("paths", files.ToArray())), L10n.T("sources.op_add")) != null)
                    StatusBar.IsOpen = false;
                await RefreshNotebookDataAsync();
            }
            return true;
        }
        finally { _createDialogOpen = false; }
    }

    private async Task<List<string>> PickOpenFilesAsync()
    {
        var picker = new FileOpenPicker();
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        picker.FileTypeFilter.Add("*");
        var picked = await picker.PickMultipleFilesAsync();
        return picked?.Select(f => f.Path).ToList() ?? new List<string>();
    }

    private async void OnDeleteNotebookClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        if (!await ConfirmAsync(L10n.T("nb.delete"), L10n.Fmt("nb.delete_confirm", _activeNotebookTitle), L10n.T("common.delete"))) return;
        if (await CoreAsync("notebook.delete", Args(("notebook_id", _activeNotebookId)), L10n.T("nb.op_delete")) == null) return;
        // the deleted notebook's unsent draft goes with it
        ChatInputBox.Text = string.Empty;
        _chatImages.Clear();
        var list = await CoreAsync("notebook.list", Args(), L10n.T("nb.op_load"));
        var next = list is { } l ? Arr(l, "notebooks").FirstOrDefault() : default;
        if (next.ValueKind == JsonValueKind.Object)
        {
            await SwitchNotebookAsync(Str(next, "id"));
        }
        else
        {
            ShowNoNotebookState();
            await ShowCreateNotebookDialogAsync();
        }
    }

    private async void OnExportLnaClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var path = await PickSaveFileAsync(_activeNotebookTitle, L10n.T("nb.archive_filter"), ".lna");
        if (path == null) return;
        if (await CoreAsync("notebook.export", Args(("notebook_id", _activeNotebookId), ("format", "lna"), ("out_path", path)), L10n.T("nb.op_export")) != null)
            ShowStatus(InfoBarSeverity.Success, L10n.T("common.exported"), path);
    }

    private async void OnExportMarkdownClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var path = await PickSaveFileAsync(_activeNotebookTitle, "Markdown", ".md");
        if (path == null) return;
        if (await CoreAsync("notebook.export", Args(("notebook_id", _activeNotebookId), ("format", "markdown"), ("out_path", path)), L10n.T("nb.op_export")) != null)
            ShowStatus(InfoBarSeverity.Success, L10n.T("common.exported"), path);
    }

    private async void OnImportLnaClick(object sender, RoutedEventArgs e)
    {
        var path = await PickOpenFileAsync(".lna");
        if (path == null) return;
        var res = await CoreAsync("notebook.import", Args(("in_path", path)), L10n.T("nb.op_import"));
        if (res is { } r)
        {
            ShowStatus(InfoBarSeverity.Success, L10n.T("nb.imported"), L10n.T("nb.imported_msg"));
            await SwitchNotebookAsync(Str(r, "notebook_id"));
        }
    }

    private async void OnExportDiagnosticsClick(object sender, RoutedEventArgs e)
    {
        var path = await PickSaveFileAsync("diagnostics", L10n.T("nb.diag_filter"), ".json");
        if (path == null) return;
        if (await CoreAsync("diagnostics.export", Args(("out_path", path)), L10n.T("nb.export_diag")) != null)
            ShowStatus(InfoBarSeverity.Success, L10n.T("nb.diag_exported"), L10n.T("nb.diag_exported_msg"));
    }

    // ---------- refresh ----------

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

        var summary = await CoreAsync("notebook.get", param, L10n.T("nb.op_load"));
        if (summary is { } sm) RenderSummary(sm);

        var src = await CoreAsync("source.list", param, L10n.T("nb.op_load_sources"));
        var notes = await CoreAsync("note.list", param, L10n.T("nb.op_load_notes"));
        RenderSources(src, notes);

        if (_streaming == null)
        {
            var chat = await CoreAsync("chat.history", param, L10n.T("nb.op_load_chat"));
            if (chat is { } c) RenderChat(c);
        }

        var art = await CoreAsync("studio.list", param, L10n.T("nb.op_load_studio"));
        if (art is { } a) RenderArtifacts(a);
    }

    private async Task RefreshStudioArtifactsAsync()
    {
        if (_activeNotebookId == null) return;
        var art = await CoreAsync("studio.list", Args(("notebook_id", _activeNotebookId)), L10n.T("nb.op_load_studio"));
        if (art is { } a) RenderArtifacts(a);
    }

    private static bool NeedsSummaryTranslation(string summaryMd, bool wantEnglish)
    {
        if (string.IsNullOrWhiteSpace(summaryMd)) return false;
        var cjk = 0;
        var latin = 0;
        foreach (var ch in summaryMd)
        {
            if (ch >= '\u4e00' && ch <= '\u9fff') cjk++;
            else if ((ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z')) latin++;
        }
        return wantEnglish ? cjk >= 4 : (cjk == 0 && latin >= 12);
    }

    private void RenderSummary(JsonElement nb)
    {
        var md = Str(nb, "summary_md");
        SummaryText.Text = md.Length > 0 ? md : L10n.T("chat.empty_summary");
        if (_activeNotebookId is { } nid && NeedsSummaryTranslation(md, L10n.IsEnglish))
        {
            var cacheKey = $"{nid}:{L10n.TargetLangCode}";
            if (_autoTranslatedSummaries.Add(cacheKey))
            {
                var targetLang = L10n.TargetLangCode;
                _ = Task.Run(async () =>
                {
                    var tr = await Api.CallAsync("i18n.translate_notebook_content", Args(("notebook_id", nid), ("target_lang", targetLang)));
                    if (tr.Result is { } res && Bool(res, "translated_with_llm"))
                    {
                        DispatcherQueue.TryEnqueue(async () =>
                        {
                            if (_activeNotebookId == nid)
                            {
                                var updated = await CoreAsync("notebook.get", Args(("notebook_id", nid)), L10n.T("nb.op_load"));
                                if (updated is { } u) RenderSummary(u);
                            }
                        });
                    }
                });
            }
        }
        SuggestedPanel.Children.Clear();
        SuggestedPanel.ColumnDefinitions.Clear();
        var raw = Str(nb, "suggested_questions_json");
        var questions = new List<string>();
        if (nb.TryGetProperty("suggested_questions", out var sq) && sq.ValueKind == JsonValueKind.Array)
        {
            foreach (var q in sq.EnumerateArray()) if (q.ValueKind == JsonValueKind.String) questions.Add(q.GetString() ?? "");
        }
        else if (raw.Length > 0)
        {
            try
            {
                using var doc = JsonDocument.Parse(raw);
                foreach (var q in doc.RootElement.EnumerateArray()) questions.Add(q.GetString() ?? "");
            }
            catch (JsonException) { }
        }
        if (!L10n.Prefs.ShowSuggestedQuestions) return;
        var topQuestions = questions.Where(q => q.Length > 0).Take(3).ToList();
        for (int i = 0; i < topQuestions.Count; i++)
        {
            SuggestedPanel.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            var q = topQuestions[i];
            var btn = new Button
            {
                Content = new TextBlock
                {
                    Text = q,
                    FontSize = 12,
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                    TextWrapping = TextWrapping.Wrap,
                    MaxLines = 2,
                    TextTrimming = TextTrimming.CharacterEllipsis,
                },
                Tag = q,
                Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["PanelBgBrush"],
                BorderBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["StrongBorderBrush"],
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(12),
                Padding = new Thickness(14, 10, 14, 10),
                MinHeight = 44,
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Left,
                VerticalAlignment = VerticalAlignment.Stretch,
            };
            btn.Click += async (s, _) => { if (s is Button b && b.Tag is string text) await SendChatAsync(text); };
            Grid.SetColumn(btn, i);
            SuggestedPanel.Children.Add(btn);
        }
    }

    // ---------- events ----------

    private void OnCoreEvent(ApiEventDto ev)
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            try
            {
                switch (ev.Event)
                {
                    case "studio.status":
                        OnKgStatusEvent(ev.Data);
                        if (!ev.Data.TryGetProperty("in_studio", out var inSt) || inSt.GetBoolean())
                            await RefreshStudioArtifactsAsync();
                        break;
                    case "studio.thought":
                        OnStudioThoughtPreview(ev.Data);
                        OnKgThoughtEvent(ev.Data);
                        break;
                    case "studio.thought_delta":
                        OnKgThoughtDeltaEvent(ev.Data);
                        break;
                    case "ai.activity":
                    {
                        var act = ev.Data.TryGetProperty("active", out var aEl) && aEl.ValueKind == JsonValueKind.True;
                        if (Str(ev.Data, "scope") == "source_index")
                        {
                            // Shown as an in-card spinner instead of a banner.
                            var sid = Str(ev.Data, "source_id");
                            if (act) _guideIndexing.Add(sid);
                            else
                            {
                                _guideIndexing.Remove(sid);
                                await RefreshNotebookDataAsync();
                                if (_sourcePreviewOpen && _previewSourceId == sid) _ = ShowSourcePreviewAsync(sid, null, null, null);
                            }
                            break;
                        }
                        var title = Str(ev.Data, "title");
                        var body = Str(ev.Data, "body");
                        if (act && !string.IsNullOrEmpty(title))
                            ShowStatus(InfoBarSeverity.Informational, title, body);
                        else if (!act)
                            StatusBar.IsOpen = false;
                        break;
                    }
                    case "source.status":
                    case "notebook.summary_updated":
                        await RefreshNotebookDataAsync();
                        break;
                    case "embedding.download_progress":
                    case "embedding.download_done":
                        OnEmbeddingEvent(ev);
                        break;
                    default:
                        if (ev.Event.StartsWith("chat.")) await OnChatEventAsync(ev);
                        break;
                }
            }
            catch (Exception ex)
            {
                ShowStatus(InfoBarSeverity.Error, L10n.T("app.ui_refresh_failed"), ex.Message);
            }
        });
    }
}
