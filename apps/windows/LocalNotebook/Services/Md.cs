using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Text;
using Windows.Storage.Streams;

namespace LocalNotebook.Services;

/// <summary>
/// Small Markdown renderer for chat answers and reports: headings, lists, quotes, code blocks,
/// bold / inline code, clickable <c>[n]</c> citation chips, and inline/block LaTeX formulas rendered
/// via the shared <see cref="WebContentHost"/> with error/limit/overflow fallbacks.
/// </summary>
public static class Md
{
    public static bool HasMath(string text) => text.Contains('$') || text.Contains("\\(") || text.Contains("\\[");

    public static UIElement Render(
        string markdown,
        Func<int, Inline?>? citationChip = null,
        double fontSize = 14,
        WebContentHost? math = null,
        Panel? mathHost = null)
    {
        var panel = new StackPanel { Spacing = 6 };
        var lines = markdown.Replace("\r\n", "\n").Split('\n');
        var para = new StringBuilder();
        var i = 0;
        var formulaCount = 0;

        void FlushPara()
        {
            if (para.Length == 0) return;
            var text = para.ToString().TrimEnd();
            para.Clear();
            AddParagraphWithDisplayMath(panel, text, citationChip, fontSize, math, mathHost, ref formulaCount);
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
            // Multi-line display math block: $$ ... $$ or \[ ... \]
            if ((trimmed.StartsWith("$$") && !trimmed[2..].Contains("$$")) ||
                (trimmed.StartsWith("\\[") && !trimmed[2..].Contains("\\]")))
            {
                FlushPara();
                var closeDelim = trimmed.StartsWith("$$") ? "$$" : "\\]";
                var mathBuf = new StringBuilder();
                var firstRest = trimmed[2..];
                if (!string.IsNullOrWhiteSpace(firstRest)) mathBuf.AppendLine(firstRest);
                i++;
                var closed = false;
                while (i < lines.Length)
                {
                    var l = lines[i];
                    var closeIdx = l.IndexOf(closeDelim, StringComparison.Ordinal);
                    if (closeIdx >= 0)
                    {
                        mathBuf.Append(l[..closeIdx]);
                        closed = true;
                        i++;
                        break;
                    }
                    mathBuf.AppendLine(l);
                    i++;
                }
                var expr = mathBuf.ToString().Trim();
                if (closed && expr.Length > 0)
                {
                    panel.Children.Add(CreateMathBlock(expr, displayMode: true, fontSize, math, mathHost, ref formulaCount));
                }
                else
                {
                    panel.Children.Add(Inline(trimmed + "\n" + expr, citationChip, fontSize, math, mathHost, ref formulaCount));
                }
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
                var body = Inline(trimmed.StartsWith("- ") || trimmed.StartsWith("* ") ? trimmed[2..] : rest, citationChip, fontSize, math, mathHost, ref formulaCount);
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
                    Child = Inline(trimmed[2..], citationChip, fontSize, math, mathHost, ref formulaCount),
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

    private static void AddParagraphWithDisplayMath(
        StackPanel panel,
        string text,
        Func<int, Inline?>? citationChip,
        double fontSize,
        WebContentHost? math,
        Panel? mathHost,
        ref int formulaCount)
    {
        // Split inline text around single-line display math delimiters $$...$$ and \[...\]
        var pos = 0;
        while (pos < text.Length)
        {
            var idxDollar = text.IndexOf("$$", pos, StringComparison.Ordinal);
            var idxBracket = text.IndexOf("\\[", pos, StringComparison.Ordinal);
            var nextIdx = (idxDollar, idxBracket) switch
            {
                (-1, -1) => -1,
                (>= 0, -1) => idxDollar,
                (-1, >= 0) => idxBracket,
                _ => Math.Min(idxDollar, idxBracket),
            };
            if (nextIdx < 0) break;

            var isDollar = nextIdx == idxDollar;
            var closeDelim = isDollar ? "$$" : "\\]";
            var closeIdx = text.IndexOf(closeDelim, nextIdx + 2, StringComparison.Ordinal);
            if (closeIdx < 0) break;

            if (nextIdx > pos)
            {
                var before = text[pos..nextIdx].Trim();
                if (before.Length > 0)
                {
                    panel.Children.Add(Inline(before, citationChip, fontSize, math, mathHost, ref formulaCount));
                }
            }
            var expr = text[(nextIdx + 2)..closeIdx].Trim();
            if (expr.Length > 0)
            {
                panel.Children.Add(CreateMathBlock(expr, displayMode: true, fontSize, math, mathHost, ref formulaCount));
            }
            pos = closeIdx + 2;
        }

        if (pos < text.Length)
        {
            var tail = text[pos..].Trim();
            if (tail.Length > 0)
            {
                panel.Children.Add(Inline(tail, citationChip, fontSize, math, mathHost, ref formulaCount));
            }
        }
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

    public static FrameworkElement Inline(string text, Func<int, Inline?>? citationChip, double fontSize = 14)
    {
        var count = 0;
        return Inline(text, citationChip, fontSize, null, null, ref count);
    }

    /// <summary>One paragraph of inline Markdown as a RichTextBlock supporting inline LaTeX & citation chips.</summary>
    private static FrameworkElement Inline(
        string text,
        Func<int, Inline?>? citationChip,
        double fontSize,
        WebContentHost? math,
        Panel? mathHost,
        ref int formulaCount)
    {
        var rtb = new RichTextBlock
        {
            TextWrapping = TextWrapping.Wrap,
            IsTextSelectionEnabled = true,
            FontSize = fontSize,
        };
        var p = new Paragraph();
        rtb.Blocks.Add(p);

        var buf = new StringBuilder();
        void Flush()
        {
            if (buf.Length > 0)
            {
                p.Inlines.Add(new Run { Text = buf.ToString() });
                buf.Clear();
            }
        }

        var i = 0;
        while (i < text.Length)
        {
            var c = text[i];
            if (c == '\\' && i + 1 < text.Length && text[i + 1] == '$')
            {
                buf.Append('$');
                i += 2;
                continue;
            }
            if (c == '`')
            {
                var end = text.IndexOf('`', i + 1);
                if (end > i)
                {
                    Flush();
                    p.Inlines.Add(new Run
                    {
                        Text = text[(i + 1)..end],
                        FontFamily = new FontFamily("Consolas"),
                        Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 160, 200, 255)),
                    });
                    i = end + 1;
                    continue;
                }
            }
            // Inline math \( ... \)
            else if (c == '\\' && i + 1 < text.Length && text[i + 1] == '(')
            {
                var end = text.IndexOf("\\)", i + 2, StringComparison.Ordinal);
                if (end > i + 2)
                {
                    var expr = text[(i + 2)..end].Trim();
                    if (expr.Length > 0)
                    {
                        Flush();
                        p.Inlines.Add(new InlineUIContainer
                        {
                            Child = CreateMathBlock(expr, displayMode: false, fontSize, math, mathHost, ref formulaCount),
                        });
                        i = end + 2;
                        continue;
                    }
                }
            }
            // Inline math $ ... $ (avoid $$ and currency like $100)
            else if (c == '$' && (i + 1 >= text.Length || text[i + 1] != '$') && TryFindInlineDollarEnd(text, i + 1, out var end))
            {
                var expr = text[(i + 1)..end].Trim();
                if (expr.Length > 0)
                {
                    Flush();
                    p.Inlines.Add(new InlineUIContainer
                    {
                        Child = CreateMathBlock(expr, displayMode: false, fontSize, math, mathHost, ref formulaCount),
                    });
                    i = end + 1;
                    continue;
                }
            }
            else if (c == '*' && i + 1 < text.Length && text[i + 1] == '*')
            {
                var boldEnd = text.IndexOf("**", i + 2, StringComparison.Ordinal);
                if (boldEnd > i + 1)
                {
                    Flush();
                    p.Inlines.Add(new Run { Text = text[(i + 2)..boldEnd], FontWeight = FontWeights.Bold });
                    i = boldEnd + 2;
                    continue;
                }
            }
            else if (c == '[' && citationChip != null)
            {
                var citeEnd = text.IndexOf(']', i + 1);
                if (citeEnd > i + 1 && citeEnd - i <= 4 && int.TryParse(text.AsSpan(i + 1, citeEnd - i - 1), out var n) && n > 0 && citationChip(n) is { } chip)
                {
                    Flush();
                    p.Inlines.Add(chip);
                    i = citeEnd + 1;
                    continue;
                }
            }
            buf.Append(c);
            i++;
        }
        Flush();
        return rtb;
    }

    private static bool TryFindInlineDollarEnd(string text, int start, out int end)
    {
        end = -1;
        if (start >= text.Length || char.IsWhiteSpace(text[start])) return false;
        for (var j = start; j < text.Length; j++)
        {
            if (text[j] == '\n') return false;
            if (text[j] == '\\' && j + 1 < text.Length) { j++; continue; }
            if (text[j] == '$')
            {
                if (j + 1 < text.Length && text[j + 1] == '$') return false;
                if (j > start && !char.IsWhiteSpace(text[j - 1]))
                {
                    end = j;
                    return true;
                }
                return false;
            }
        }
        return false;
    }

    private static UIElement CreateMathBlock(
        string latex,
        bool displayMode,
        double fontSize,
        WebContentHost? math,
        Panel? mathHost,
        ref int formulaCount)
    {
        formulaCount++;
        var fallback = new TextBlock
        {
            Text = displayMode ? $"$${latex}$$" : $"${latex}$",
            FontFamily = new FontFamily("Consolas"),
            FontSize = Math.Max(12, fontSize - 1),
            TextWrapping = displayMode ? TextWrapping.Wrap : TextWrapping.NoWrap,
            IsTextSelectionEnabled = true,
            Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 210, 220, 235)),
        };

        var container = new Border
        {
            CornerRadius = new CornerRadius(6),
            Padding = displayMode ? new Thickness(8, 6, 8, 6) : new Thickness(3, 0, 3, 0),
            Margin = displayMode ? new Thickness(0, 4, 0, 4) : new Thickness(1, 0, 1, 0),
            Background = displayMode
                ? new SolidColorBrush(Windows.UI.Color.FromArgb(60, 19, 19, 20))
                : new SolidColorBrush(Windows.UI.Color.FromArgb(0, 0, 0, 0)),
            VerticalAlignment = VerticalAlignment.Center,
            Child = displayMode
                ? new ScrollViewer
                {
                    HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
                    VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
                    Content = fallback,
                }
                : (UIElement)fallback,
        };

        if (formulaCount > WebContentHost.MaxFormulasPerMessage)
        {
            ApplyErrorStyle(container, fallback, $"单条消息公式数量超过 {WebContentHost.MaxFormulasPerMessage} 个上限，已回退显示原文");
            return container;
        }
        if (latex.Length > WebContentHost.MaxFormulaLength)
        {
            ApplyErrorStyle(container, fallback, $"公式长度超过 {WebContentHost.MaxFormulaLength} 字符上限，已回退显示原文");
            return container;
        }
        if (math == null || mathHost == null)
        {
            return container;
        }

        _ = RenderMathIntoAsync(container, fallback, latex, displayMode, math, mathHost);
        return container;
    }

    private static async Task RenderMathIntoAsync(
        Border container,
        TextBlock fallback,
        string latex,
        bool displayMode,
        WebContentHost math,
        Panel mathHost)
    {
        var res = await math.RenderFormulaAsync(mathHost, latex, displayMode);
        if (!res.Success || res.PngBytes == null)
        {
            ApplyErrorStyle(container, fallback, $"公式渲染失败：{res.Error ?? "未知错误"}");
            return;
        }

        try
        {
            using var mem = new InMemoryRandomAccessStream();
            await mem.WriteAsync(res.PngBytes.AsBuffer());
            mem.Seek(0);
            var bmp = new BitmapImage();
            await bmp.SetSourceAsync(mem);

            var img = new Image
            {
                Source = bmp,
                Width = res.Width,
                Height = res.Height,
                Stretch = Stretch.Uniform,
                VerticalAlignment = VerticalAlignment.Center,
                HorizontalAlignment = displayMode ? HorizontalAlignment.Center : HorizontalAlignment.Left,
            };
            ToolTipService.SetToolTip(
                img,
                res.Overflow ? $"{latex}（公式过宽已裁剪至 1600px，可横向滚动）" : latex);

            if (displayMode)
            {
                container.Child = new ScrollViewer
                {
                    HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
                    VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
                    Content = img,
                };
            }
            else
            {
                container.Child = img;
            }
        }
        catch (Exception ex)
        {
            ApplyErrorStyle(container, fallback, $"公式图像解码失败：{ex.Message}");
        }
    }

    private static void ApplyErrorStyle(Border container, TextBlock fallback, string reason)
    {
        container.BorderBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(160, 240, 120, 120));
        container.BorderThickness = new Thickness(1);
        container.Background = new SolidColorBrush(Windows.UI.Color.FromArgb(40, 180, 60, 60));
        fallback.Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 190, 190));
        ToolTipService.SetToolTip(container, reason);
    }
}
