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

    private static readonly HashSet<string> ChemicalElements = new(StringComparer.Ordinal)
    {
        "H", "He", "Li", "Be", "B", "C", "N", "O", "F", "Ne",
        "Na", "Mg", "Al", "Si", "P", "S", "Cl", "Ar", "K", "Ca",
        "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni", "Cu", "Zn",
        "Ga", "Ge", "As", "Se", "Br", "Kr", "Rb", "Sr", "Y", "Zr",
        "Nb", "Mo", "Tc", "Ru", "Rh", "Pd", "Ag", "Cd", "In", "Sn",
        "Sb", "Te", "I", "Xe", "Cs", "Ba", "La", "Ce", "Pr", "Nd",
        "Pm", "Sm", "Eu", "Gd", "Tb", "Dy", "Ho", "Er", "Tm", "Yb",
        "Lu", "Hf", "Ta", "W", "Re", "Os", "Ir", "Pt", "Au", "Hg",
        "Tl", "Pb", "Bi", "Po", "At", "Rn", "Fr", "Ra", "Ac", "Th",
        "Pa", "U"
    };

    private static readonly System.Text.RegularExpressions.Regex BareChemRegex = new(
        @"(?<![A-Za-z0-9_$\{\\])(?:(?:H2|N2|O2|O3|F2|Cl2|Br2|I2|P4|S8|C60|C70)|(?:(?:[A-Z][a-z]?\d*|\((?:[A-Z][a-z]?\d*)+\)\d*){2,}))(?![A-Za-z0-9_$\}\]])",
        System.Text.RegularExpressions.RegexOptions.Compiled);

    private static readonly System.Text.RegularExpressions.Regex MathCodeSpanRegex = new(
        @"\$\$[\s\S]*?\$\$|\$[^\$\r\n]+?\$|\\\[[\s\S]*?\\\]|\\\([\s\S]*?\\\)|`[^`\r\n]+?`",
        System.Text.RegularExpressions.RegexOptions.Compiled);

    /// <summary>
    /// Wraps unambiguous bare ASCII chemical formulas (e.g. O2, H2O, CO2, H2SO4) outside existing
    /// math/code spans into $\ce{...}$ so KaTeX + mhchem renders proper subscripts.
    /// </summary>
    public static string NormalizeQuizMath(string text)
    {
        if (string.IsNullOrEmpty(text)) return text;
        var sb = new StringBuilder(text.Length);
        var last = 0;
        foreach (System.Text.RegularExpressions.Match span in MathCodeSpanRegex.Matches(text))
        {
            if (span.Index > last)
            {
                sb.Append(WrapBareChem(text[last..span.Index]));
            }
            sb.Append(span.Value);
            last = span.Index + span.Length;
        }
        if (last < text.Length)
        {
            sb.Append(WrapBareChem(text[last..]));
        }
        return sb.ToString();
    }

    private static string WrapBareChem(string segment) => BareChemRegex.Replace(segment, m =>
    {
        var val = m.Value;
        var hasDigit = false;
        var elemCount = 0;
        var i = 0;
        while (i < val.Length)
        {
            var ch = val[i];
            if (ch is '(' or ')') { i++; continue; }
            if (char.IsDigit(ch)) { hasDigit = true; i++; continue; }
            if (ch is >= 'A' and <= 'Z')
            {
                var len = (i + 1 < val.Length && val[i + 1] is >= 'a' and <= 'z') ? 2 : 1;
                var sym = val.Substring(i, len);
                if (!ChemicalElements.Contains(sym)) return val;
                elemCount++;
                i += len;
                continue;
            }
            return val;
        }
        return hasDigit && elemCount >= 1 ? $"$\\ce{{{val}}}$" : val;
    });

    public static readonly FontFamily BodyFontFamily = new("Microsoft YaHei UI, Segoe UI");

    public static Inline SuperscriptCitation(int no, string? tooltip = null, Action? onClick = null) =>
        SuperscriptCitation(no, tooltip, onClick == null ? null : (Action<FrameworkElement>)(_ => onClick()));

    public static Inline SuperscriptCitation(int no, string? tooltip, Action<FrameworkElement>? onClick)
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
                onClick(pill);
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
        fontSize = effectiveSize;
        var host = new StackPanel { Spacing = 0, HorizontalAlignment = HorizontalAlignment.Stretch };
        RichTextBlock? currentRtb = null;

        RichTextBlock EnsureRtb()
        {
            if (currentRtb != null) return currentRtb;
            currentRtb = new RichTextBlock
            {
                FontFamily = BodyFontFamily,
                TextWrapping = TextWrapping.Wrap,
                IsTextSelectionEnabled = true,
                FontSize = effectiveSize,
                LineHeight = Math.Round(effectiveSize * 1.72),
                LineStackingStrategy = LineStackingStrategy.MaxHeight,
                CharacterSpacing = 12,
                Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 226, 228, 232)),
            };
            host.Children.Add(currentRtb);
            return currentRtb;
        }

        void AddDisplayMathElement(string expr, ref int fCount)
        {
            currentRtb = null;
            host.Children.Add(CreateMathBlock(expr, displayMode: true, fontSize, math, mathHost, ref fCount));
        }

        var lines = markdown.Replace("\r\n", "\n").Split('\n');
        var para = new StringBuilder();
        var i = 0;
        var formulaCount = 0;

        void FlushPara()
        {
            if (para.Length == 0) return;
            var text = para.ToString().TrimEnd();
            para.Clear();
            AddParagraphWithDisplayMath(EnsureRtb, AddDisplayMathElement, text, citationChip, fontSize, math, mathHost, ref formulaCount);
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
                EnsureRtb().Blocks.Add(codePara);
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
                    AddDisplayMathElement(expr, ref formulaCount);
                }
                else
                {
                    var p = new Paragraph { Margin = new Thickness(0, 0, 0, 8) };
                    AppendInlines(p, trimmed + "\n" + expr, citationChip, fontSize, math, mathHost, ref formulaCount);
                    EnsureRtb().Blocks.Add(p);
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
                EnsureRtb().Blocks.Add(hp);
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
                EnsureRtb().Blocks.Add(lp);
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
                EnsureRtb().Blocks.Add(qp);
                i++;
                continue;
            }
            if (para.Length > 0) para.Append('\n');
            para.Append(line);
            i++;
        }
        FlushPara();
        if (host.Children.Count == 1)
        {
            var single = host.Children[0];
            host.Children.Clear();
            return single;
        }
        return host;
    }

    private delegate void DisplayMathAdder(string expr, ref int formulaCount);

    private static void AddParagraphWithDisplayMath(
        Func<RichTextBlock> ensureRtb,
        DisplayMathAdder addDisplayMath,
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
                    ensureRtb().Blocks.Add(p);
                }
            }
            var expr = text[(nextIdx + 2)..closeIdx].Trim();
            if (expr.Length > 0)
            {
                addDisplayMath(expr, ref formulaCount);
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
                ensureRtb().Blocks.Add(p);
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
            LineStackingStrategy = LineStackingStrategy.MaxHeight,
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

        // Recover common LaTeX commands corrupted by single-backslash JSON escaping (\frac, \times, \beta, \rightarrow)
        if (latex.IndexOfAny(['\f', '\t', '\b', '\r']) >= 0)
        {
            latex = latex
                .Replace("\f", "\\f")
                .Replace("\t", "\\t")
                .Replace("\b", "\\b")
                .Replace("\r", "\\r");
        }

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
            CornerRadius = new CornerRadius(8),
            Padding = displayMode ? new Thickness(12, 10, 12, 10) : new Thickness(3, 0, 3, 0),
            Margin = displayMode ? new Thickness(0, 10, 0, 10) : new Thickness(1, 0, 1, 0),
            HorizontalAlignment = displayMode ? HorizontalAlignment.Stretch : HorizontalAlignment.Left,
            Background = displayMode
                ? new SolidColorBrush(Windows.UI.Color.FromArgb(60, 19, 19, 20))
                : new SolidColorBrush(Windows.UI.Color.FromArgb(0, 0, 0, 0)),
            VerticalAlignment = VerticalAlignment.Center,
            Child = displayMode
                ? new ScrollViewer
                {
                    HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
                    VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
                    HorizontalAlignment = HorizontalAlignment.Stretch,
                    HorizontalContentAlignment = HorizontalAlignment.Center,
                    Content = fallback,
                }
                : (UIElement)fallback,
        };

        if (latex.Length > WebContentHost.MaxFormulaLength)
        {
            ApplyErrorStyle(container, fallback, rawText, L10n.Fmt("math.too_long_fallback", WebContentHost.MaxFormulaLength));
            return container;
        }
        if (math == null || mathHost == null)
        {
            return container;
        }

        // Approximate placeholder size so layout does not jump badly when the image arrives.
        if (displayMode)
        {
            container.MinHeight = 64;
        }
        else
        {
            container.MinHeight = Math.Round(fontSize * 1.4);
            container.MinWidth = Math.Min(260, 10 + latex.Length * 6.0);
        }
        MathVisibilityGate.Defer(
            container, math, mathHost, latex, displayMode,
            () => _ = RenderMathIntoAsync(container, fallback, rawText, latex, displayMode, math, mathHost));
        return container;
    }

    /// <summary>
    /// Defers formula rendering until the placeholder is near the visible viewport of its vertical ScrollViewer.
    /// Formulas already cached are applied at once (no WebView2 needed).
    /// </summary>
    private static class MathVisibilityGate
    {
        private sealed class Entry
        {
            public required Border Container;
            public required Action Start;
            public required Func<bool> Dead;
        }

        private sealed class State
        {
            public readonly List<Entry> Pending = new();
            public DispatcherTimer? Timer;
        }

        private static readonly System.Runtime.CompilerServices.ConditionalWeakTable<ScrollViewer, State> States = new();

        public static void Defer(
            Border container, WebContentHost math, Panel mathHost, string latex, bool displayMode, Action start)
        {
            var started = false;
            void Go()
            {
                if (started) return;
                started = true;
                start();
            }

            var unloaded = false;
            container.Unloaded += (_, _) => unloaded = true;
            container.Loaded += (_, _) =>
            {
                unloaded = false;
                if (started) return;
                var scale = container.XamlRoot?.RasterizationScale ?? 1.0;
                if (math.IsCached(latex, displayMode, scale)) { Go(); return; }
                math.NoteUncached();
                var sv = FindVerticalScrollViewer(container);
                if (sv == null) { Go(); return; }
                var state = States.GetValue(sv, _ => new State());
                if (state.Pending.Any(e => ReferenceEquals(e.Container, container))) return;
                state.Pending.Add(new Entry { Container = container, Start = Go, Dead = () => unloaded });
                if (Check(sv, state)) return;
                EnsureTimer(sv, state);
            };
        }

        private static ScrollViewer? FindVerticalScrollViewer(DependencyObject start)
        {
            var cur = VisualTreeHelper.GetParent(start);
            while (cur != null)
            {
                if (cur is ScrollViewer sv && sv.VerticalScrollBarVisibility != ScrollBarVisibility.Disabled)
                    return sv;
                cur = VisualTreeHelper.GetParent(cur);
            }
            return null;
        }

        private static void EnsureTimer(ScrollViewer sv, State state)
        {
            if (state.Timer != null)
            {
                if (!state.Timer.IsEnabled) state.Timer.Start();
                return;
            }
            state.Timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(250) };
            state.Timer.Tick += (_, _) =>
            {
                if (Check(sv, state)) state.Timer!.Stop();
            };
            state.Timer.Start();
        }

        /// <summary>Starts visible entries; returns true when nothing is left pending.</summary>
        private static bool Check(ScrollViewer sv, State state)
        {
            if (state.Pending.Count == 0) return true;
            var vh = sv.ViewportHeight;
            if (vh <= 0) return false;
            var margin = vh; // render one extra screen above and below
            for (var i = state.Pending.Count - 1; i >= 0; i--)
            {
                var e = state.Pending[i];
                // Removed from the tree for good: forget it. (IsLoaded is also false while the Loaded
                // handlers of a burst of siblings are still running, so that alone proves nothing.)
                if (e.Dead()) { state.Pending.RemoveAt(i); continue; }
                if (!e.Container.IsLoaded) continue;
                try
                {
                    var p = e.Container.TransformToVisual(sv).TransformPoint(new Windows.Foundation.Point(0, 0));
                    var h = Math.Max(e.Container.ActualHeight, 1);
                    if (p.Y + h >= -margin && p.Y <= vh + margin)
                    {
                        state.Pending.RemoveAt(i);
                        e.Start();
                    }
                }
                catch
                {
                    state.Pending.RemoveAt(i);
                    e.Start();
                }
            }
            return state.Pending.Count == 0;
        }
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
        if (!res.Success)
        {
            ApplyErrorStyle(container, fallback, rawText, L10n.Fmt("math.render_failed", res.Error ?? L10n.T("math.unknown_error")));
            return;
        }

        if (res.Image != null)
        {
            ApplyRenderedMath(container, res.Image, res, latex, displayMode);
            return;
        }

        if (res.PngBytes == null)
        {
            ApplyErrorStyle(container, fallback, rawText, L10n.Fmt("math.render_failed", res.Error ?? L10n.T("math.unknown_error")));
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
            ApplyErrorStyle(container, fallback, rawText, L10n.Fmt("math.decode_failed", ex.Message));
        }
    }

    private static Border BuildRenderedMathContainer(
        ImageSource bmp,
        MathRenderResult res,
        string latex,
        bool displayMode)
    {
        var container = new Border
        {
            CornerRadius = new CornerRadius(8),
            Padding = displayMode ? new Thickness(12, 10, 12, 10) : new Thickness(1, 0, 1, 0),
            Margin = displayMode ? new Thickness(0, 10, 0, 10) : new Thickness(1, 0, 1, -res.Descent),
            HorizontalAlignment = displayMode ? HorizontalAlignment.Stretch : HorizontalAlignment.Left,
            Background = displayMode
                ? new SolidColorBrush(Windows.UI.Color.FromArgb(60, 19, 19, 20))
                : new SolidColorBrush(Windows.UI.Color.FromArgb(0, 0, 0, 0)),
        };
        ApplyRenderedMath(container, bmp, res, latex, displayMode);
        return container;
    }

    private static void ApplyRenderedMath(
        Border container,
        ImageSource bmp,
        MathRenderResult res,
        string latex,
        bool displayMode)
    {
        container.MinWidth = 0;
        container.MinHeight = 0;
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
            res.Overflow ? L10n.Fmt("math.overflow_tip", latex) : latex);

        if (displayMode)
        {
            container.HorizontalAlignment = HorizontalAlignment.Stretch;
            container.VerticalAlignment = VerticalAlignment.Center;
            var centeredWrap = new Grid
            {
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
                Padding = new Thickness(4, 2, 4, 2),
                Children = { img },
            };
            container.Child = new ScrollViewer
            {
                HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
                VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Center,
                VerticalContentAlignment = VerticalAlignment.Center,
                Content = centeredWrap,
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
        container.MinWidth = 0;
        container.MinHeight = 0;
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
