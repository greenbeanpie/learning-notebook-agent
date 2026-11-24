using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.Web.WebView2.Core;
using System.Diagnostics;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

public sealed partial class MainWindow
{
    private static string StatusLabel(string status) => status switch
    {
        "QUEUED" => L10n.T("sources.status_queued"),
        "PARSING" => L10n.T("sources.status_parsing"),
        "CHUNKING" => L10n.T("sources.status_chunking"),
        "EMBEDDING" => L10n.T("sources.status_embedding"),
        "READY" => L10n.T("sources.status_ready"),
        "FAILED" => L10n.T("sources.status_failed"),
        _ => status,
    };

    private bool _updatingSourceChecks;

    private static (string Hex, string PathData) SourceKindIcon(string kind, string title)
    {
        var k = kind.ToLowerInvariant();
        if (title.EndsWith(".pdf", StringComparison.OrdinalIgnoreCase)) k = "pdf";
        else if (title.EndsWith(".md", StringComparison.OrdinalIgnoreCase) || title.EndsWith(".markdown", StringComparison.OrdinalIgnoreCase)) k = "md";
        else if (string.IsNullOrEmpty(k) && title.EndsWith(".txt", StringComparison.OrdinalIgnoreCase)) k = "txt";
        return k switch
        {
            "pdf" => ("#F28B82", "M19,3H5C3.9,3 3,3.9 3,5V19C3,20.1 3.9,21 5,21H19C20.1,21 21,20.1 21,19V5C21,3.9 20.1,3 19,3M9.5,11.5C9.5,12.3 8.8,13 8,13H7V15H5.5V9H8C8.8,9 9.5,9.7 9.5,10.5V11.5M14.5,13.5C14.5,14.3 13.8,15 13,15H10.5V9H13C13.8,9 14.5,9.7 14.5,10.5V13.5M18.5,10.5H17V11.5H18.5V13H17V15H15.5V9H18.5V10.5M12,10.5H13V13.5H12V10.5M7,10.5H8V11.5H7V10.5Z"),
            "md" or "markdown" => ("#8AB4F8", "M20.56,18H3.44C2.65,18 2,17.37 2,16.59V7.41C2,6.63 2.65,6 3.44,6H20.56C21.35,6 22,6.63 22,7.41V16.59C22,17.37 21.35,18 20.56,18M6.81,15.19V11.53L8.73,13.88L10.65,11.53V15.19H12.58V8.81H10.65L8.73,11.16L6.81,8.81H4.89V15.19H6.81M19.69,12H17.77V8.81H15.85V12H13.92L16.81,15.28L19.69,12Z"),
            "docx" or "odt" or "rtf" or "hwp" or "hwpx" => ("#669DF6", "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20M16,11V13H8V11H16M16,15V17H8V15H16Z"),
            "csv" => ("#5BB974", "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20M10,19H7V16H10V19M10,15H7V12H10V15M15,19H12V16H15V19M15,15H12V12H15V15Z"),
            "pptx" => ("#FCAD70", "M19,3H5C3.89,3 3,3.89 3,5V19A2,2 0 0,0 5,21H19A2,2 0 0,0 21,19V5C21,3.89 20.1,3 19,3M19,19H5V5H19V19M7,8H17V10H7V8M7,12H14V14H7V12Z"),
            "epub" => ("#C58AF9", "M18,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V4A2,2 0 0,0 18,2M18,20H6V4H9V12L11.5,10.5L14,12V4H18V20Z"),
            "image" => ("#78D9EC", "M19,19H5V5H19M19,3H5A2,2 0 0,0 3,5V19A2,2 0 0,0 5,21H19A2,2 0 0,0 21,19V5C21,3.89 20.1,3 19,3M13.96,12.29L11.21,15.83L9.25,13.47L6.5,17H17.5L13.96,12.29Z"),
            "audio" => ("#F48FB1", "M12,3V13.55C11.41,13.21 10.73,13 10,13A4,4 0 0,0 6,17A4,4 0 0,0 10,21A4,4 0 0,0 14,17V7H18V3H12Z"),
            "video" => ("#EE675C", "M17,10.5V7A1,1 0 0,0 16,6H4A1,1 0 0,0 3,7V17A1,1 0 0,0 4,18H16A1,1 0 0,0 17,17V13.5L21,17.5V6.5L17,10.5Z"),
            "txt" => ("#81C995", "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20M8,12H16V14H8V12M8,16H13V18H8V16Z"),
            "web" => ("#78D9EC", "M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2M11,19.93C7.05,19.44 4,16.08 4,12C4,11.38 4.08,10.79 4.21,10.21L9,15V16A2,2 0 0,0 11,18M17.9,17.39C17.64,16.58 16.9,16 16,16H15V13A1,1 0 0,0 14,12H8V10H10A1,1 0 0,0 11,9V7H13A2,2 0 0,0 15,5V4.59C17.93,5.78 20,8.65 20,12C20,14.08 19.2,15.97 17.9,17.39Z"),
            "note" => ("#FDD663", "M14,10H19.5L14,4.5V10M5,3H15L21,9V19A2,2 0 0,1 19,21H5C3.89,21 3,20.1 3,19V5C3,3.89 3.89,3 5,3M5,5V19H19V12H12V5H5Z"),
            _ => ("#C4C7C5", "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20Z"),
        };
    }

    private static Microsoft.UI.Xaml.Media.SolidColorBrush BrushFromHex(string hex) => new(Windows.UI.Color.FromArgb(
        255,
        Convert.ToByte(hex.Substring(1, 2), 16),
        Convert.ToByte(hex.Substring(3, 2), 16),
        Convert.ToByte(hex.Substring(5, 2), 16)));

    private void RenderSources(JsonElement? src, JsonElement? notes)
    {
        SourcesList.Items.Clear();
        if (src is { } sr)
        {
            var sourceItems = Arr(sr, "sources").ToList();
            if (sourceItems.Count > 0)
            {
                var allIds = sourceItems.Select(x => Str(x, "id")).ToList();
                var allSelected = sourceItems.All(x => Bool(x, "is_selected"));
                var selectAllRow = new Grid
                {
                    ColumnSpacing = 8,
                    MinHeight = 38,
                    Padding = new Thickness(8, 2, 4, 2),
                    Margin = new Thickness(0, 0, 0, 4),
                };
                selectAllRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                selectAllRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                selectAllRow.Children.Add(new TextBlock
                {
                    Text = L10n.T("sources.select_all"),
                    FontSize = 12,
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
                    VerticalAlignment = VerticalAlignment.Center,
                });
                var selectAllCb = new CheckBox
                {
                    IsChecked = allSelected,
                    MinWidth = 36,
                    Width = 36,
                    MinHeight = 36,
                    Height = 36,
                    Padding = new Thickness(0),
                    HorizontalContentAlignment = HorizontalAlignment.Center,
                    VerticalContentAlignment = VerticalAlignment.Center,
                    VerticalAlignment = VerticalAlignment.Center,
                };
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(selectAllCb, L10n.T("sources.select_all"));
                ToolTipService.SetToolTip(selectAllCb, L10n.T("sources.select_all_tip"));
                selectAllCb.Checked += async (_, _) => await SetAllSourcesSelectedAsync(allIds, true);
                selectAllCb.Unchecked += async (_, _) => await SetAllSourcesSelectedAsync(allIds, false);
                Grid.SetColumn(selectAllCb, 1);
                selectAllRow.Children.Add(selectAllCb);
                SourcesList.Items.Add(selectAllRow);

                foreach (var s in sourceItems)
                {
                    SourcesList.Items.Add(SourceRow(s));
                }
            }
            else
            {
                var empty = Line(L10n.T("sources.empty_hint"));
                empty.Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"];
                empty.Margin = new Thickness(4, 6, 4, 6);
                SourcesList.Items.Add(empty);
            }
        }
        if (notes is { } nr)
        {
            var list = Arr(nr, "notes").ToList();
            if (list.Count > 0)
            {
                SourcesList.Items.Add(new TextBlock
                {
                    Text = L10n.T("sources.notes_header"),
                    FontSize = 13,
                    FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                    Margin = new Thickness(8, 16, 4, 6),
                });
                foreach (var n in list)
                {
                    var rowContent = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, VerticalAlignment = VerticalAlignment.Center };
                    rowContent.Children.Add(new Viewbox
                    {
                        Width = 18,
                        Height = 18,
                        VerticalAlignment = VerticalAlignment.Center,
                        Child = new PathIcon
                        {
                            Width = 24,
                            Height = 24,
                            Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M14,10H19.5L14,4.5V10M5,3H15L21,9V19A2,2 0 0,1 19,21H5C3.89,21 3,20.1 3,19V5C3,3.89 3.89,3 5,3M5,5V19H19V12H12V5H5Z"),
                            Foreground = BrushFromHex("#FDD663"),
                        },
                    });
                    rowContent.Children.Add(new TextBlock
                    {
                        Text = Str(n, "title"),
                        FontSize = 13,
                        Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"],
                        TextTrimming = TextTrimming.CharacterEllipsis,
                        MaxLines = 1,
                        VerticalAlignment = VerticalAlignment.Center,
                    });
                    var b = new Button
                    {
                        Content = rowContent,
                        Tag = Str(n, "id"),
                        MinHeight = 42,
                        Padding = new Thickness(8, 8, 8, 8),
                        Margin = new Thickness(0, 2, 0, 2),
                        CornerRadius = new CornerRadius(10),
                        Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
                        BorderThickness = new Thickness(0),
                        HorizontalAlignment = HorizontalAlignment.Stretch,
                        HorizontalContentAlignment = HorizontalAlignment.Left,
                    };
                    b.Click += async (s, _) => { if (s is Button btn && btn.Tag is string id) await EditNoteAsync(id); };
                    SourcesList.Items.Add(b);
                }
            }
        }
    }

    private async Task SetAllSourcesSelectedAsync(List<string> ids, bool selected)
    {
        if (_updatingSourceChecks) return;
        _updatingSourceChecks = true;
        try
        {
            foreach (var id in ids)
            {
                await CoreAsync("source.set_selected", Args(("source_id", id), ("selected", selected)), L10n.T("sources.op_select"));
            }
            await RefreshNotebookDataAsync();
        }
        finally
        {
            _updatingSourceChecks = false;
        }
    }

    private UIElement SourceRow(JsonElement s)
    {
        var id = Str(s, "id");
        var status = Str(s, "status");
        var code = Str(s, "error_code");
        var title = Str(s, "title");
        var kind = Str(s, "kind");

        string? subtext = null;
        var subColor = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"];
        if (status == "FAILED")
        {
            var advice = Advice(code);
            var detail = advice.Length > 0 ? advice : Str(s, "error_detail");
            subtext = detail.Length > 0 ? L10n.Fmt("sources.status_failed_detail", detail) : L10n.T("sources.status_failed");
            subColor = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 160, 140));
        }
        else if (status != "READY")
        {
            subtext = StatusLabel(status);
        }
        else if (code == "EMBEDDING_INCOMPLETE")
        {
            subtext = L10n.T("sources.keyword_only");
            subColor = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 200, 120));
        }

        // 1. Icon (PDF / Markdown / TXT / Web / Note)
        var (iconHex, iconPath) = SourceKindIcon(kind, title);
        var typeIcon = new PathIcon
        {
            Width = 24,
            Height = 24,
            Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), iconPath),
            Foreground = BrushFromHex(iconHex),
        };
        var iconHost = new Viewbox
        {
            Width = 18,
            Height = 18,
            VerticalAlignment = VerticalAlignment.Center,
            HorizontalAlignment = HorizontalAlignment.Center,
            Child = typeIcon,
        };

        // 2. Filename (+ subtle status line only when not READY)
        var titleStack = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        var titleBlock = new TextBlock
        {
            Text = title,
            FontSize = 13,
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"],
            TextTrimming = TextTrimming.CharacterEllipsis,
            MaxLines = 1,
        };
        titleStack.Children.Add(titleBlock);
        if (subtext != null)
        {
            titleStack.Children.Add(new TextBlock
            {
                Text = subtext,
                FontSize = 11,
                Foreground = subColor,
                TextTrimming = TextTrimming.CharacterEllipsis,
                MaxLines = 1,
            });
        }
        ToolTipService.SetToolTip(titleStack, subtext != null ? $"{title}\n{subtext}" : $"{title} ({L10n.T("sources.click_to_view")})");

        // 3. Actions (three dots: rename, retry, delete — preview is triggered by clicking the row)
        var menu = new MenuFlyout { Placement = Microsoft.UI.Xaml.Controls.Primitives.FlyoutPlacementMode.BottomEdgeAlignedRight };
        void Add(string label, RoutedEventHandler h)
        {
            var item = new MenuFlyoutItem { Text = label, Tag = (id, title) };
            item.Click += h;
            menu.Items.Add(item);
        }
        Add(L10n.T("sources.rename"), OnRenameSourceClick);
        if (status == "FAILED") Add(L10n.T("sources.retry_import"), OnRetrySourceClick);
        Add(L10n.T("sources.delete"), OnDeleteSourceClick);

        var more = new Button
        {
            Content = new Viewbox
            {
                Width = 18,
                Height = 18,
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
                Child = new PathIcon
                {
                    Width = 24,
                    Height = 24,
                    Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), "M6,10A2,2 0 0,0 4,12A2,2 0 0,0 6,14A2,2 0 0,0 8,12A2,2 0 0,0 6,10M12,10A2,2 0 0,0 10,12A2,2 0 0,0 12,14A2,2 0 0,0 14,12A2,2 0 0,0 12,10M18,10A2,2 0 0,0 16,12A2,2 0 0,0 18,14A2,2 0 0,0 20,12A2,2 0 0,0 18,10Z"),
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                },
            },
            Flyout = menu,
            MinWidth = 36,
            Width = 36,
            MinHeight = 36,
            Height = 36,
            Padding = new Thickness(0),
            Margin = new Thickness(0, 0, 4, 0),
            CornerRadius = new CornerRadius(18),
            HorizontalContentAlignment = HorizontalAlignment.Center,
            VerticalContentAlignment = VerticalAlignment.Center,
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
            VerticalAlignment = VerticalAlignment.Center,
        };
        more.Tapped += (_, e) => e.Handled = true;
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(more, L10n.Fmt("sources.more_actions_for", title));
        ToolTipService.SetToolTip(more, L10n.T("sources.more_actions_tip"));

        // 4. Select whether to cite (CheckBox)
        var cb = new CheckBox
        {
            IsChecked = Bool(s, "is_selected"),
            Tag = id,
            MinWidth = 36,
            Width = 36,
            MinHeight = 36,
            Height = 36,
            Padding = new Thickness(0),
            HorizontalContentAlignment = HorizontalAlignment.Center,
            VerticalContentAlignment = VerticalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center,
        };
        cb.Tapped += (_, e) => e.Handled = true;
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(cb, L10n.Fmt("sources.cite_source", title));
        ToolTipService.SetToolTip(cb, L10n.T("sources.cite_source_tip"));
        cb.Checked += OnSourceCheckChanged;
        cb.Unchecked += OnSourceCheckChanged;

        var row = new Grid
        {
            ColumnSpacing = 8,
            MinHeight = 44,
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
        };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        Grid.SetColumn(iconHost, 0);
        Grid.SetColumn(titleStack, 1);
        Grid.SetColumn(more, 2);
        Grid.SetColumn(cb, 3);

        row.Children.Add(iconHost);
        row.Children.Add(titleStack);
        row.Children.Add(more);
        row.Children.Add(cb);

        var transparentBrush = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent);
        var hoverBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["HoverBgBrush"];
        var pressedBrush = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"];
        var border = new Border
        {
            Background = transparentBrush,
            CornerRadius = new CornerRadius(10),
            Padding = new Thickness(8, 4, 4, 4),
            Margin = new Thickness(0, 2, 0, 2),
            Child = row,
        };
        border.PointerEntered += (_, _) => border.Background = hoverBrush;
        border.PointerExited += (_, _) => border.Background = transparentBrush;
        border.PointerPressed += (_, _) => border.Background = pressedBrush;
        border.PointerReleased += (_, _) => border.Background = hoverBrush;
        border.Tapped += async (_, _) => await ShowSourcePreviewAsync(id, null, null, title);
        return border;
    }

    private static (string Id, string Title) Tagged(object sender) =>
        sender is MenuFlyoutItem { Tag: ValueTuple<string, string> t } ? t : ("", "");

    private async void OnAddSourceClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var avEnabled = true;
        var mediaSet = await Api.CallAsync("settings.get", Args(("key", "media_processing")));
        if (mediaSet.Result is { } ms && ms.TryGetProperty("value", out var mv) && mv.ValueKind == JsonValueKind.Object)
        {
            if (mv.TryGetProperty("audio_video_enabled", out var avp) && avp.ValueKind == JsonValueKind.False)
                avEnabled = false;
        }

        var exts = new List<string>
        {
            ".pdf", ".txt", ".md", ".markdown", ".docx", ".csv", ".pptx", ".epub", ".odt", ".rtf", ".hwp", ".hwpx",
            ".avif", ".bmp", ".gif", ".ico", ".jp2", ".png", ".webp", ".tif", ".tiff", ".heic", ".heif", ".jpeg", ".jpg", ".jpe",
            ".json", ".jsonl", ".yaml", ".yml", ".toml", ".xml", ".html", ".htm", ".xhtml", ".tex", ".log", ".rst", ".tsv",
            ".py", ".rs", ".js", ".ts", ".c", ".cpp", ".h", ".cs", ".java", ".go", ".sql", ".ini", ".cfg", ".srt", ".vtt",
        };
        if (avEnabled)
        {
            exts.AddRange(new[]
            {
                ".aac", ".aif", ".aifc", ".aiff", ".amr", ".au", ".m4a", ".mp3", ".ogg", ".opus", ".ra", ".snd", ".wav", ".wma",
                ".3g2", ".3gp", ".avi", ".mp4", ".mpeg",
            });
        }
        exts.Add("*");

        var path = await PickOpenFileAsync(exts.ToArray());
        if (path == null) return;
        await CoreAsync("source.add_files", Args(("notebook_id", _activeNotebookId), ("paths", new[] { path })), L10n.T("sources.op_add"));
        await RefreshNotebookDataAsync();
    }

    private async void OnAddUrlClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var url = await PromptTextAsync(L10n.T("sources.add_url_title"), L10n.T("sources.add_url_label"), "https://", false, L10n.T("common.import"));
        if (string.IsNullOrWhiteSpace(url)) return;
        ShowStatus(InfoBarSeverity.Informational, L10n.T("sources.fetching_url"), url.Trim());
        if (await CoreAsync("source.add_url", Args(("notebook_id", _activeNotebookId), ("url", url.Trim())), L10n.T("sources.add_url_title")) != null)
        {
            StatusBar.IsOpen = false;
            await RefreshNotebookDataAsync();
        }
    }

    private async void OnAddPasteClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var titleBox = new TextBox { Header = L10n.T("common.title"), Text = L10n.T("sources.paste_title_default"), MinWidth = 460 };
        var textBox = new TextBox { Header = L10n.T("common.content"), AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 240, MinWidth = 460 };
        var panel = new StackPanel { Spacing = 10 };
        panel.Children.Add(titleBox);
        panel.Children.Add(textBox);
        var dlg = new ContentDialog { Title = L10n.T("sources.paste_dialog_title"), Content = panel, PrimaryButtonText = L10n.T("common.add"), CloseButtonText = L10n.T("common.cancel"), DefaultButton = ContentDialogButton.Primary, XamlRoot = Content.XamlRoot };
        if (await dlg.ShowAsync() != ContentDialogResult.Primary || string.IsNullOrWhiteSpace(textBox.Text)) return;
        var res = await CoreAsync("source.add_text", Args(("notebook_id", _activeNotebookId), ("title", titleBox.Text.Trim()), ("text", textBox.Text)), L10n.T("sources.op_add_text"));
        if (res is { } r && Bool(r, "duplicate")) ShowStatus(InfoBarSeverity.Informational, L10n.T("sources.dup_title"), L10n.T("sources.dup_msg"));
        await RefreshNotebookDataAsync();
    }

    private async void OnSourceCheckChanged(object sender, RoutedEventArgs e)
    {
        if (_updatingSourceChecks) return;
        if (sender is not CheckBox cb || cb.Tag is not string id) return;
        await CoreAsync("source.set_selected", Args(("source_id", id), ("selected", cb.IsChecked == true)), L10n.T("sources.op_select"));
    }

    private async void OnRetrySourceClick(object sender, RoutedEventArgs e)
    {
        var (id, _) = Tagged(sender);
        if (id.Length == 0) return;
        await CoreAsync("source.retry", Args(("source_id", id)), L10n.T("sources.retry_import"));
        await RefreshNotebookDataAsync();
    }

    private async void OnDeleteSourceClick(object sender, RoutedEventArgs e)
    {
        var (id, title) = Tagged(sender);
        if (id.Length == 0) return;
        if (!await ConfirmAsync(L10n.T("sources.delete"), L10n.Fmt("sources.delete_confirm", title), L10n.T("common.delete"))) return;
        await CoreAsync("source.remove", Args(("source_id", id)), L10n.T("sources.delete"));
        await RefreshNotebookDataAsync();
    }

    private async void OnRenameSourceClick(object sender, RoutedEventArgs e)
    {
        var (id, title) = Tagged(sender);
        if (id.Length == 0) return;
        var name = await PromptTextAsync(L10n.T("sources.rename_title"), L10n.T("common.name"), title);
        if (string.IsNullOrWhiteSpace(name)) return;
        await CoreAsync("source.rename", Args(("source_id", id), ("title", name.Trim())), L10n.T("sources.rename_title"));
        await RefreshNotebookDataAsync();
    }

    private const string SourceFullscreenIconPath = "M5,5H10V7H7V10H5V5M14,5H19V10H17V7H14V5M17,14H19V19H14V17H17V14M10,17V19H5V14H7V17H10Z";
    private const string SourceExitFullscreenIconPath = "M14,14H19V16H16V19H14V14M5,14H10V19H8V16H5V14M8,5H10V10H5V8H8V5M19,8V10H14V5H16V8H19Z";
    private const string SourceCloseIconPath = "M19,6.41L17.59,5L12,10.59L6.41,5L5,6.41L10.59,12L5,17.59L6.41,19L12,13.41L17.59,19L19,17.59L13.41,12L19,6.41Z";
    private const string SourceExternalIconPath = "M14,3V5H17.59L7.76,14.83L9.17,16.24L19,6.41V10H21V3M19,19H5V5H12V3H5C3.89,3 3,3.9 3,5V19A2,2 0 0,0 5,21H19A2,2 0 0,0 21,19V12H19V19Z";
    private const string GuideSparkleIconPath = "M12,2L14.4,8.6L21,11L14.4,13.4L12,20L9.6,13.4L3,11L9.6,8.6L12,2M19,15L20.1,17.9L23,19L20.1,20.1L19,23L17.9,20.1L15,19L17.9,17.9L19,15Z";
    private const string ChevronUpIconPath = "M7.41,15.41L12,10.83L16.59,15.41L18,14L12,8L6,14L7.41,15.41Z";
    private const string ChevronDownIconPath = "M7.41,8.58L12,13.17L16.59,8.58L18,10L12,16L6,10L7.41,8.58Z";

    private bool _sourcePreviewOpen;
    private bool _sourceFullscreen;
    private PathIcon? _sourceFullscreenIcon;
    private WebView2? _sourcePreviewWebView;

    private static Button MakePaneIconBtn(string path, string tip, RoutedEventHandler onClick, out PathIcon iconRef)
    {
        iconRef = new PathIcon
        {
            Width = 24,
            Height = 24,
            Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), path),
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
        };
        var btn = new Button
        {
            Content = new Viewbox
            {
                Width = 16,
                Height = 16,
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
                Child = iconRef,
            },
            MinWidth = 34,
            Width = 34,
            MinHeight = 34,
            Height = 34,
            Padding = new Thickness(0),
            CornerRadius = new CornerRadius(17),
            HorizontalContentAlignment = HorizontalAlignment.Center,
            VerticalContentAlignment = VerticalAlignment.Center,
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
        };
        ToolTipService.SetToolTip(btn, tip);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(btn, tip);
        btn.Click += onClick;
        return btn;
    }

    private void DisposeSourcePreviewWebView()
    {
        if (_sourcePreviewWebView != null)
        {
            try { _sourcePreviewWebView.Close(); } catch { }
            _sourcePreviewWebView = null;
        }
    }

    private void CloseSourcePreview()
    {
        _sourcePreviewOpen = false;
        _sourceFullscreen = false;
        _sourceFullscreenIcon = null;
        DisposeSourcePreviewWebView();
        SourceDetailHost.Content = null;
        SourceDetailHost.Visibility = Visibility.Collapsed;
        SourcesHomeView.Visibility = Visibility.Visible;
        ApplyPaneLayout();
    }

    private void ToggleSourceFullscreen()
    {
        _sourceFullscreen = !_sourceFullscreen;
        if (_sourceFullscreen) _studioFullscreen = false;
        if (_sourceFullscreenIcon != null)
        {
            _sourceFullscreenIcon.Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(
                typeof(Microsoft.UI.Xaml.Media.Geometry),
                _sourceFullscreen ? SourceExitFullscreenIconPath : SourceFullscreenIconPath);
        }
        AnimatePaneLayout(SourcesPaneBorder, _sourceFullscreen);
    }

    /// <summary>Builds the collapsible NotebookLM-style "来源指南" card with bolded summary and clickable topic pills.</summary>
    private string? _activeKgArtifactId;
    private StackPanel? _kgStepsPanel;
    private TextBlock? _kgStageLabel;
    private ProgressBar? _kgProgressBar;
    private ProgressRing? _kgProgressRing;
    private FontIcon? _kgDoneIcon;
    private Button? _kgDetailBtn;
    private Action? _onKgTerminalState;
    private readonly Dictionary<long, TextBlock> _kgStepBodies = new();

    private void OnKgThoughtEvent(JsonElement data)
    {
        if (_activeKgArtifactId == null || Str(data, "artifact_id") != _activeKgArtifactId) return;
        if (_kgStepsPanel == null) return;
        var kind = Str(data, "kind");
        if (kind == "retry") return;
        var seq = Int(data, "seq", 0);
        var title = Str(data, "title");
        var body = Str(data, "body");
        if (seq > 0 && _kgStepBodies.TryGetValue(seq, out var existing))
        {
            if (body.Length > existing.Text.Length)
            {
                existing.Text = body;
                existing.Visibility = Visibility.Visible;
            }
            return;
        }
        var dotColor = kind switch
        {
            "tool" => Windows.UI.Color.FromArgb(255, 120, 217, 236),
            "reasoning" => Windows.UI.Color.FromArgb(255, 168, 199, 250),
            "error" => Windows.UI.Color.FromArgb(255, 242, 139, 130),
            _ => Windows.UI.Color.FromArgb(255, 129, 201, 149),
        };
        var card = new StackPanel { Spacing = 2, Margin = new Thickness(0, 2, 0, 2) };
        var head = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        head.Children.Add(new Border
        {
            Width = 6, Height = 6, CornerRadius = new CornerRadius(3),
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(dotColor),
            VerticalAlignment = VerticalAlignment.Center,
        });
        head.Children.Add(new TextBlock
        {
            Text = title.Length > 0 ? title : kind,
            FontSize = 11.5,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
            TextWrapping = TextWrapping.Wrap,
            Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 224, 236, 252)),
        });
        card.Children.Add(head);
        var bodyTb = new TextBlock
        {
            Text = body,
            FontSize = 11,
            TextWrapping = TextWrapping.Wrap,
            MaxLines = 3,
            Margin = new Thickness(12, 0, 0, 0),
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
            Visibility = body.Length > 0 ? Visibility.Visible : Visibility.Collapsed,
        };
        if (seq > 0) _kgStepBodies[seq] = bodyTb;
        card.Children.Add(bodyTb);
        _kgStepsPanel.Children.Add(card);
        if (_kgStageLabel != null && title.Length > 0) _kgStageLabel.Text = title;
        if (title == "成品生成完成")
        {
            SetKgBuildingCompletedUi(succeeded: true);
        }
    }

    private void SetKgBuildingCompletedUi(bool succeeded)
    {
        if (_kgProgressRing != null)
        {
            _kgProgressRing.IsActive = false;
            _kgProgressRing.Visibility = Visibility.Collapsed;
        }
        if (_kgDoneIcon != null && succeeded)
        {
            _kgDoneIcon.Visibility = Visibility.Visible;
        }
        if (_kgDetailBtn != null)
        {
            _kgDetailBtn.Visibility = Visibility.Collapsed;
        }
        if (_kgProgressBar != null)
        {
            _kgProgressBar.IsIndeterminate = false;
            if (succeeded)
            {
                _kgProgressBar.Value = 100;
                _kgProgressBar.Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 129, 201, 149));
            }
        }
    }

    private void OnKgThoughtDeltaEvent(JsonElement data)
    {
        if (_activeKgArtifactId == null || Str(data, "artifact_id") != _activeKgArtifactId) return;
        var seq = Int(data, "seq", 0);
        var delta = Str(data, "delta");
        if (seq > 0 && delta.Length > 0 && _kgStepBodies.TryGetValue(seq, out var tb))
        {
            tb.Text += delta;
            tb.Visibility = Visibility.Visible;
        }
    }

    private void OnKgStatusEvent(JsonElement data)
    {
        if (_activeKgArtifactId == null || Str(data, "artifact_id") != _activeKgArtifactId) return;
        var status = Str(data, "status");
        var isTerminal = status is "READY" or "FAILED" or "CANCELLED";
        if (isTerminal)
        {
            SetKgBuildingCompletedUi(succeeded: status == "READY");
            var cb = _onKgTerminalState;
            _onKgTerminalState = null;
            cb?.Invoke();
            return;
        }
        if (_kgProgressBar != null)
        {
            var prog = Num(data, "progress");
            _kgProgressBar.IsIndeterminate = prog <= 0.01;
            _kgProgressBar.Value = prog * 100;
        }
    }

    private FrameworkElement BuildSourceGuideCard(
        string sourceId,
        string sourceTitle,
        string guideSummaryMd,
        List<string> guideTopics,
        bool recommendKnowledgeGraph = false,
        JsonElement? kgArtifact = null,
        Action? onOpenKg = null,
        Action? onRebuildKg = null,
        Action? onViewKgProgress = null)
    {
        var expanded = true;
        var cardBg = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 36, 40, 56));
        var cardBorder = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 56, 62, 86));

        var outer = new StackPanel { Spacing = 10 };

        // Header row: sparkle icon + "来源指南" + collapse chevron
        var header = new Grid { ColumnSpacing = 8 };
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        var leftHead = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
        leftHead.Children.Add(new Viewbox
        {
            Width = 16,
            Height = 16,
            VerticalAlignment = VerticalAlignment.Center,
            Child = new PathIcon
            {
                Width = 24,
                Height = 24,
                Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Microsoft.UI.Xaml.Media.Geometry), GuideSparkleIconPath),
                Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 224, 232, 255)),
            },
        });
        leftHead.Children.Add(new TextBlock
        {
            Text = L10n.T("sources.guide_title"),
            FontSize = 14,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
            Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 241, 243, 244)),
            VerticalAlignment = VerticalAlignment.Center,
        });
        header.Children.Add(leftHead);

        var body = new StackPanel { Spacing = 10 };
        var chevronBtn = MakePaneIconBtn(ChevronUpIconPath, L10n.T("sources.guide_toggle_tip"), (_, _) => { }, out var chevronIcon);
        chevronBtn.Width = 28;
        chevronBtn.MinWidth = 28;
        chevronBtn.Height = 28;
        chevronBtn.MinHeight = 28;
        chevronBtn.Click += (_, _) =>
        {
            expanded = !expanded;
            body.Visibility = expanded ? Visibility.Visible : Visibility.Collapsed;
            chevronIcon.Data = (Microsoft.UI.Xaml.Media.Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(
                typeof(Microsoft.UI.Xaml.Media.Geometry),
                expanded ? ChevronUpIconPath : ChevronDownIconPath);
        };
        Grid.SetColumn(chevronBtn, 1);
        header.Children.Add(chevronBtn);
        outer.Children.Add(header);

        if (!string.IsNullOrWhiteSpace(guideSummaryMd))
        {
            var tb = new TextBlock
            {
                FontSize = 13,
                LineHeight = 22,
                TextWrapping = TextWrapping.Wrap,
                IsTextSelectionEnabled = true,
                Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 206, 212, 228)),
            };
            // Parse **bold** inline segments
            var raw = guideSummaryMd.Trim();
            var idx = 0;
            while (idx < raw.Length)
            {
                var open = raw.IndexOf("**", idx, StringComparison.Ordinal);
                if (open < 0)
                {
                    tb.Inlines.Add(new Run { Text = raw[idx..] });
                    break;
                }
                if (open > idx)
                {
                    tb.Inlines.Add(new Run { Text = raw[idx..open] });
                }
                var close = raw.IndexOf("**", open + 2, StringComparison.Ordinal);
                if (close < 0)
                {
                    tb.Inlines.Add(new Run { Text = raw[open..] });
                    break;
                }
                var boldRun = new Run
                {
                    Text = raw[(open + 2)..close],
                    FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                    Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 248, 250, 255)),
                };
                tb.Inlines.Add(boldRun);
                idx = close + 2;
            }
            body.Children.Add(tb);

            if (guideTopics.Count > 0)
            {
                // True horizontal flow-wrap layout: fills each row before wrapping to next line
                var wrapPanel = new FlowWrapPanel { HorizontalSpacing = 8, VerticalSpacing = 8, Margin = new Thickness(0, 2, 0, 0) };
                foreach (var topic in guideTopics)
                {
                    var t = topic.Trim();
                    if (t.Length == 0) continue;
                    var chipText = new TextBlock
                    {
                        Text = t,
                        FontFamily = Md.BodyFontFamily,
                        FontSize = 12.5,
                        MaxLines = 1,
                        TextTrimming = TextTrimming.CharacterEllipsis,
                        Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 224, 229, 242)),
                    };
                    var chip = new Button
                    {
                        Content = chipText,
                        Padding = new Thickness(12, 5, 12, 5),
                        CornerRadius = new CornerRadius(14),
                        Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 48, 53, 73)),
                        BorderBrush = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 68, 74, 102)),
                        BorderThickness = new Thickness(1),
                    };
                    ToolTipService.SetToolTip(chip, t);
                    var capturedTopic = t;
                    chip.Click += async (_, _) =>
                    {
                        await SendChatAsync(L10n.Fmt("sources.guide_ask_prompt", sourceTitle, capturedTopic));
                    };
                    wrapPanel.Children.Add(chip);
                }
                body.Children.Add(wrapPanel);
            }

            // Learning tool: Knowledge Graph attached outcome (with understanding progress)
            var kgStatus = kgArtifact is { ValueKind: JsonValueKind.Object } kgObj ? Str(kgObj, "status") : "";
            var isKgReady = kgStatus == "READY";
            var isKgBuilding = kgStatus is "PENDING" or "RUNNING";
            var kgBar = new Border
            {
                Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(
                    recommendKnowledgeGraph || isKgReady
                        ? Windows.UI.Color.FromArgb(255, 34, 52, 68)
                        : Windows.UI.Color.FromArgb(255, 40, 44, 60)),
                BorderBrush = new Microsoft.UI.Xaml.Media.SolidColorBrush(
                    recommendKnowledgeGraph || isKgReady
                        ? Windows.UI.Color.FromArgb(255, 82, 134, 186)
                        : Windows.UI.Color.FromArgb(255, 62, 68, 92)),
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(10),
                Padding = new Thickness(10, 8, 10, 8),
                Margin = new Thickness(0, 4, 0, 0),
            };
            var kgRow = new Grid { ColumnSpacing = 10 };
            kgRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            kgRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

            if (isKgReady && kgArtifact is { } readyKg)
            {
                _activeKgArtifactId = null;
                _kgProgressRing = null;
                _kgDoneIcon = null;
                _kgDetailBtn = null;
                _kgStepsPanel = null;
                _kgStageLabel = null;
                _kgProgressBar = null;
                _onKgTerminalState = null;
                var (mastered, total, pct) = ComputeKgMasteryCounts(readyKg);
                var leftStack = new StackPanel { Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
                leftStack.Children.Add(new TextBlock
                {
                    Text = $"{L10n.T("kg.title")} · {L10n.Fmt("kg.progress_summary_fmt", mastered, total, pct)}",
                    FontFamily = Md.BodyFontFamily,
                    FontSize = 12,
                    TextWrapping = TextWrapping.Wrap,
                    Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 212, 228, 248)),
                });
                leftStack.Children.Add(new ProgressBar
                {
                    Minimum = 0,
                    Maximum = 100,
                    Value = pct,
                    Height = 4,
                    CornerRadius = new CornerRadius(2),
                    Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 129, 201, 149)),
                    Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(110, 74, 88, 112)),
                });
                kgRow.Children.Add(leftStack);

                var btnRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
                if (onOpenKg != null)
                {
                    var openBtn = new Button
                    {
                        Content = L10n.T("kg.open_btn"),
                        Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"],
                        MinHeight = 30,
                        Padding = new Thickness(12, 4, 12, 4),
                        FontSize = 12,
                    };
                    openBtn.Click += (_, _) => onOpenKg();
                    btnRow.Children.Add(openBtn);
                }
                var rebuildBtn = new Button
                {
                    Content = L10n.T("kg.rebuild_btn"),
                    Style = (Style)Application.Current.Resources["PillButtonStyle"],
                    MinHeight = 30,
                    Padding = new Thickness(10, 4, 10, 4),
                    FontSize = 12,
                };
                rebuildBtn.Click += (_, _) => onRebuildKg?.Invoke();
                btnRow.Children.Add(rebuildBtn);
                Grid.SetColumn(btnRow, 1);
                kgRow.Children.Add(btnRow);
            }
            else if (isKgBuilding)
            {
                _activeKgArtifactId = kgArtifact is { } ak ? Str(ak, "id") : null;
                _kgStepBodies.Clear();
                var buildingStack = new StackPanel { Spacing = 6 };
                var topLine = new Grid { ColumnSpacing = 8 };
                topLine.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                topLine.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

                var buildingRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
                _kgProgressRing = new ProgressRing { IsActive = true, Width = 14, Height = 14 };
                _kgDoneIcon = new FontIcon
                {
                    Glyph = "\uE73E",
                    FontSize = 13,
                    Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 129, 201, 149)),
                    VerticalAlignment = VerticalAlignment.Center,
                    Visibility = Visibility.Collapsed,
                };
                buildingRow.Children.Add(_kgProgressRing);
                buildingRow.Children.Add(_kgDoneIcon);
                _kgStageLabel = new TextBlock
                {
                    Text = L10n.T("kg.building_inline"),
                    FontFamily = Md.BodyFontFamily,
                    FontSize = 12,
                    FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                    TextWrapping = TextWrapping.Wrap,
                    Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 212, 228, 248)),
                };
                buildingRow.Children.Add(_kgStageLabel);
                topLine.Children.Add(buildingRow);

                _kgDetailBtn = null;
                if (onViewKgProgress != null)
                {
                    _kgDetailBtn = new Button
                    {
                        Content = L10n.T("art.view_process"),
                        Style = (Style)Application.Current.Resources["PillButtonStyle"],
                        MinHeight = 26,
                        Padding = new Thickness(10, 2, 10, 2),
                        FontSize = 11.5,
                    };
                    _kgDetailBtn.Click += (_, _) => onViewKgProgress();
                    Grid.SetColumn(_kgDetailBtn, 1);
                    topLine.Children.Add(_kgDetailBtn);
                }
                buildingStack.Children.Add(topLine);

                var prog = kgArtifact is { } pk ? Num(pk, "progress") : 0.0;
                _kgProgressBar = new ProgressBar
                {
                    IsIndeterminate = prog <= 0.01,
                    Value = prog * 100,
                    Maximum = 100,
                    Height = 4,
                    CornerRadius = new CornerRadius(2),
                };
                buildingStack.Children.Add(_kgProgressBar);

                _kgStepsPanel = new StackPanel { Spacing = 4 };
                if (kgArtifact is { } ak2)
                {
                    foreach (var th in Arr(ak2, "thoughts"))
                    {
                        OnKgThoughtEvent(th);
                    }
                }
                var stepsScroll = new ScrollViewer
                {
                    MaxHeight = 140,
                    VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
                    HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
                    Content = _kgStepsPanel,
                };
                buildingStack.Children.Add(stepsScroll);
                Grid.SetColumnSpan(buildingStack, 2);
                kgRow.Children.Add(buildingStack);
            }
            else
            {
                kgRow.Children.Add(new TextBlock
                {
                    Text = recommendKnowledgeGraph ? L10n.T("kg.recommended_banner") : L10n.T("kg.title"),
                    FontFamily = Md.BodyFontFamily,
                    FontSize = 12,
                    TextWrapping = TextWrapping.Wrap,
                    VerticalAlignment = VerticalAlignment.Center,
                    Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 212, 228, 248)),
                });
                var kgBtn = new Button
                {
                    Content = L10n.T("kg.build_btn"),
                    Style = (Style)Application.Current.Resources[recommendKnowledgeGraph ? "PrimaryPillButtonStyle" : "PillButtonStyle"],
                    MinHeight = 30,
                    Padding = new Thickness(12, 4, 12, 4),
                    FontSize = 12,
                };
                kgBtn.Click += (_, _) => onRebuildKg?.Invoke();
                Grid.SetColumn(kgBtn, 1);
                kgRow.Children.Add(kgBtn);
            }
            kgBar.Child = kgRow;
            body.Children.Add(kgBar);
        }
        else
        {
            var hintRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, VerticalAlignment = VerticalAlignment.Center };
            hintRow.Children.Add(new TextBlock
            {
                Text = L10n.T("sources.guide_missing"),
                FontSize = 12,
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
                VerticalAlignment = VerticalAlignment.Center,
                TextWrapping = TextWrapping.Wrap,
            });
            var genBtn = new Button
            {
                Content = L10n.T("sources.gen_guide"),
                FontSize = 12,
                Padding = new Thickness(10, 4, 10, 4),
                CornerRadius = new CornerRadius(12),
            };
            genBtn.Click += async (_, _) =>
            {
                genBtn.IsEnabled = false;
                genBtn.Content = L10n.T("sources.guide_generating");
                var g = await CoreAsync("source.generate_guide", Args(("source_id", sourceId)), L10n.T("sources.op_gen_guide"));
                if (g != null)
                {
                    await ShowSourcePreviewAsync(sourceId, null, null, sourceTitle);
                }
                else
                {
                    genBtn.IsEnabled = true;
                    genBtn.Content = L10n.T("sources.gen_guide");
                }
            };
            hintRow.Children.Add(genBtn);
            body.Children.Add(hintRow);
        }

        outer.Children.Add(body);

        return new Border
        {
            Background = cardBg,
            BorderBrush = cardBorder,
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(12),
            Padding = new Thickness(14, 12, 14, 12),
            Child = outer,
        };
    }

    private static (int Mastered, int Total, int Percent) ComputeKgMasteryCounts(JsonElement kgArtifact)
    {
        if (!kgArtifact.TryGetProperty("payload", out var payload) || payload.ValueKind != JsonValueKind.Object)
            return (0, 0, 0);
        var nodes = Arr(payload, "nodes").ToList();
        var conceptNodes = nodes.Count > 1
            ? nodes.Where(n => n.TryGetProperty("parent_id", out var p) && p.ValueKind == JsonValueKind.String).ToList()
            : nodes;
        var total = conceptNodes.Count;
        if (total == 0) return (0, 0, 0);
        var mastered = 0;
        if (kgArtifact.TryGetProperty("progress_state", out var ps) && ps.ValueKind == JsonValueKind.Object
            && ps.TryGetProperty("mastery", out var m) && m.ValueKind == JsonValueKind.Object)
        {
            foreach (var n in conceptNodes)
            {
                var nid = Str(n, "id");
                if (nid.Length > 0 && m.TryGetProperty(nid, out var entry) && Str(entry, "status") == "mastered")
                    mastered++;
            }
        }
        return (mastered, total, (mastered * 100) / total);
    }

    private async Task PollSourceKnowledgeGraphAsync(string sourceId, string kgId, int? page, string? focusText, string title)
    {
        for (var i = 0; i < 60 && _sourcePreviewOpen; i++)
        {
            await Task.Delay(2000);
            if (!_sourcePreviewOpen) return;
            var res = await Api.CallAsync("studio.get", Args(("artifact_id", kgId)));
            if (res.Result is { } art)
            {
                var st = Str(art, "status");
                if (st is "READY" or "FAILED" or "CANCELLED")
                {
                    await ShowSourcePreviewAsync(sourceId, page, focusText, title);
                    return;
                }
            }
        }
    }

    private static bool IsCodeExtension(string ext) => ext is
        ".py" or ".rs" or ".js" or ".ts" or ".jsx" or ".tsx" or ".c" or ".cpp" or ".h" or ".hpp" or
        ".cs" or ".java" or ".go" or ".sql" or ".json" or ".jsonl" or ".yaml" or ".yml" or ".toml" or
        ".xml" or ".ini" or ".cfg" or ".sh" or ".ps1" or ".bat";

    private async Task<UIElement> BuildOriginalFileViewAsync(
        string kind,
        string blobPath,
        string previewHtmlPath,
        int? page,
        string? focusText,
        List<JsonElement> chunks)
    {
        var hasBlob = !string.IsNullOrWhiteSpace(blobPath) && File.Exists(blobPath);
        var ext = hasBlob ? Path.GetExtension(blobPath).ToLowerInvariant() : "";

        // 1. PDF -> Native WebView2 Chromium PDF Viewer with #page=N jump
        if (kind == "pdf" && hasBlob)
        {
            return await CreateWebViewForFileAsync(blobPath, page is > 0 ? $"#page={page}" : "");
        }

        // PDF always shows the original file; never fall back to the (possibly mis-recognised) OCR text layer.
        if (kind == "pdf")
        {
            return new TextBlock
            {
                Text = L10n.T("sources.original_missing"),
                TextWrapping = TextWrapping.Wrap,
                Margin = new Thickness(4, 12, 4, 4),
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
            };
        }

        // 2. Office / EPUB / CSV / RTF / HWP -> Clean dark HTML preview in WebView2
        if (!string.IsNullOrWhiteSpace(previewHtmlPath) && File.Exists(previewHtmlPath))
        {
            return await CreateWebViewForFileAsync(previewHtmlPath, page is > 0 ? $"#page-{page}" : "");
        }

        // 3. Web / HTML -> WebView2 sandboxed view
        if ((kind == "web" || ext is ".html" or ".htm" or ".xhtml") && hasBlob)
        {
            return await CreateWebViewForFileAsync(blobPath, "");
        }

        // 4. Image -> Full zoomable HTML image wrapper in WebView2 (supports png/jpg/webp/gif/avif/bmp/ico)
        if (kind == "image" && hasBlob)
        {
            var dir = Path.GetDirectoryName(blobPath)!;
            var fileName = Path.GetFileName(blobPath);
            var html = $"<!DOCTYPE html><html><head><meta charset='utf-8'><style>body{{margin:0;background:#141720;display:flex;justify-content:center;align-items:flex-start;min-height:100vh;padding:12px;box-sizing:border-box;}}img{{max-width:100%;height:auto;border-radius:8px;box-shadow:0 4px 20px rgba(0,0,0,.45);}}</style></head><body><img src='https://source-preview.local/{Uri.EscapeDataString(fileName)}' /></body></html>";
            return await CreateWebViewWithHtmlAndDirAsync(dir, html);
        }

        // 5. Audio / Video -> Native HTML5 player + AI transcript below
        if ((kind == "audio" || kind == "video") && hasBlob)
        {
            var dir = Path.GetDirectoryName(blobPath)!;
            var fileName = Path.GetFileName(blobPath);
            var mediaTag = kind == "video"
                ? $"<video controls style='width:100%;max-height:360px;border-radius:8px;background:#000;' src='https://source-preview.local/{Uri.EscapeDataString(fileName)}'></video>"
                : $"<audio controls style='width:100%;margin:8px 0;' src='https://source-preview.local/{Uri.EscapeDataString(fileName)}'></audio>";
            var transcript = string.Join("\n\n", chunks.Select(c => Str(c, "content")));
            var escTranscript = System.Net.WebUtility.HtmlEncode(transcript);
            var html = $"<!DOCTYPE html><html><head><meta charset='utf-8'><style>body{{margin:0;padding:14px;background:#1B1E28;color:#E8EAED;font:13.5px/1.7 'Microsoft YaHei UI',sans-serif;}}.tr{{margin-top:14px;white-space:pre-wrap;background:#232836;padding:12px;border-radius:8px;}}</style></head><body>{mediaTag}{(transcript.Length > 0 ? $"<div class='tr'>{escTranscript}</div>" : "")}</body></html>";
            return await CreateWebViewWithHtmlAndDirAsync(dir, html);
        }

        // 6. Markdown / Note -> Full original file rendered with Md.Render (with aligned KaTeX)
        if ((kind is "md" or "note" || ext is ".md" or ".markdown" or ".mdx") && hasBlob)
        {
            var rawMd = await File.ReadAllTextAsync(blobPath);
            var rendered = Md.Render(rawMd, null, 14, _math, MathRenderHost);
            return new ScrollViewer
            {
                Content = new Border
                {
                    Padding = new Thickness(4, 4, 10, 20),
                    Child = rendered,
                },
            };
        }

        // 7. Plain text / Code -> Full original file text
        if (hasBlob)
        {
            var rawTxt = await File.ReadAllTextAsync(blobPath);
            var tb = new TextBlock
            {
                Text = rawTxt,
                TextWrapping = TextWrapping.Wrap,
                IsTextSelectionEnabled = true,
                FontSize = IsCodeExtension(ext) ? 12.5 : 13.5,
                LineHeight = 22,
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"],
            };
            if (IsCodeExtension(ext))
            {
                tb.FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Consolas, Cascadia Mono, monospace");
            }
            return new ScrollViewer
            {
                Content = new Border
                {
                    Padding = new Thickness(4, 4, 10, 20),
                    Child = tb,
                },
            };
        }

        return BuildChunksFallbackView(chunks, page, focusText);
    }

    private async Task<UIElement> CreateWebViewForFileAsync(string filePath, string fragment)
    {
        DisposeSourcePreviewWebView();
        var wv = new WebView2
        {
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            MinHeight = 320,
        };
        _sourcePreviewWebView = wv;
        var dir = Path.GetDirectoryName(filePath) ?? ".";
        var name = Path.GetFileName(filePath);
        wv.Loaded += async (_, _) =>
        {
            try
            {
                await wv.EnsureCoreWebView2Async();
                if (wv.CoreWebView2 == null) return;
                wv.CoreWebView2.SetVirtualHostNameToFolderMapping(
                    "source-preview.local",
                    dir,
                    CoreWebView2HostResourceAccessKind.Allow);
                wv.CoreWebView2.Navigate($"https://source-preview.local/{Uri.EscapeDataString(name)}{fragment}");
            }
            catch { }
        };
        return new Border
        {
            CornerRadius = new CornerRadius(8),
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 24, 27, 36)),
            Child = wv,
        };
    }

    private async Task<UIElement> CreateWebViewWithHtmlAndDirAsync(string dir, string html)
    {
        DisposeSourcePreviewWebView();
        var wv = new WebView2
        {
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            MinHeight = 320,
        };
        _sourcePreviewWebView = wv;
        wv.Loaded += async (_, _) =>
        {
            try
            {
                await wv.EnsureCoreWebView2Async();
                if (wv.CoreWebView2 == null) return;
                wv.CoreWebView2.SetVirtualHostNameToFolderMapping(
                    "source-preview.local",
                    dir,
                    CoreWebView2HostResourceAccessKind.Allow);
                wv.CoreWebView2.NavigateToString(html);
            }
            catch { }
        };
        return new Border
        {
            CornerRadius = new CornerRadius(8),
            Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 24, 27, 36)),
            Child = wv,
        };
    }

    private UIElement BuildChunksFallbackView(List<JsonElement> chunks, int? page, string? focusText)
    {
        var panel = new StackPanel { Spacing = 10, Padding = new Thickness(0, 0, 8, 8) };
        var needle = (focusText ?? "").Trim();
        if (needle.Length > 60) needle = needle[..60];
        foreach (var c in chunks)
        {
            var content = Str(c, "content");
            var p = (int)Int(c, "page_start");
            panel.Children.Add(new TextBlock
            {
                FontSize = 11,
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
                Text = $"{L10n.Fmt("sources.chunk_meta", Int(c, "chunk_index") + 1)}{(p > 0 ? L10n.Fmt("sources.chunk_page", p) : "")}{(Str(c, "section_path").Length > 0 ? $" · {Str(c, "section_path")}" : "")}",
            });
            var hit = (needle.Length > 0 && content.Contains(needle)) || (needle.Length == 0 && page is { } pg && p == pg);
            panel.Children.Add(new Border
            {
                Background = hit
                    ? new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 40, 60, 90))
                    : (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"],
                Padding = new Thickness(10),
                CornerRadius = new CornerRadius(8),
                Child = Line(content),
            });
        }
        if (chunks.Count == 0)
        {
            var empty = Line(L10n.T("sources.empty_content"));
            empty.Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"];
            panel.Children.Add(empty);
        }
        return new ScrollViewer { Content = panel };
    }

    private UIElement BuildIndexTreeView(List<JsonElement> indexNodes, List<JsonElement> chunks, Action<int> onJumpToPage)
    {
        if (indexNodes.Count == 0)
        {
            return BuildChunksFallbackView(chunks, null, null);
        }
        var panel = new StackPanel { Spacing = 8, Padding = new Thickness(0, 2, 8, 12) };
        foreach (var n in indexNodes)
        {
            var level = (int)Int(n, "level");
            var title = Str(n, "title");
            var summary = Str(n, "summary");
            var ps = (int)Int(n, "page_start");
            var pe = (int)Int(n, "page_end");
            var pageLabel = ps > 0 ? (pe > ps ? L10n.Fmt("sources.page_range", ps, pe) : L10n.Fmt("sources.page_single", ps)) : "";

            var inner = new StackPanel { Spacing = 4 };
            var topRow = new Grid { ColumnSpacing = 8 };
            topRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            topRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            topRow.Children.Add(new TextBlock
            {
                Text = title,
                FontSize = level <= 1 ? 13.5 : 12.5,
                FontWeight = level <= 1 ? Microsoft.UI.Text.FontWeights.SemiBold : Microsoft.UI.Text.FontWeights.Normal,
                Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"],
                TextWrapping = TextWrapping.Wrap,
            });
            if (pageLabel.Length > 0)
            {
                var pTxt = new TextBlock
                {
                    Text = pageLabel,
                    FontSize = 11,
                    Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 138, 180, 248)),
                    VerticalAlignment = VerticalAlignment.Center,
                };
                Grid.SetColumn(pTxt, 1);
                topRow.Children.Add(pTxt);
            }
            inner.Children.Add(topRow);

            if (!string.IsNullOrWhiteSpace(summary))
            {
                inner.Children.Add(new TextBlock
                {
                    Text = summary,
                    FontSize = 12,
                    Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextSecondaryBrush"],
                    TextWrapping = TextWrapping.Wrap,
                });
            }

            var itemBorder = new Border
            {
                Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["ElevatedBgBrush"],
                CornerRadius = new CornerRadius(8),
                Padding = new Thickness(10, 8, 10, 8),
                Margin = new Thickness(Math.Max(0, (level - 1) * 14), 0, 0, 0),
                Child = inner,
            };
            if (ps > 0)
            {
                var targetPage = ps;
                ToolTipService.SetToolTip(itemBorder, L10n.Fmt("sources.jump_page_tip", targetPage));
                itemBorder.Tapped += (_, _) => onJumpToPage(targetPage);
            }
            panel.Children.Add(itemBorder);
        }
        return new ScrollViewer { Content = panel };
    }

    /// <summary>Displays a collapsible "来源指南" card + native original file preview (with optional AI index tree view & fullscreen toggle).</summary>
    private async Task ShowSourcePreviewAsync(string sourceId, int? page, string? focusText, string? sourceTitle = null)
    {
        var res = await CoreAsync("source.preview", Args(("source_id", sourceId)), L10n.T("sources.op_preview"));
        if (res is not { } r) return;

        var title = !string.IsNullOrWhiteSpace(sourceTitle)
            ? sourceTitle
            : (Str(r, "title") is { Length: > 0 } t ? t : L10n.T("sources.material_fallback"));
        var kind = Str(r, "kind");
        var blobPath = Str(r, "blob_path");
        var previewHtmlPath = Str(r, "preview_html_path");
        var pageCount = (int)Int(r, "page_count");
        var guideSummaryMd = Str(r, "guide_summary_md");
        var guideTopics = Arr(r, "guide_topics").Select(x => x.GetString() ?? "").Where(s => s.Length > 0).ToList();
        var indexNodes = Arr(r, "index_nodes").ToList();
        var chunks = Arr(r, "chunks").ToList();

        var container = new Grid { RowSpacing = 10 };
        container.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto }); // Header
        container.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto }); // 来源指南
        container.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) }); // 原文件 / 索引视图

        // Row 0: Header
        var head = new Grid { ColumnSpacing = 8 };
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        var titleCol = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        titleCol.Children.Add(new TextBlock
        {
            Text = title,
            FontSize = 15,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextPrimaryBrush"],
            TextTrimming = TextTrimming.CharacterEllipsis,
            MaxLines = 2,
            TextWrapping = TextWrapping.Wrap,
        });
        var metaParts = new List<string>();
        if (kind.Length > 0) metaParts.Add(kind.ToUpperInvariant());
        if (pageCount > 0) metaParts.Add(L10n.Fmt("sources.pages_summary", pageCount));
        if (indexNodes.Count > 0) metaParts.Add(L10n.Fmt("sources.ai_nodes_summary", indexNodes.Count));
        titleCol.Children.Add(new TextBlock
        {
            Text = metaParts.Count > 0 ? string.Join(" · ", metaParts) : L10n.T("sources.original_preview"),
            FontSize = 11,
            Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextMutedBrush"],
        });
        head.Children.Add(titleCol);

        var tools = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        var contentHost = new ContentControl
        {
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            VerticalContentAlignment = VerticalAlignment.Stretch,
        };

        var showingIndex = false;
        var modeBtn = new Button
        {
            Content = L10n.T("sources.index_toc"),
            FontSize = 11.5,
            Padding = new Thickness(9, 4, 9, 4),
            CornerRadius = new CornerRadius(12),
            VerticalAlignment = VerticalAlignment.Center,
        };
        ToolTipService.SetToolTip(modeBtn, L10n.T("sources.toggle_mode_tip"));

        var kgRes = await Api.CallAsync("source.knowledge_graph", Args(("source_id", sourceId)));
        JsonElement? kgArtifact = kgRes.Result is { } kgWrap
            && kgWrap.TryGetProperty("artifact", out var kgProp)
            && kgProp.ValueKind == JsonValueKind.Object
            ? kgProp
            : null;
        var kgId = kgArtifact is { } kgA ? Str(kgA, "id") : "";
        var kgStatus = kgArtifact is { } kgB ? Str(kgB, "status") : "";

        async Task LoadOriginalAsync(int? targetPage)
        {
            showingIndex = false;
            modeBtn.Content = L10n.T("sources.index_toc");
            contentHost.Content = await BuildOriginalFileViewAsync(kind, blobPath, previewHtmlPath, targetPage, focusText, chunks);
        }

        void LoadKnowledgeGraphView()
        {
            if (kgId.Length > 0)
            {
                OpenArtifact(kgId);
            }
        }

        modeBtn.Click += async (_, _) =>
        {
            if (showingIndex)
            {
                await LoadOriginalAsync(page);
            }
            else
            {
                showingIndex = true;
                modeBtn.Content = L10n.T("sources.original_file");
                DisposeSourcePreviewWebView();
                contentHost.Content = BuildIndexTreeView(indexNodes, chunks, p => _ = LoadOriginalAsync(p));
            }
        };
        tools.Children.Add(modeBtn);

        if (kgStatus == "READY")
        {
            var kgViewBtn = new Button
            {
                Content = L10n.T("kg.title"),
                FontSize = 11.5,
                Padding = new Thickness(9, 4, 9, 4),
                CornerRadius = new CornerRadius(12),
                VerticalAlignment = VerticalAlignment.Center,
            };
            kgViewBtn.Click += (_, _) => LoadKnowledgeGraphView();
            tools.Children.Add(kgViewBtn);
        }

        if (!string.IsNullOrWhiteSpace(blobPath) && File.Exists(blobPath))
        {
            var openExtBtn = MakePaneIconBtn(SourceExternalIconPath, L10n.T("sources.open_external_tip"), (_, _) =>
            {
                try { Process.Start(new ProcessStartInfo(blobPath) { UseShellExecute = true }); } catch { }
            }, out _);
            tools.Children.Add(openExtBtn);
        }

        var fsBtn = MakePaneIconBtn(_sourceFullscreen ? SourceExitFullscreenIconPath : SourceFullscreenIconPath, L10n.T("studio.fullscreen_toggle"), (_, _) => ToggleSourceFullscreen(), out _sourceFullscreenIcon);
        var closeBtn = MakePaneIconBtn(SourceCloseIconPath, L10n.T("sources.back_to_list"), (_, _) => CloseSourcePreview(), out _);
        tools.Children.Add(fsBtn);
        tools.Children.Add(closeBtn);
        Grid.SetColumn(tools, 1);
        head.Children.Add(tools);
        container.Children.Add(head);

        // Row 1: Collapsible "来源指南" card
        var recommendKg = Bool(r, "recommend_knowledge_graph") || guideTopics.Count >= 3;
        var guideCard = BuildSourceGuideCard(
            sourceId,
            title,
            guideSummaryMd,
            guideTopics,
            recommendKg,
            kgArtifact,
            onOpenKg: kgStatus == "READY" ? LoadKnowledgeGraphView : null,
            onRebuildKg: async () =>
            {
                await CreateKnowledgeGraphAsync(sourceId, title);
                if (_sourcePreviewOpen) await ShowSourcePreviewAsync(sourceId, page, focusText, title);
            },
            onViewKgProgress: kgId.Length > 0 ? () => OpenArtifact(kgId) : null);
        Grid.SetRow(guideCard, 1);
        container.Children.Add(guideCard);

        // Row 2: Original file preview (default)
        Grid.SetRow(contentHost, 2);
        container.Children.Add(contentHost);

        _sourcePreviewOpen = true;
        if (_studioFullscreen)
        {
            _studioFullscreen = false;
            _activeArtifactPane?.SetFullscreenIcon(false);
        }
        SourceDetailHost.Content = container;
        SourcesHomeView.Visibility = Visibility.Collapsed;
        SourceDetailHost.Visibility = Visibility.Visible;
        ApplyPaneLayout();

        await LoadOriginalAsync(page);

        if (kgStatus is "PENDING" or "RUNNING" && kgId.Length > 0)
        {
            _onKgTerminalState = () =>
            {
                if (_sourcePreviewOpen) _ = ShowSourcePreviewAsync(sourceId, page, focusText, title);
            };
            _ = PollSourceKnowledgeGraphAsync(sourceId, kgId, page, focusText, title);
        }
        else
        {
            _onKgTerminalState = null;
        }
    }

    // ---------- notes ----------

    private async void OnCreateNoteClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var res = await CoreAsync("note.save", Args(("notebook_id", _activeNotebookId), ("title", L10n.T("note.new_title")), ("body_md", "")), L10n.T("note.op_create"));
        await RefreshNotebookDataAsync();
        if (res is { } r) await EditNoteAsync(Str(r, "id"));
    }

    private async Task EditNoteAsync(string noteId)
    {
        var note = await CoreAsync("note.get", Args(("note_id", noteId)), L10n.T("note.op_read"));
        if (note is not { } n || _activeNotebookId == null) return;
        var curTitle = Str(n, "title");
        var curBody = Str(n, "body_md");
        var origin = Str(n, "origin");

        while (true)
        {
            var titleBox = new TextBox { Header = L10n.T("common.title"), Text = curTitle, MinWidth = 520 };
            var bodyBox = new TextBox { Header = L10n.T("note.body_header"), Text = curBody, AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 260 };
            var panel = new StackPanel { Spacing = 10 };
            panel.Children.Add(titleBox);
            panel.Children.Add(bodyBox);
            if (origin == "chat") panel.Children.Add(new TextBlock { Text = L10n.T("note.origin_chat"), Opacity = 0.6, FontSize = 12 });
            var del = new Button { Content = L10n.T("note.delete_btn") };
            panel.Children.Add(del);
            var dlg = new ContentDialog
            {
                Title = L10n.T("note.dialog_title"),
                Content = panel,
                PrimaryButtonText = L10n.T("common.save"),
                SecondaryButtonText = L10n.T("note.to_source_btn"),
                CloseButtonText = L10n.T("common.cancel"),
                DefaultButton = ContentDialogButton.Primary,
                XamlRoot = Content.XamlRoot,
            };
            var wantDelete = false;
            del.Click += (_, _) =>
            {
                // Close this ContentDialog before opening the confirmation ContentDialog.
                wantDelete = true;
                dlg.Hide();
            };
            var result = await dlg.ShowAsync();
            curTitle = titleBox.Text;
            curBody = bodyBox.Text;
            if (wantDelete)
            {
                if (await ConfirmAsync(L10n.T("note.delete_btn"), L10n.T("note.delete_confirm"), L10n.T("common.delete")))
                {
                    await CoreAsync("note.delete", Args(("note_id", noteId)), L10n.T("note.delete_btn"));
                    break;
                }
                continue;
            }
            if (result == ContentDialogResult.Primary)
            {
                await CoreAsync("note.save", Args(("notebook_id", _activeNotebookId), ("id", noteId), ("title", curTitle), ("body_md", curBody)), L10n.T("note.op_save"));
            }
            else if (result == ContentDialogResult.Secondary)
            {
                await CoreAsync("note.save", Args(("notebook_id", _activeNotebookId), ("id", noteId), ("title", curTitle), ("body_md", curBody)), L10n.T("note.op_save"));
                if (await CoreAsync("note.to_source", Args(("note_id", noteId)), L10n.T("note.to_source_btn")) != null)
                    ShowStatus(InfoBarSeverity.Success, L10n.T("note.converted_title"), L10n.T("note.converted_msg"));
            }
            break;
        }
        await RefreshNotebookDataAsync();
    }
}
