using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Media;
using System.Text;

namespace LocalNotebook.Services;

/// <summary>
/// Small Markdown renderer for chat answers and reports: headings, lists, quotes, code blocks,
/// bold / italic / inline code, and clickable <c>[n]</c> citation chips. Formulas stay as text here;
/// the "formula view" typesets them in the shared WebView2.
/// </summary>
public static class Md
{
    public static bool HasMath(string text) => text.Contains('$') || text.Contains("\\(") || text.Contains("\\[");

    public static UIElement Render(string markdown, Func<int, Inline?>? citationChip = null, double fontSize = 14)
    {
        var panel = new StackPanel { Spacing = 6 };
        var lines = markdown.Replace("\r\n", "\n").Split('\n');
        var para = new StringBuilder();
        var i = 0;

        void FlushPara()
        {
            if (para.Length == 0) return;
            panel.Children.Add(Inline(para.ToString().TrimEnd(), citationChip, fontSize));
            para.Clear();
        }

        while (i < lines.Length)
        {
            var line = lines[i];
            var trimmed = line.TrimStart();
            if (trimmed.StartsWith("```"))
            {
                FlushPara();
                var code = new StringBuilder();
                i++;
                while (i < lines.Length && !lines[i].TrimStart().StartsWith("```"))
                {
                    code.AppendLine(lines[i]);
                    i++;
                }
                i++; // closing fence
                panel.Children.Add(new Border
                {
                    Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 19, 19, 20)),
                    CornerRadius = new CornerRadius(8),
                    Padding = new Thickness(10),
                    Child = new TextBlock
                    {
                        Text = code.ToString().TrimEnd(),
                        FontFamily = new FontFamily("Consolas"),
                        FontSize = fontSize - 1,
                        IsTextSelectionEnabled = true,
                        TextWrapping = TextWrapping.Wrap,
                    },
                });
                continue;
            }
            if (trimmed.Length == 0)
            {
                FlushPara();
                i++;
                continue;
            }
            var level = HeadingLevel(trimmed);
            if (level > 0)
            {
                FlushPara();
                panel.Children.Add(new TextBlock
                {
                    Text = trimmed.TrimStart('#').Trim(),
                    FontSize = level == 1 ? fontSize + 6 : level == 2 ? fontSize + 3 : fontSize + 1,
                    FontWeight = FontWeights.SemiBold,
                    TextWrapping = TextWrapping.Wrap,
                    IsTextSelectionEnabled = true,
                    Margin = new Thickness(0, 6, 0, 0),
                });
                i++;
                continue;
            }
            if (trimmed.StartsWith("- ") || trimmed.StartsWith("* ") || IsOrdered(trimmed, out _))
            {
                FlushPara();
                var marker = IsOrdered(trimmed, out var rest) ? trimmed[..(trimmed.Length - rest.Length)].Trim() : "•";
                var item = new Grid { ColumnSpacing = 8 };
                item.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                item.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                item.Children.Add(new TextBlock { Text = marker, FontSize = fontSize });
                var body = Inline(trimmed.StartsWith("- ") || trimmed.StartsWith("* ") ? trimmed[2..] : rest, citationChip, fontSize);
                Grid.SetColumn(body, 1);
                item.Children.Add(body);
                item.Margin = new Thickness(Math.Min(line.Length - trimmed.Length, 12) * 2, 0, 0, 0);
                panel.Children.Add(item);
                i++;
                continue;
            }
            if (trimmed.StartsWith("> "))
            {
                FlushPara();
                panel.Children.Add(new Border
                {
                    BorderBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 100, 100, 110)),
                    BorderThickness = new Thickness(3, 0, 0, 0),
                    Padding = new Thickness(10, 2, 0, 2),
                    Child = Inline(trimmed[2..], citationChip, fontSize),
                });
                i++;
                continue;
            }
            if (para.Length > 0) para.Append('\n');
            para.Append(line);
            i++;
        }
        FlushPara();
        return panel;
    }

    private static int HeadingLevel(string t)
    {
        var n = 0;
        while (n < t.Length && t[n] == '#') n++;
        return n is >= 1 and <= 6 && n < t.Length && t[n] == ' ' ? n : 0;
    }

    private static bool IsOrdered(string t, out string rest)
    {
        var n = 0;
        while (n < t.Length && char.IsDigit(t[n])) n++;
        if (n > 0 && n + 1 < t.Length && (t[n] == '.' || t[n] == ')') && t[n + 1] == ' ')
        {
            rest = t[(n + 2)..];
            return true;
        }
        rest = "";
        return false;
    }

    /// <summary>One paragraph of inline Markdown as a wrapping TextBlock.</summary>
    public static TextBlock Inline(string text, Func<int, Inline?>? citationChip, double fontSize = 14)
    {
        var tb = new TextBlock { TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true, FontSize = fontSize };
        var buf = new StringBuilder();
        void Flush()
        {
            if (buf.Length > 0) { tb.Inlines.Add(new Run { Text = buf.ToString() }); buf.Clear(); }
        }
        var i = 0;
        while (i < text.Length)
        {
            var c = text[i];
            if (c == '`')
            {
                var end = text.IndexOf('`', i + 1);
                if (end > i)
                {
                    Flush();
                    tb.Inlines.Add(new Run { Text = text[(i + 1)..end], FontFamily = new FontFamily("Consolas"), Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 160, 200, 255)) });
                    i = end + 1;
                    continue;
                }
            }
            else if (c == '*' && i + 1 < text.Length && text[i + 1] == '*')
            {
                var end = text.IndexOf("**", i + 2, StringComparison.Ordinal);
                if (end > i + 1)
                {
                    Flush();
                    tb.Inlines.Add(new Run { Text = text[(i + 2)..end], FontWeight = FontWeights.Bold });
                    i = end + 2;
                    continue;
                }
            }
            else if (c == '[' && citationChip != null)
            {
                var end = text.IndexOf(']', i + 1);
                if (end > i + 1 && end - i <= 4 && int.TryParse(text.AsSpan(i + 1, end - i - 1), out var n) && n > 0 && citationChip(n) is { } chip)
                {
                    Flush();
                    tb.Inlines.Add(chip);
                    i = end + 1;
                    continue;
                }
            }
            buf.Append(c);
            i++;
        }
        Flush();
        return tb;
    }
}
