using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Text.Json;
using System.Text.Json.Nodes;
using Windows.ApplicationModel.DataTransfer;
using Windows.Storage;
using Windows.Storage.Pickers;
using Windows.Storage.Streams;
using WinRT.Interop;

namespace LocalNotebook.Views;

/// <summary>Something waiting to be sent with the next message: a picture, a file or a link.</summary>
internal sealed class PendingItem
{
    /// <summary><c>image</c>, <c>file</c> or <c>link</c>.</summary>
    public required string Kind { get; init; }
    public required string Name { get; init; }
    public byte[]? Bytes { get; init; }
    public BitmapImage? Thumb { get; init; }
    public string? Url { get; init; }
}

/// <summary>
/// The "+ 添加上下文" control of a text input (chat, Studio request, outline revision, lecture
/// question): a menu to add pictures, one-off reference files and web links; Ctrl+V of a
/// screenshot or copied file; drag and drop; and a strip of removable thumbnails / file chips above
/// the input. Files are sent as they are: the core hands them to the agent, which reads them on demand.
/// </summary>
internal sealed class ImageAttachments
{
    public const int MaxImages = 6;
    private const long MaxImageBytes = 25L * 1024 * 1024;
    private const long MaxFileBytes = 200L * 1024 * 1024;
    private static readonly string[] ImageExtensions = { ".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp" };

    private readonly Func<nint> _hwnd;
    private readonly Func<XamlRoot?> _xamlRoot;
    private readonly Action<string>? _notify;
    private readonly List<PendingItem> _items = new();
    private readonly StackPanel _row = new() { Orientation = Orientation.Horizontal, Spacing = 8 };

    /// <summary>The attachment strip; collapsed while nothing is attached.</summary>
    public ScrollViewer Strip { get; }
    /// <summary>The "+" button opening the "添加上下文" menu.</summary>
    public Button AttachButton { get; }
    public bool HasItems => _items.Count > 0;
    public int Count => _items.Count;
    public event Action? Changed;

    public ImageAttachments(Func<nint> hwnd, Func<XamlRoot?> xamlRoot, Action<string>? notify = null)
    {
        _hwnd = hwnd;
        _xamlRoot = xamlRoot;
        _notify = notify;
        Strip = new ScrollViewer
        {
            Content = _row,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
            VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
            HorizontalScrollMode = ScrollMode.Auto,
            Visibility = Visibility.Collapsed,
        };
        AttachButton = new Button
        {
            Content = Icon(PlusPath, 18),
            Width = 36,
            Height = 36,
            MinWidth = 36,
            MinHeight = 36,
            Padding = new Thickness(0),
            CornerRadius = new CornerRadius(18),
            Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
            VerticalAlignment = VerticalAlignment.Center,
        };
        ApplyLocalization();
        AttachButton.Click += (_, _) => ShowMenu();
    }

    /// <summary>Re-reads the button's tooltip and accessible name after the UI language changed.</summary>
    public void ApplyLocalization()
    {
        ToolTipService.SetToolTip(AttachButton, L10n.T("ctx.add"));
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(AttachButton, L10n.T("ctx.add"));
    }

    private const string PlusPath = "M19,13H13V19H11V13H5V11H11V5H13V11H19V13Z";
    private const string ImagePath = "M21,19V5C21,3.9 20.1,3 19,3H5C3.9,3 3,3.9 3,5V19C3,20.1 3.9,21 5,21H19C20.1,21 21,20.1 21,19M8.5,13.5L11,16.51L14.5,12L19,18H5L8.5,13.5Z";
    private const string ClipPath = "M16.5,6V17.5A4,4 0 0,1 12.5,21.5A4,4 0 0,1 8.5,17.5V5A2.5,2.5 0 0,1 11,2.5A2.5,2.5 0 0,1 13.5,5V15.5A1,1 0 0,1 12.5,16.5A1,1 0 0,1 11.5,15.5V6H10V15.5A2.5,2.5 0 0,0 12.5,18A2.5,2.5 0 0,0 15,15.5V5A4,4 0 0,0 11,1A4,4 0 0,0 7,5V17.5A5.5,5.5 0 0,0 12.5,23A5.5,5.5 0 0,0 18,17.5V6H16.5Z";
    private const string LinkPath = "M3.9,12C3.9,10.29 5.29,8.9 7,8.9H11V7H7A5,5 0 0,0 2,12A5,5 0 0,0 7,17H11V15.1H7C5.29,15.1 3.9,13.71 3.9,12M8,13H16V11H8V13M17,7H13V8.9H17C18.71,8.9 20.1,10.29 20.1,12C20.1,13.71 18.71,15.1 17,15.1H13V17H17A5,5 0 0,0 22,12A5,5 0 0,0 17,7Z";
    private const string FilePath = "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20Z";
    private const string ClosePath = "M19,6.41L17.59,5L12,10.59L6.41,5L5,6.41L10.59,12L5,17.59L6.41,19L12,13.41L17.59,19L19,17.59L13.41,12L19,6.41Z";

    internal static FrameworkElement Icon(string data, double size, Brush? brush = null) => new Viewbox
    {
        Width = size,
        Height = size,
        HorizontalAlignment = HorizontalAlignment.Center,
        VerticalAlignment = VerticalAlignment.Center,
        Child = new PathIcon
        {
            Width = 24,
            Height = 24,
            Data = (Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Geometry), data),
            Foreground = brush ?? (Brush)Application.Current.Resources["TextSecondaryBrush"],
        },
    };

    // ---------- "添加上下文" menu ----------

    private void ShowMenu()
    {
        var panel = new StackPanel { Spacing = 2, MinWidth = 200 };
        panel.Children.Add(new TextBlock
        {
            Text = L10n.T("ctx.add"),
            FontSize = 12.5,
            Opacity = 0.6,
            Margin = new Thickness(10, 2, 10, 6),
        });
        var flyout = new Flyout { Content = panel, Placement = FlyoutPlacementMode.TopEdgeAlignedLeft };
        flyout.FlyoutPresenterStyle = new Style(typeof(FlyoutPresenter))
        {
            Setters =
            {
                new Setter(Control.PaddingProperty, new Thickness(6, 8, 6, 6)),
                new Setter(Control.CornerRadiusProperty, new CornerRadius(14)),
                new Setter(FrameworkElement.MinWidthProperty, 0.0),
            },
        };
        void AddItem(string path, string label, Func<Task> action)
        {
            var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
            row.Children.Add(Icon(path, 17));
            row.Children.Add(new TextBlock { Text = label, FontSize = 14.5, VerticalAlignment = VerticalAlignment.Center });
            var b = new Button
            {
                Content = row,
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Left,
                Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent),
                BorderThickness = new Thickness(0),
                CornerRadius = new CornerRadius(8),
                Padding = new Thickness(10, 8, 12, 8),
            };
            b.Click += async (_, _) =>
            {
                flyout.Hide();
                await action();
            };
            panel.Children.Add(b);
        }
        AddItem(ImagePath, L10n.T("ctx.image"), PickImagesAsync);
        AddItem(ClipPath, L10n.T("ctx.file"), PickFilesAsync);
        AddItem(LinkPath, L10n.T("ctx.link"), AskLinkAsync);
        flyout.ShowAt(AttachButton);
    }

    private async Task PickImagesAsync()
    {
        var picker = new FileOpenPicker();
        InitializeWithWindow.Initialize(picker, _hwnd());
        foreach (var ext in ImageExtensions) picker.FileTypeFilter.Add(ext);
        var files = await picker.PickMultipleFilesAsync();
        if (files is { Count: > 0 }) await AddFilesAsync(files);
    }

    private async Task PickFilesAsync()
    {
        var picker = new FileOpenPicker();
        InitializeWithWindow.Initialize(picker, _hwnd());
        // the core checks the format (the same list as adding a source) and says what is wrong
        picker.FileTypeFilter.Add("*");
        var files = await picker.PickMultipleFilesAsync();
        if (files is { Count: > 0 }) await AddFilesAsync(files);
    }

    private async Task AskLinkAsync()
    {
        var root = _xamlRoot();
        if (root == null) return;
        var box = new TextBox { PlaceholderText = "https://", MinWidth = 380 };
        var dlg = new ContentDialog
        {
            Title = L10n.T("ctx.link_title"),
            Content = box,
            PrimaryButtonText = L10n.T("ctx.link_add"),
            CloseButtonText = L10n.T("common.cancel"),
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = root,
        };
        if (await dlg.ShowAsync() != ContentDialogResult.Primary) return;
        var url = box.Text.Trim();
        if (!Uri.TryCreate(url, UriKind.Absolute, out var u) || (u.Scheme != "http" && u.Scheme != "https"))
        {
            _notify?.Invoke(L10n.T("ctx.link_bad"));
            return;
        }
        if (!Room()) return;
        Add(new PendingItem { Kind = "link", Name = u.Host + u.AbsolutePath.TrimEnd('/'), Url = url });
    }

    // ---------- drop / paste ----------

    /// <summary>Makes <paramref name="dropTarget"/> accept dropped pictures and files and, for a TextBox, Ctrl+V of them.</summary>
    public void Hook(FrameworkElement dropTarget, TextBox? box = null)
    {
        dropTarget.AllowDrop = true;
        dropTarget.DragOver += async (_, e) =>
        {
            var def = e.GetDeferral();
            try
            {
                if (await HasAttachableAsync(e.DataView))
                {
                    e.AcceptedOperation = DataPackageOperation.Copy;
                    e.DragUIOverride.Caption = L10n.T("ctx.drop");
                    e.Handled = true;
                }
            }
            finally { def.Complete(); }
        };
        dropTarget.Drop += async (_, e) =>
        {
            var def = e.GetDeferral();
            try
            {
                if (await HasAttachableAsync(e.DataView))
                {
                    e.Handled = true;
                    await AddFromPackageAsync(e.DataView);
                }
            }
            finally { def.Complete(); }
        };
        if (box != null)
        {
            // A TextBox raises Paste only when there is text to paste, so Ctrl+V on a screenshot is
            // caught here. Handled must be set before the first await, hence the synchronous checks.
            box.AddHandler(UIElement.KeyDownEvent, new Microsoft.UI.Xaml.Input.KeyEventHandler(async (_, e) =>
            {
                if (e.Key != Windows.System.VirtualKey.V) return;
                var ctrl = Microsoft.UI.Input.InputKeyboardSource
                    .GetKeyStateForCurrentThread(Windows.System.VirtualKey.Control)
                    .HasFlag(Windows.UI.Core.CoreVirtualKeyStates.Down);
                if (!ctrl) return;
                var view = Clipboard.GetContent();
                if (view.Contains(StandardDataFormats.Text)) return;
                if (!view.Contains(StandardDataFormats.Bitmap) && !view.Contains(StandardDataFormats.StorageItems)) return;
                e.Handled = true;
                if (!ClaimPaste()) return;
                await AddFromPackageAsync(view);
            }), true);
            box.Paste += async (_, e) =>
            {
                var view = Clipboard.GetContent();
                // text wins: a pasted picture or file is only taken when there is no text with it
                if (view.Contains(StandardDataFormats.Text)) return;
                // claimed synchronously so the KeyDown handler and this one never both take it
                if (!view.Contains(StandardDataFormats.Bitmap) && !view.Contains(StandardDataFormats.StorageItems)) return;
                e.Handled = true;
                if (!ClaimPaste()) return;
                if (!await HasAttachableAsync(view)) return;
                await AddFromPackageAsync(view);
            };
        }
    }

    private long _lastPasteAt;

    // One Ctrl+V can raise both KeyDown and Paste; only the first within a short window adds items.
    private bool ClaimPaste()
    {
        var now = Environment.TickCount64;
        if (now - _lastPasteAt < 700) return false;
        _lastPasteAt = now;
        return true;
    }

    private static async Task<bool> HasAttachableAsync(DataPackageView view)
    {
        if (view.Contains(StandardDataFormats.Bitmap)) return true;
        if (!view.Contains(StandardDataFormats.StorageItems)) return false;
        try
        {
            var items = await view.GetStorageItemsAsync();
            return items.OfType<StorageFile>().Any();
        }
        catch
        {
            return false;
        }
    }

    private static bool IsImage(string name) =>
        ImageExtensions.Contains(Path.GetExtension(name).ToLowerInvariant());

    private async Task AddFromPackageAsync(DataPackageView view)
    {
        if (view.Contains(StandardDataFormats.StorageItems))
        {
            var files = (await view.GetStorageItemsAsync()).OfType<StorageFile>().ToList();
            if (files.Count > 0)
            {
                await AddFilesAsync(files);
                return;
            }
        }
        if (view.Contains(StandardDataFormats.Bitmap))
        {
            var reference = await view.GetBitmapAsync();
            using var stream = await reference.OpenReadAsync();
            var bytes = new byte[stream.Size];
            await stream.ReadAsync(bytes.AsBuffer(), (uint)bytes.Length, InputStreamOptions.None);
            await AddImageBytesAsync($"{L10n.T("img.pasted")} {DateTime.Now:yyyyMMdd-HHmmss}.png", bytes);
        }
    }

    /// <summary>Pictures become picture attachments; anything else is a one-off reference file.</summary>
    public async Task AddFilesAsync(IEnumerable<StorageFile> files)
    {
        foreach (var f in files)
        {
            try
            {
                var props = await f.GetBasicPropertiesAsync();
                var image = IsImage(f.Name);
                var limit = image ? MaxImageBytes : MaxFileBytes;
                if (props.Size > (ulong)limit)
                {
                    _notify?.Invoke(L10n.Fmt("img.too_large", f.Name, limit / 1024 / 1024));
                    continue;
                }
                var buf = await FileIO.ReadBufferAsync(f);
                if (image) await AddImageBytesAsync(f.Name, buf.ToArray());
                else if (Room()) Add(new PendingItem { Kind = "file", Name = f.Name, Bytes = buf.ToArray() });
            }
            catch (Exception ex)
            {
                _notify?.Invoke(L10n.Fmt("img.read_failed", f.Name, ex.Message));
            }
        }
    }

    private bool Room()
    {
        if (_items.Count < MaxImages) return true;
        _notify?.Invoke(L10n.Fmt("ctx.too_many", MaxImages));
        return false;
    }

    private async Task AddImageBytesAsync(string name, byte[] bytes)
    {
        if (!Room()) return;
        if (bytes.Length == 0 || bytes.Length > MaxImageBytes)
        {
            _notify?.Invoke(L10n.Fmt("img.too_large", name, MaxImageBytes / 1024 / 1024));
            return;
        }
        var thumb = new BitmapImage { DecodePixelWidth = 160 };
        try
        {
            using var ms = new InMemoryRandomAccessStream();
            await ms.WriteAsync(bytes.AsBuffer());
            ms.Seek(0);
            await thumb.SetSourceAsync(ms);
        }
        catch
        {
            _notify?.Invoke(L10n.Fmt("img.bad_format", name));
            return;
        }
        Add(new PendingItem { Kind = "image", Name = name, Bytes = bytes, Thumb = thumb });
    }

    private void Add(PendingItem item)
    {
        _items.Add(item);
        _row.Children.Add(item.Kind == "image" ? Tile(item) : Chip(item));
        Refresh();
    }

    private UIElement RemoveButton(PendingItem item, FrameworkElement cell)
    {
        var remove = new Button
        {
            Content = new Viewbox
            {
                Child = new PathIcon
                {
                    Width = 24,
                    Height = 24,
                    Data = (Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Geometry), ClosePath),
                    Foreground = new SolidColorBrush(Microsoft.UI.Colors.White),
                },
            },
            Width = 22,
            Height = 22,
            MinWidth = 22,
            MinHeight = 22,
            Padding = new Thickness(3),
            CornerRadius = new CornerRadius(11),
            Background = new SolidColorBrush(Windows.UI.Color.FromArgb(210, 20, 20, 24)),
            BorderThickness = new Thickness(0),
            HorizontalAlignment = HorizontalAlignment.Right,
            VerticalAlignment = VerticalAlignment.Top,
            Margin = new Thickness(0, 3, 3, 0),
        };
        ToolTipService.SetToolTip(remove, L10n.T("img.remove"));
        remove.Click += (_, _) =>
        {
            _items.Remove(item);
            _row.Children.Remove(cell);
            Refresh();
        };
        return remove;
    }

    private UIElement Tile(PendingItem item)
    {
        var img = new Image { Source = item.Thumb, Stretch = Stretch.UniformToFill, Width = 68, Height = 68 };
        var frame = new Border
        {
            Width = 68,
            Height = 68,
            CornerRadius = new CornerRadius(12),
            BorderThickness = new Thickness(1),
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            Child = img,
        };
        var open = new Button
        {
            Content = frame,
            Padding = new Thickness(0),
            Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent),
            BorderThickness = new Thickness(0),
            CornerRadius = new CornerRadius(12),
        };
        ToolTipService.SetToolTip(open, item.Name);
        open.Click += async (_, _) => await ShowPreviewAsync(_xamlRoot(), item.Thumb, item.Bytes, item.Name);
        var cell = new Grid { Width = 68, Height = 68 };
        cell.Children.Add(open);
        cell.Children.Add(RemoveButton(item, cell));
        return cell;
    }

    /// <summary>A pending file or link: format icon, name and size/host, with a remove button.</summary>
    private UIElement Chip(PendingItem item)
    {
        var body = ChipBody(item.Kind, item.Name, item.Kind == "link" ? item.Url ?? "" : HumanSize(item.Bytes?.LongLength ?? 0));
        body.Margin = new Thickness(0, 0, 18, 0);
        var cell = new Grid { Height = 68, VerticalAlignment = VerticalAlignment.Top };
        cell.Children.Add(body);
        cell.Children.Add(RemoveButton(item, cell));
        return cell;
    }

    private static Border ChipBody(string kind, string name, string detail)
    {
        var ext = kind == "link" ? "" : Path.GetExtension(name).TrimStart('.').ToUpperInvariant();
        var badge = new Grid { Width = 38, Height = 38 };
        badge.Children.Add(new Border
        {
            CornerRadius = new CornerRadius(9),
            Background = new SolidColorBrush(Windows.UI.Color.FromArgb(0x1F, 0xFF, 0xFF, 0xFF)),
        });
        badge.Children.Add(Icon(kind == "link" ? LinkPath : FilePath, 20));
        var text = new StackPanel { VerticalAlignment = VerticalAlignment.Center, Spacing = 1, MaxWidth = 200 };
        text.Children.Add(new TextBlock { Text = name, FontSize = 13, TextTrimming = TextTrimming.CharacterEllipsis, MaxLines = 1 });
        text.Children.Add(new TextBlock
        {
            Text = ext.Length > 0 ? $"{ext} · {detail}" : detail,
            FontSize = 11.5,
            Opacity = 0.65,
            TextTrimming = TextTrimming.CharacterEllipsis,
            MaxLines = 1,
        });
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
        row.Children.Add(badge);
        row.Children.Add(text);
        var border = new Border
        {
            Height = 56,
            Padding = new Thickness(9, 8, 14, 8),
            CornerRadius = new CornerRadius(12),
            BorderThickness = new Thickness(1),
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            Child = row,
            VerticalAlignment = VerticalAlignment.Center,
            Margin = new Thickness(0, 6, 0, 0),
        };
        ToolTipService.SetToolTip(border, name);
        return border;
    }

    private static string HumanSize(long b) =>
        b <= 0 ? "" : b < 1024 * 1024 ? $"{Math.Max(1, b / 1024)} KB" : $"{b / 1024.0 / 1024.0:0.0} MB";

    private void Refresh()
    {
        Strip.Visibility = _items.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        AttachButton.IsEnabled = _items.Count < MaxImages;
        Changed?.Invoke();
    }

    public void Clear()
    {
        _items.Clear();
        _row.Children.Clear();
        Refresh();
    }

    /// <summary>Replaces what is attached with <paramref name="items"/> (a draft saved by <see cref="Snapshot"/>).</summary>
    public void Restore(IEnumerable<PendingItem> items)
    {
        _items.Clear();
        _row.Children.Clear();
        foreach (var i in items)
        {
            _items.Add(i);
            _row.Children.Add(i.Kind == "image" ? Tile(i) : Chip(i));
        }
        Refresh();
    }

    public void SetEnabled(bool enabled)
    {
        AttachButton.IsEnabled = enabled && _items.Count < MaxImages;
        Strip.IsHitTestVisible = enabled;
    }

    /// <summary>
    /// The attachment request parameters <c>{ images, files, links }</c> (only the non-empty ones),
    /// or null when nothing is attached. Merge them into a request with <see cref="Merge"/>.
    /// </summary>
    public JsonElement? ToArgs()
    {
        if (_items.Count == 0) return null;
        var obj = new JsonObject();
        JsonArray Files(string kind)
        {
            var arr = new JsonArray();
            foreach (var i in _items.Where(x => x.Kind == kind))
                arr.Add(new JsonObject { ["name"] = i.Name, ["data_base64"] = Convert.ToBase64String(i.Bytes ?? Array.Empty<byte>()) });
            return arr;
        }
        var images = Files("image");
        var files = Files("file");
        var links = new JsonArray();
        foreach (var l in _items.Where(x => x.Kind == "link")) links.Add(l.Url);
        if (images.Count > 0) obj["images"] = images;
        if (files.Count > 0) obj["files"] = files;
        if (links.Count > 0) obj["links"] = links;
        return JsonDocument.Parse(obj.ToJsonString()).RootElement.Clone();
    }

    /// <summary>Request <paramref name="args"/> plus the attachment parameters from <see cref="ToArgs"/>.</summary>
    public static JsonElement Merge(JsonElement args, JsonElement? attachments)
    {
        if (attachments is not { ValueKind: JsonValueKind.Object } extra) return args;
        var kv = new List<(string, object?)>();
        foreach (var p in args.EnumerateObject()) kv.Add((p.Name, p.Value));
        foreach (var p in extra.EnumerateObject()) kv.Add((p.Name, p.Value));
        return Api.Args(kv.ToArray());
    }

    /// <summary>What is about to be sent, for the message bubble shown while it is processed.</summary>
    public IReadOnlyList<PendingItem> Snapshot() => _items.ToList();

    // ---------- display ----------

    internal static async Task ShowPreviewAsync(XamlRoot? root, BitmapImage? thumb, byte[]? bytes, string name, string? path = null)
    {
        if (root == null) return;
        var full = new BitmapImage();
        try
        {
            if (path != null && File.Exists(path))
            {
                full.UriSource = new Uri(path);
            }
            else if (bytes != null)
            {
                using var ms = new InMemoryRandomAccessStream();
                await ms.WriteAsync(bytes.AsBuffer());
                ms.Seek(0);
                await full.SetSourceAsync(ms);
            }
            else if (thumb != null)
            {
                full = thumb;
            }
        }
        catch
        {
            if (thumb != null) full = thumb;
        }
        // fit the picture to the dialog (no scrolling), and let the dialog grow for wide pictures
        var view = new Image { Source = full, Stretch = Stretch.Uniform, MaxHeight = 640, HorizontalAlignment = HorizontalAlignment.Center };
        var dlg = new ContentDialog
        {
            Title = name,
            Content = view,
            CloseButtonText = L10n.T("common.close"),
            XamlRoot = root,
        };
        dlg.Resources["ContentDialogMaxWidth"] = 1200.0;
        dlg.Resources["ContentDialogMinWidth"] = 640.0;
        await dlg.ShowAsync();
    }

    private static Button PlainButton(UIElement content) => new()
    {
        Content = content,
        Padding = new Thickness(0),
        Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent),
        BorderThickness = new Thickness(0),
        CornerRadius = new CornerRadius(12),
    };

    /// <summary>
    /// Read-only view of the stored attachments of a message (from the core): picture thumbnails
    /// and file/link chips. A chip opens the file (default app) or the link (browser); its context
    /// menu offers to add it as a source through <paramref name="promote"/>.
    /// </summary>
    internal static FrameworkElement Gallery(IEnumerable<JsonElement> attachments, Func<XamlRoot?> xamlRoot, Func<string, Task>? promote = null)
    {
        var panel = new FlowWrapPanel { HorizontalSpacing = 8, VerticalSpacing = 8, HorizontalAlignment = HorizontalAlignment.Left };
        foreach (var a in attachments)
        {
            string S(string k) => a.TryGetProperty(k, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() ?? "" : "";
            var kind = S("kind");
            var path = S("path");
            var name = S("name");
            var id = S("id");
            if (kind is "file" or "link")
            {
                var url = S("url");
                var size = a.TryGetProperty("bytes", out var bv) && bv.ValueKind == JsonValueKind.Number ? bv.GetInt64() : 0;
                var chip = PlainButton(ChipBody(kind, name, kind == "link" ? url : HumanSize(size)));
                chip.Click += async (_, _) =>
                {
                    try
                    {
                        if (kind == "link" && Uri.TryCreate(url, UriKind.Absolute, out var u))
                            await Windows.System.Launcher.LaunchUriAsync(u);
                        else if (File.Exists(path))
                            await Windows.System.Launcher.LaunchFileAsync(await StorageFile.GetFileFromPathAsync(path));
                    }
                    catch { }
                };
                if (promote != null && id.Length > 0)
                {
                    var menu = new MenuFlyout();
                    var item = new MenuFlyoutItem { Text = L10n.T("ctx.promote") };
                    item.Click += async (_, _) => await promote(id);
                    menu.Items.Add(item);
                    chip.ContextFlyout = menu;
                }
                panel.Children.Add(chip);
                continue;
            }
            if (path.Length == 0 || !File.Exists(path)) continue;
            var bmp = new BitmapImage { DecodePixelWidth = 320 };
            try { bmp.UriSource = new Uri(path); } catch { continue; }
            var frame = new Border
            {
                MaxWidth = 160,
                MaxHeight = 160,
                CornerRadius = new CornerRadius(12),
                BorderThickness = new Thickness(1),
                BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
                Child = new Image { Source = bmp, Stretch = Stretch.UniformToFill, Width = 120, Height = 120 },
            };
            var btn = PlainButton(frame);
            ToolTipService.SetToolTip(btn, name);
            var captured = path;
            btn.Click += async (_, _) => await ShowPreviewAsync(xamlRoot(), bmp, null, name, captured);
            panel.Children.Add(btn);
        }
        return panel;
    }

    /// <summary>Gallery for the bubble of a message that is still being sent (nothing stored yet).</summary>
    internal static FrameworkElement PendingGallery(IReadOnlyList<PendingItem> items, Func<XamlRoot?> xamlRoot)
    {
        var panel = new FlowWrapPanel { HorizontalSpacing = 8, VerticalSpacing = 8, HorizontalAlignment = HorizontalAlignment.Left };
        foreach (var it in items)
        {
            if (it.Kind != "image")
            {
                panel.Children.Add(ChipBody(it.Kind, it.Name, it.Kind == "link" ? it.Url ?? "" : HumanSize(it.Bytes?.LongLength ?? 0)));
                continue;
            }
            var frame = new Border
            {
                CornerRadius = new CornerRadius(12),
                BorderThickness = new Thickness(1),
                BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
                Child = new Image { Source = it.Thumb, Stretch = Stretch.UniformToFill, Width = 120, Height = 120 },
            };
            var btn = PlainButton(frame);
            var captured = it;
            btn.Click += async (_, _) => await ShowPreviewAsync(xamlRoot(), captured.Thumb, captured.Bytes, captured.Name);
            panel.Children.Add(btn);
        }
        return panel;
    }
}
