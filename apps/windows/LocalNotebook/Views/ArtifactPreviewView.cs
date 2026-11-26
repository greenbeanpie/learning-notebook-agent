using LocalNotebook.Services;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>
/// Lightweight, read-only rendering of unvalidated in-progress artifact payloads
/// (everything except mind maps, which reuse <see cref="MindmapTreeView"/>).
/// </summary>
internal static class ArtifactPreviewView
{
    private const int MaxItemsShown = 400;
    private const int MaxBodyChars = 700;

    public static FrameworkElement Build(string stage, string artifactType, JsonElement payload, HashSet<string> expanded)
    {
        var root = new StackPanel { Spacing = 8, Margin = new Thickness(0, 0, 8, 8) };
        try
        {
            if (stage == "lecture_passes" || artifactType == "lecture" || payload.TryGetProperty("outline", out _))
                BuildLecture(root, payload, expanded);
            else
            {
                var kind = artifactType;
                if (!(kind is "quiz" or "slides" or "report" or "flashcards" or "infographic" or "audio"))
                    kind = Guess(payload);
                switch (kind)
                {
                    case "quiz": BuildQuiz(root, payload); break;
                    case "slides": BuildSlides(root, payload); break;
                    case "report": BuildReport(root, payload); break;
                    case "flashcards": BuildFlashcards(root, payload); break;
                    case "infographic": BuildInfographic(root, payload); break;
                    case "audio": BuildAudio(root, payload); break;
                    default: BuildFallback(root, payload); break;
                }
            }
        }
        catch
        {
            root.Children.Clear();
            BuildFallback(root, payload);
        }
        if (root.Children.Count == 0) BuildFallback(root, payload);
        return root;
    }

    private static string Guess(JsonElement p)
    {
        if (p.ValueKind != JsonValueKind.Object) return "";
        if (p.TryGetProperty("questions", out _)) return "quiz";
        if (p.TryGetProperty("slides", out _)) return "slides";
        if (p.TryGetProperty("cards", out _)) return "flashcards";
        if (p.TryGetProperty("blocks", out _)) return "infographic";
        if (p.TryGetProperty("turns", out _)) return "audio";
        if (p.TryGetProperty("sections", out _)) return "report";
        return "";
    }

    // ---------- helpers ----------

    private static TextBlock Text(string text, double size = 12.5, bool bold = false, bool muted = false, double left = 0) => new()
    {
        Text = text,
        FontSize = size,
        FontWeight = bold ? FontWeights.SemiBold : FontWeights.Normal,
        TextWrapping = TextWrapping.Wrap,
        IsTextSelectionEnabled = true,
        Margin = new Thickness(left, 0, 0, 0),
        Foreground = (Brush)Application.Current.Resources[muted ? "TextMutedBrush" : "TextSecondaryBrush"],
    };

    private static Border Card(UIElement child) => new()
    {
        Background = (Brush)Application.Current.Resources["ElevatedBgBrush"],
        BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
        BorderThickness = new Thickness(1),
        CornerRadius = new CornerRadius(10),
        Padding = new Thickness(12, 8, 12, 8),
        Child = child,
    };

    private static string ItemText(JsonElement e)
    {
        switch (e.ValueKind)
        {
            case JsonValueKind.String: return e.GetString() ?? "";
            case JsonValueKind.Number: return e.ToString();
            case JsonValueKind.Object:
                foreach (var k in new[] { "text", "label", "title", "heading", "value", "content" })
                    if (Str(e, k) is { Length: > 0 } v) return v;
                return "";
            default: return "";
        }
    }

    private static string Clip(string s, int max) => s.Length <= max ? s : s[..max] + "…";

    private static void Title(StackPanel root, JsonElement p)
    {
        var t = Str(p, "title");
        if (t.Length > 0) root.Children.Add(Text(t, 15, bold: true));
        var st = Str(p, "subtitle");
        if (st.Length > 0) root.Children.Add(Text(st, 12, muted: true));
    }

    private static IEnumerable<JsonElement> Items(JsonElement p, string name) => Arr(p, name).Take(MaxItemsShown);

    // ---------- per type ----------

    private static void BuildQuiz(StackPanel root, JsonElement p)
    {
        Title(root, p);
        var n = 0;
        foreach (var q in Items(p, "questions"))
        {
            n++;
            var col = new StackPanel { Spacing = 4 };
            col.Children.Add(Text($"{n}. {Str(q, "prompt")}", 13, bold: true));
            var i = 0;
            foreach (var o in Arr(q, "options"))
            {
                col.Children.Add(Text($"{(char)('A' + Math.Min(i, 25))}. {ItemText(o)}", 12.5, left: 14));
                i++;
            }
            root.Children.Add(Card(col));
        }
    }

    private static void BuildSlides(StackPanel root, JsonElement p)
    {
        Title(root, p);
        var n = 0;
        foreach (var s in Items(p, "slides"))
        {
            n++;
            var col = new StackPanel { Spacing = 3 };
            var t = Str(s, "title");
            col.Children.Add(Text($"{n}. {(t.Length > 0 ? t : Str(s, "layout"))}", 13, bold: true));
            var st = Str(s, "subtitle");
            if (st.Length > 0) col.Children.Add(Text(st, 12, muted: true));
            foreach (var b in Arr(s, "bullets"))
            {
                var bt = ItemText(b);
                if (bt.Length > 0) col.Children.Add(Text("• " + bt, 12.5, left: 8));
            }
            var callout = Str(s, "callout");
            if (callout.Length > 0) col.Children.Add(Text(callout, 12, muted: true, left: 8));
            root.Children.Add(Card(col));
        }
    }

    private static void BuildReport(StackPanel root, JsonElement p)
    {
        Title(root, p);
        foreach (var s in Items(p, "sections"))
        {
            var col = new StackPanel { Spacing = 3 };
            col.Children.Add(Text(Str(s, "heading"), 13, bold: true));
            var body = Str(s, "body_md");
            if (body.Length > 0) col.Children.Add(Text(Clip(body, MaxBodyChars), 12.5));
            foreach (var k in Arr(s, "key_takeaways"))
            {
                var kt = ItemText(k);
                if (kt.Length > 0) col.Children.Add(Text("• " + kt, 12.5, left: 8));
            }
            root.Children.Add(Card(col));
        }
    }

    private static void BuildFlashcards(StackPanel root, JsonElement p)
    {
        Title(root, p);
        var n = 0;
        foreach (var c in Items(p, "cards"))
        {
            n++;
            var col = new StackPanel { Spacing = 3 };
            col.Children.Add(Text($"{n}. {Str(c, "front")}", 13, bold: true));
            var back = Str(c, "back");
            if (back.Length > 0) col.Children.Add(Text(back, 12.5, left: 14));
            root.Children.Add(Card(col));
        }
    }

    private static void BuildInfographic(StackPanel root, JsonElement p)
    {
        Title(root, p);
        foreach (var b in Items(p, "blocks"))
        {
            var col = new StackPanel { Spacing = 3 };
            var head = Str(b, "heading");
            var metric = Str(b, "metric");
            col.Children.Add(Text(metric.Length > 0 ? $"{head}  ·  {metric}" : head, 13, bold: true));
            var sum = Str(b, "summary");
            if (sum.Length > 0) col.Children.Add(Text(sum, 12.5));
            root.Children.Add(Card(col));
        }
    }

    private static void BuildAudio(StackPanel root, JsonElement p)
    {
        Title(root, p);
        foreach (var t in Items(p, "turns"))
        {
            var sp = Str(t, "speaker");
            root.Children.Add(Text(sp.Length > 0 ? $"{sp}: {Str(t, "text")}" : Str(t, "text"), 12.5));
        }
    }

    private static void BuildLecture(StackPanel root, JsonElement p, HashSet<string> expanded)
    {
        var outline = Obj(p, "outline");
        var done = Obj(p, "done");
        var t = Str(outline, "title");
        if (t.Length > 0) root.Children.Add(Text(t, 15, bold: true));
        var i = 0;
        foreach (var sec in Arr(outline, "sections"))
        {
            i++;
            var key = $"s{i}";
            var title = Str(sec, "title");
            var content = Obj(done, key);
            var isDone = content.ValueKind == JsonValueKind.Object;
            if (!isDone)
            {
                root.Children.Add(Text($"{i}. {title}", 13, muted: true));
                continue;
            }

            var body = new StackPanel { Spacing = 6, Margin = new Thickness(0, 4, 0, 4) };
            var sn = 0;
            foreach (var slide in Arr(content, "slides").Take(MaxItemsShown))
            {
                sn++;
                body.Children.Add(Text($"{sn}. {Str(slide, "title")}", 12.5, bold: true));
                var narration = string.Join(" ", Arr(slide, "narration").Select(n => Str(n, "text")).Where(x => x.Length > 0));
                if (narration.Length > 0) body.Children.Add(Text(Clip(narration, MaxBodyChars), 12, muted: true, left: 10));
            }
            if (sn == 0) body.Children.Add(Text(Clip(content.ToString(), 1200), 11.5, muted: true));

            var expander = new Expander
            {
                Header = Text($"✓ {i}. {title}", 13, bold: true),
                Content = body,
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Stretch,
                IsExpanded = expanded.Contains(key),
            };
            expander.Expanding += (_, _) => expanded.Add(key);
            expander.Collapsed += (_, _) => expanded.Remove(key);
            root.Children.Add(expander);
        }
    }

    private static void BuildFallback(StackPanel root, JsonElement p)
    {
        string json;
        try
        {
            using var ms = new MemoryStream();
            using (var w = new Utf8JsonWriter(ms, new JsonWriterOptions { Indented = true })) p.WriteTo(w);
            json = System.Text.Encoding.UTF8.GetString(ms.ToArray());
        }
        catch { json = p.ToString(); }
        root.Children.Add(new TextBlock
        {
            Text = Clip(json, 4000),
            FontFamily = new FontFamily("Consolas"),
            FontSize = 11.5,
            TextWrapping = TextWrapping.Wrap,
            IsTextSelectionEnabled = true,
            Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
        });
    }
}
