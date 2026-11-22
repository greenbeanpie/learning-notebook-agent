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

    public static readonly FontFamily BodyFontFamily = new("Microsoft YaHei UI, Segoe UI");

    public static Inline SuperscriptCitation(int no, string? tooltip = null, Action? onClick = null)
    {
        var pill = new Border
        {
            Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 42, 46, 54)),
            BorderBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 68, 74, 86)),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(10),
            MinWidth = 20,
            Height = 20,
            Padding = new Thickness(6, 0, 6, 0),
            Margin = new Thickness(3, 0, 3, -3),
            Child = new TextBlock
            {
                Text = $"{no}",
                FontFamily = BodyFontFamily,
                FontSize = 11,
                FontWeight = FontWeights.Medium,
                Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 198, 206, 220)),
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
            },
        };
        if (!string.IsNullOrEmpty(tooltip))
        {
            ToolTipService.SetToolTip(pill, tooltip);
        }
        if (onClick != null)
        {
            pill.Tapped += (_, e) =>
            {
                e.Handled = true;
                onClick();
            };
        }
        return new InlineUIContainer { Child = pill };
    }

    public static UIElement Render(
        string markdown,
        Func<int, Inline?>? citationChip = null,
        double fontSize = 15,
        WebContentHost? math = null,
        Panel? mathHost = null)
    {
        var effectiveSize = fontSize <= 0 ? L10n.Prefs.FontSize : fontSize;
        var rtb = new RichTextBlock
        {
            FontFamily = BodyFontFamily,
            TextWrapping = TextWrapping.Wrap,
            IsTextSelectionEnabled = true,
            FontSize = effectiveSize,
            LineHeight = Math.Round(effectiveSize * 1.72),
            LineStackingStrategy = LineStackingStrategy.BlockLineHeight,
            CharacterSpacing = 12,
            Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 226, 228, 232)),
        };
        fontSize = effectiveSize;
        var lines = markdown.Replace("\r\n", "\n").Split('\n');
        var para = new StringBuilder();
        var i = 0;
        var formulaCount = 0;

        void FlushPara()
        {
            if (para.Length == 0) return;
            var text = para.ToString().TrimEnd();
            para.Clear();
            AddParagraphWithDisplayMath(rtb, text, citationChip, fontSize, math, mathHost, ref formulaCount);
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
                var codePara = new Paragraph
                {
                    FontFamily = new FontFamily("Consolas"),
                    FontSize = fontSize - 1,
                    Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 210, 224, 245)),
                    Margin = new Thickness(12, 4, 0, 8),
                };
                codePara.Inlines.Add(new Run { Text = code.ToString().TrimEnd() });
                rtb.Blocks.Add(codePara);
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
                    var mathPara = new Paragraph { TextAlignment = TextAlignment.Center, Margin = new Thickness(0, 6, 0, 8) };
                    mathPara.Inlines.Add(new InlineUIContainer { Child = CreateMathBlock(expr, displayMode: true, fontSize, math, mathHost, ref formulaCount) });
                    rtb.Blocks.Add(mathPara);
                }
                else
                {
                    var p = new Paragraph { Margin = new Thickness(0, 0, 0, 8) };
                    AppendInlines(p, trimmed + "\n" + expr, citationChip, fontSize, math, mathHost, ref formulaCount);
                    rtb.Blocks.Add(p);
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
                var hp = new Paragraph
                {
                    FontSize = level == 1 ? fontSize + 6 : level == 2 ? fontSize + 3 : fontSize + 1,
                    FontWeight = FontWeights.SemiBold,
                    Margin = new Thickness(0, 10, 0, 4),
                };
                AppendInlines(hp, trimmed.TrimStart('#').Trim(), citationChip, hp.FontSize, math, mathHost, ref formulaCount);
                rtb.Blocks.Add(hp);
                i++;
                continue;
            }
            if (trimmed.StartsWith("- ") || trimmed.StartsWith("* ") || IsOrdered(trimmed, out _))
            {
                FlushPara();
                var marker = IsOrdered(trimmed, out var rest) ? trimmed[..(trimmed.Length - rest.Length)].Trim() : "•";
                var indent = Math.Min(line.Length - trimmed.Length, 12) * 2;
                var lp = new Paragraph
                {
                    Margin = new Thickness(18 + indent, 0, 0, 4),
                    TextIndent = -18,
                };
                lp.Inlines.Add(new Run { Text = $"{marker} " });
                var bodyText = trimmed.StartsWith("- ") || trimmed.StartsWith("* ") ? trimmed[2..] : rest;
                AppendInlines(lp, bodyText, citationChip, fontSize, math, mathHost, ref formulaCount);
                rtb.Blocks.Add(lp);
                i++;
                continue;
            }
            if (trimmed.StartsWith("> "))
            {
                FlushPara();
                var qp = new Paragraph
                {
                    Margin = new Thickness(12, 2, 0, 6),
                    Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 196, 199, 197)),
                };
                qp.Inlines.Add(new Run { Text = "│ ", Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 110, 118, 130)) });
                AppendInlines(qp, trimmed[2..], citationChip, fontSize, math, mathHost, ref formulaCount);
                rtb.Blocks.Add(qp);
                i++;
                continue;
            }
            if (para.Length > 0) para.Append('\n');
            para.Append(line);
            i++;
        }
        FlushPara();
        return rtb;
    }

    private static void AddParagraphWithDisplayMath(
        RichTextBlock rtb,
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
                    var p = new Paragraph { Margin = new Thickness(0, 0, 0, 8) };
                    AppendInlines(p, before, citationChip, fontSize, math, mathHost, ref formulaCount);
                    rtb.Blocks.Add(p);
                }
            }
            var expr = text[(nextIdx + 2)..closeIdx].Trim();
            if (expr.Length > 0)
            {
                var mp = new Paragraph { TextAlignment = TextAlignment.Center, Margin = new Thickness(0, 6, 0, 8) };
                mp.Inlines.Add(new InlineUIContainer { Child = CreateMathBlock(expr, displayMode: true, fontSize, math, mathHost, ref formulaCount) });
                rtb.Blocks.Add(mp);
            }
            pos = closeIdx + 2;
        }

        if (pos < text.Length)
        {
            var tail = text[pos..].Trim();
            if (tail.Length > 0)
            {
                var p = new Paragraph { Margin = new Thickness(0, 0, 0, 8) };
                AppendInlines(p, tail, citationChip, fontSize, math, mathHost, ref formulaCount);
                rtb.Blocks.Add(p);
            }
        }
    }

    internal static int HeadingLevel(string t)
    {
        var n = 0;
        while (n < t.Length && t[n] == '#') n++;
        return n is >= 1 and <= 6 && n < t.Length && t[n] == ' ' ? n : 0;
    }

    internal static bool IsOrdered(string t, out string rest)
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
            FontFamily = BodyFontFamily,
            TextWrapping = TextWrapping.Wrap,
            IsTextSelectionEnabled = true,
            FontSize = fontSize,
            LineHeight = Math.Round(fontSize * 1.72),
            LineStackingStrategy = LineStackingStrategy.BlockLineHeight,
            CharacterSpacing = 12,
            Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 226, 228, 232)),
        };
        var p = new Paragraph();
        rtb.Blocks.Add(p);
        AppendInlines(p, text, citationChip, fontSize, math, mathHost, ref formulaCount);
        return rtb;
    }

    private static void AppendInlines(
        Paragraph p,
        string text,
        Func<int, Inline?>? citationChip,
        double fontSize,
        WebContentHost? math,
        Panel? mathHost,
        ref int formulaCount) =>
        AppendInlinesTo(p.Inlines, text, citationChip, fontSize, math, mathHost, ref formulaCount, bold: false);

    private static void AppendInlinesTo(
        InlineCollection target,
        string text,
        Func<int, Inline?>? citationChip,
        double fontSize,
        WebContentHost? math,
        Panel? mathHost,
        ref int formulaCount,
        bool bold)
    {
        var buf = new StringBuilder();
        void Flush()
        {
            if (buf.Length > 0)
            {
                var run = new Run { Text = buf.ToString(), FontFamily = BodyFontFamily };
                if (bold)
                {
                    run.FontWeight = FontWeights.SemiBold;
                    run.Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 244, 246, 250));
                }
                target.Add(run);
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
                    target.Add(new Run
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
                        target.Add(new InlineUIContainer
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
                    target.Add(new InlineUIContainer
                    {
                        Child = CreateMathBlock(expr, displayMode: false, fontSize, math, mathHost, ref formulaCount),
                    });
                    i = end + 1;
                    continue;
                }
            }
            else if (!bold && c == '*' && i + 1 < text.Length && text[i + 1] == '*')
            {
                var boldEnd = text.IndexOf("**", i + 2, StringComparison.Ordinal);
                if (boldEnd > i + 1)
                {
                    Flush();
                    AppendInlinesTo(target, text[(i + 2)..boldEnd], citationChip, fontSize, math, mathHost, ref formulaCount, bold: true);
                    i = boldEnd + 2;
                    continue;
                }
            }
            else if (c == '[' && citationChip != null)
            {
                var citeEnd = text.IndexOf(']', i + 1);
                if (citeEnd > i + 1 && citeEnd - i <= 4 && int.TryParse(text.AsSpan(i + 1, citeEnd - i - 1), out var n) && n > 0 && citationChip(n) is { } chip)
                {
                    // Trim a single space before citation so superscript sits cleanly next to the text
                    if (buf.Length > 0 && buf[^1] == ' ') buf.Length--;
                    Flush();
                    target.Add(chip);
                    i = citeEnd + 1;
                    continue;
                }
            }
            buf.Append(c);
            i++;
        }
        Flush();
    }

    internal static bool TryFindInlineDollarEnd(string text, int start, out int end)
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
        _ = formulaCount;

        if (latex.Length <= WebContentHost.MaxFormulaLength
            && math != null
            && math.TryGetCachedBitmap(latex, displayMode, out var cachedRes, out var cachedBmp)
            && cachedRes.Success
            && cachedBmp != null)
        {
            return BuildRenderedMathContainer(cachedBmp, cachedRes, latex, displayMode);
        }

        var canRenderAsync = math != null && mathHost != null && latex.Length <= WebContentHost.MaxFormulaLength;
        var rawText = displayMode ? $"$${latex}$$" : $"${latex}$";
        var fallback = new TextBlock
        {
            Text = canRenderAsync ? "…" : rawText,
            FontFamily = new FontFamily("Consolas"),
            FontSize = Math.Max(12, fontSize - 1),
            TextWrapping = displayMode ? TextWrapping.Wrap : TextWrapping.NoWrap,
            IsTextSelectionEnabled = !canRenderAsync,
            Opacity = canRenderAsync ? 0.55 : 1.0,
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

        if (latex.Length > WebContentHost.MaxFormulaLength)
        {
            ApplyErrorStyle(container, fallback, rawText, $"公式长度超过 {WebContentHost.MaxFormulaLength} 字符上限，已回退显示原文");
            return container;
        }
        if (math == null || mathHost == null)
        {
            return container;
        }

        _ = RenderMathIntoAsync(container, fallback, rawText, latex, displayMode, math, mathHost);
        return container;
    }

    private static async Task RenderMathIntoAsync(
        Border container,
        TextBlock fallback,
        string rawText,
        string latex,
        bool displayMode,
        WebContentHost math,
        Panel mathHost)
    {
        var res = await math.RenderFormulaAsync(mathHost, latex, displayMode);
        if (!res.Success || res.PngBytes == null)
        {
            ApplyErrorStyle(container, fallback, rawText, $"公式渲染失败：{res.Error ?? "未知错误"}");
            return;
        }

        try
        {
            using var mem = new InMemoryRandomAccessStream();
            await mem.WriteAsync(res.PngBytes.AsBuffer());
            mem.Seek(0);
            var bmp = new BitmapImage();
            await bmp.SetSourceAsync(mem);
            math.CacheBitmap(latex, displayMode, bmp);

            ApplyRenderedMath(container, bmp, res, latex, displayMode);
        }
        catch (Exception ex)
        {
            ApplyErrorStyle(container, fallback, rawText, $"公式图像解码失败：{ex.Message}");
        }
    }

    private static Border BuildRenderedMathContainer(
        BitmapImage bmp,
        MathRenderResult res,
        string latex,
        bool displayMode)
    {
        var container = new Border
        {
            CornerRadius = new CornerRadius(6),
            Padding = displayMode ? new Thickness(8, 6, 8, 6) : new Thickness(1, 0, 1, 0),
            Margin = displayMode ? new Thickness(0, 4, 0, 4) : new Thickness(1, 0, 1, -res.Descent),
            Background = displayMode
                ? new SolidColorBrush(Windows.UI.Color.FromArgb(60, 19, 19, 20))
                : new SolidColorBrush(Windows.UI.Color.FromArgb(0, 0, 0, 0)),
        };
        ApplyRenderedMath(container, bmp, res, latex, displayMode);
        return container;
    }

    private static void ApplyRenderedMath(
        Border container,
        BitmapImage bmp,
        MathRenderResult res,
        string latex,
        bool displayMode)
    {
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
            container.VerticalAlignment = VerticalAlignment.Center;
            container.Child = new ScrollViewer
            {
                HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
                VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
                Content = img,
            };
        }
        else
        {
            container.Padding = new Thickness(1, 0, 1, 0);
            container.VerticalAlignment = VerticalAlignment.Bottom;
            container.Margin = new Thickness(1, 0, 1, -res.Descent);
            container.Child = img;
        }
    }

    private static void ApplyErrorStyle(Border container, TextBlock fallback, string rawText, string reason)
    {
        container.BorderBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(160, 240, 120, 120));
        container.BorderThickness = new Thickness(1);
        container.Background = new SolidColorBrush(Windows.UI.Color.FromArgb(40, 180, 60, 60));
        fallback.Text = rawText;
        fallback.Opacity = 1.0;
        fallback.IsTextSelectionEnabled = true;
        fallback.Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 190, 190));
        ToolTipService.SetToolTip(container, reason);
    }
}
