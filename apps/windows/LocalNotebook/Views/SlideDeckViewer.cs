using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>
/// Interactive 16:9 animated PPT presentation player built on WebView2.
/// Renders all 5 curated editorial themes and 12 rich slide layouts with
/// directional slide transitions, staggered element entrance animations,
/// auto-play slideshow mode, keyboard navigation, thumbnail strip, and speaker notes.
/// </summary>
internal sealed class SlideDeckViewer
{
    public FrameworkElement Root { get; }

    public SlideDeckViewer(JsonElement payload, bool compact, Action<StackPanel>? appendFeedback = null, string? outputDir = null)
    {
        var slides = Arr(payload, "slides").ToList();
        if (slides.Count == 0)
        {
            Root = new TextBlock { Text = L10n.T("slides.empty"), Margin = new Thickness(0, 12, 0, 12) };
            return;
        }

        var webView = new WebView2
        {
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            MinHeight = compact ? 380 : 420,
            DefaultBackgroundColor = Windows.UI.Color.FromArgb(255, 19, 20, 24),
        };

        var html = BuildPresentationHtml(PayloadWithFigures(payload, outputDir), L10n.IsEnglish, lectureMode: false);
        webView.Loaded += async (_, _) =>
        {
            try
            {
                await webView.EnsureCoreWebView2Async();
                webView.CoreWebView2.Settings.AreDefaultContextMenusEnabled = false;
                webView.CoreWebView2.Settings.AreDevToolsEnabled = false;
                webView.NavigateToString(html);
            }
            catch
            {
                // Keep graceful empty surface if WebView2 runtime is unavailable
            }
        };

        var stageFrame = new Border
        {
            CornerRadius = new CornerRadius(14),
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 19, 20, 24)),
            Child = webView,
        };

        if (compact)
        {
            var box = new StackPanel { Spacing = 10 };
            stageFrame.Height = 420;
            box.Children.Add(stageFrame);
            appendFeedback?.Invoke(box);
            Root = box;
        }
        else
        {
            var grid = new Grid { RowSpacing = 10 };
            grid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
            grid.Children.Add(stageFrame);
            if (appendFeedback != null)
            {
                var fbHost = new StackPanel { Spacing = 6 };
                appendFeedback(fbHost);
                if (fbHost.Children.Count > 0)
                {
                    grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
                    Grid.SetRow(fbHost, 1);
                    grid.Children.Add(fbHost);
                }
            }
            Root = grid;
        }
    }

    /// <summary>
    /// Payload JSON with each slide's figure SVG (written by the core to
    /// <c>figures/&lt;slide id&gt;.svg</c>) inlined as <c>figure_svg</c>.
    /// </summary>
    internal static string PayloadWithFigures(JsonElement payload, string? outputDir)
    {
        if (string.IsNullOrEmpty(outputDir)) return payload.GetRawText();
        var node = System.Text.Json.Nodes.JsonNode.Parse(payload.GetRawText());
        if (node?["slides"] is not System.Text.Json.Nodes.JsonArray slides) return payload.GetRawText();
        for (var i = 0; i < slides.Count; i++)
        {
            if (slides[i] is not System.Text.Json.Nodes.JsonObject s || s["figure"] is not System.Text.Json.Nodes.JsonObject) continue;
            var path = Path.Combine(outputDir, "figures", SlideFileId(s, i) + ".svg");
            try
            {
                if (File.Exists(path)) s["figure_svg"] = File.ReadAllText(path);
            }
            catch
            {
                // a missing figure only hides the picture
            }
        }
        return node.ToJsonString();
    }

    /// <summary>Same naming as the core's <c>lecture::slide_file_id</c>.</summary>
    internal static string SlideFileId(System.Text.Json.Nodes.JsonObject slide, int index)
    {
        var id = slide["id"] is System.Text.Json.Nodes.JsonValue v && v.TryGetValue<string>(out var sv) ? sv.Trim() : "";
        id = new string(id.Where(c => char.IsAsciiLetterOrDigit(c) || c is '-' or '_' or '.').ToArray());
        return id.Length == 0 ? $"slide{index + 1}" : id;
    }

    private static string BuildPresentationHtml(JsonElement payload, bool en) =>
        BuildPresentationHtml(payload.GetRawText(), en, lectureMode: false);

    /// The shared deck/lecture player page (apps/shared/deck_player.html, embedded).
    private static readonly Lazy<string> DeckTemplate = new(() =>
    {
        using var st = typeof(SlideDeckViewer).Assembly.GetManifestResourceStream("deck_player.html")
            ?? throw new InvalidOperationException("deck_player.html resource missing");
        using var rd = new StreamReader(st);
        return rd.ReadToEnd();
    });

    internal static string BuildPresentationHtml(string payloadJson, bool en, bool lectureMode)
    {
        var rawJson = payloadJson
            .Replace("</", "<\\/")
            .Replace("\u2028", "\\u2028")
            .Replace("\u2029", "\\u2029");
        var tSteps = L10n.T("deck.steps", en);

        var tPrev = L10n.T("slides.prev", en);
        var tNext = L10n.T("slides.next", en);
        var tPlay = L10n.T("slides.play", en);
        var tPause = L10n.T("slides.pause", en);
        var tNotes = L10n.T("deck.speaker_notes", en);
        var tNoNotes = L10n.T("slides.no_speaker_notes_for_this", en);
        var tAnimSlide = L10n.T("deck.slide", en);
        var tAnimFade = L10n.T("deck.fade", en);
        var tAnimZoom = L10n.T("deck.zoom", en);
        var tFullscreen = L10n.T("deck.present", en);

        var tpl = DeckTemplate.Value;
        string J(string v) => "\"" + JsonEncodedText.Encode(v).ToString() + "\"";
        return tpl
            .Replace("__LANG__", en ? "en" : "zh-CN")
            .Replace("__LECTURE__", lectureMode ? "true" : "false")
            .Replace("__T_PLAY_JSON__", J(tPlay))
            .Replace("__T_PAUSE_JSON__", J(tPause))
            .Replace("__T_NONOTES_JSON__", J(tNoNotes))
            .Replace("__T_PREV__", tPrev)
            .Replace("__T_NEXT__", tNext)
            .Replace("__T_PLAY__", tPlay)
            .Replace("__T_ANIMSLIDE__", tAnimSlide)
            .Replace("__T_ANIMFADE__", tAnimFade)
            .Replace("__T_ANIMZOOM__", tAnimZoom)
            .Replace("__T_STEPS__", tSteps)
            .Replace("__T_NOTES__", tNotes)
            .Replace("__T_FULLSCREEN__", tFullscreen)
            // payload last: its text must not be scanned for tokens
            .Replace("__PAYLOAD__", rawJson);
    }
}
