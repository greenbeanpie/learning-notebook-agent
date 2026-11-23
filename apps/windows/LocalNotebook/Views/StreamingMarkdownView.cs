using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Media;
using System.Text;

namespace LocalNotebook.Views;

/// <summary>
/// Incremental Markdown + LaTeX streaming view for assistant replies: commits finished blocks once
/// and only re-renders the active tail block while sanitizing incomplete inline/block delimiters so
/// raw Markdown/TeX tokens are never flashed during streaming.
/// </summary>
public sealed class StreamingMarkdownView
{
    public StackPanel Root { get; }

    private readonly StackPanel _committedPanel = new() { Spacing = 0 };
    private readonly ContentControl _tailHost = new() { HorizontalContentAlignment = HorizontalAlignment.Stretch };
    private readonly List<string> _committedBlocks = new();
    private readonly Func<int, Inline?>? _citationChip;
    private readonly double _fontSize;
    private readonly WebContentHost? _math;
    private readonly Panel? _mathHost;
    private string _lastTailKey = "";

    public StreamingMarkdownView(
        Func<int, Inline?>? citationChip,
        double fontSize = 15,
        WebContentHost? math = null,
        Panel? mathHost = null)
    {
        _citationChip = citationChip;
        _fontSize = fontSize;
        _math = math;
        _mathHost = mathHost;
        Root = new StackPanel { Spacing = 0 };
        Root.Children.Add(_committedPanel);
        Root.Children.Add(_tailHost);
    }

    public void Update(string rawMarkdown)
    {
        var (committed, activeTail, unclosedDisplayMath) = SplitBlocks(rawMarkdown);

        var commonPrefix = 0;
        var limit = Math.Min(_committedBlocks.Count, committed.Count);
        while (commonPrefix < limit && string.Equals(_committedBlocks[commonPrefix], committed[commonPrefix], StringComparison.Ordinal))
        {
            commonPrefix++;
        }

        while (_committedBlocks.Count > commonPrefix)
        {
            var last = _committedBlocks.Count - 1;
            _committedBlocks.RemoveAt(last);
            _committedPanel.Children.RemoveAt(last);
        }

        for (var i = commonPrefix; i < committed.Count; i++)
        {
            var block = committed[i];
            _committedBlocks.Add(block);
            _committedPanel.Children.Add(Md.Render(block, _citationChip, _fontSize, _math, _mathHost));
        }

        if (unclosedDisplayMath)
        {
            const string pendingKey = "\u0001MATH_PENDING";
            if (!string.Equals(_lastTailKey, pendingKey, StringComparison.Ordinal))
            {
                _tailHost.Content = CreatePendingMathBadge();
                _lastTailKey = pendingKey;
            }
            return;
        }

        if (string.IsNullOrWhiteSpace(activeTail))
        {
            if (_lastTailKey.Length > 0)
            {
                _tailHost.Content = null;
                _lastTailKey = "";
            }
            return;
        }

        var sanitized = PrepareStreamingTail(activeTail);
        if (string.Equals(sanitized, _lastTailKey, StringComparison.Ordinal))
        {
            return;
        }

        _tailHost.Content = sanitized.Length > 0
            ? Md.Render(sanitized, _citationChip, _fontSize, _math, _mathHost)
            : null;
        _lastTailKey = sanitized;
    }

    private static UIElement CreatePendingMathBadge() => new Border
    {
        CornerRadius = new CornerRadius(6),
        Padding = new Thickness(10, 6, 10, 6),
        Margin = new Thickness(0, 4, 0, 4),
        HorizontalAlignment = HorizontalAlignment.Left,
        Background = new SolidColorBrush(Windows.UI.Color.FromArgb(45, 255, 255, 255)),
        Child = new TextBlock
        {
            Text = Services.L10n.T("math.rendering"),
            FontSize = 12,
            Opacity = 0.7,
        },
    };

    internal static (List<string> Committed, string ActiveTail, bool UnclosedDisplayMath) SplitBlocks(string rawMarkdown)
    {
        var committed = new List<string>();
        if (string.IsNullOrEmpty(rawMarkdown))
        {
            return (committed, "", false);
        }

        var lines = rawMarkdown.Replace("\r\n", "\n").Split('\n');
        var para = new StringBuilder();
        var activeTail = "";
        var unclosedDisplayMath = false;

        void FlushPara()
        {
            if (para.Length == 0) return;
            var text = para.ToString().TrimEnd();
            para.Clear();
            if (text.Length > 0)
            {
                committed.Add(text);
            }
        }

        var i = 0;
        while (i < lines.Length)
        {
            var line = lines[i];
            var trimmed = line.TrimStart();

            if (trimmed.StartsWith("```", StringComparison.Ordinal))
            {
                FlushPara();
                var closeIdx = -1;
                for (var j = i + 1; j < lines.Length; j++)
                {
                    if (lines[j].TrimStart().StartsWith("```", StringComparison.Ordinal))
                    {
                        closeIdx = j;
                        break;
                    }
                }
                if (closeIdx >= 0 && closeIdx < lines.Length - 1)
                {
                    committed.Add(string.Join('\n', lines, i, closeIdx - i + 1));
                    i = closeIdx + 1;
                    continue;
                }
                activeTail = string.Join('\n', lines, i, lines.Length - i);
                unclosedDisplayMath = false;
                break;
            }

            if ((trimmed.StartsWith("$$", StringComparison.Ordinal) && !trimmed[2..].Contains("$$", StringComparison.Ordinal)) ||
                (trimmed.StartsWith("\\[", StringComparison.Ordinal) && !trimmed[2..].Contains("\\]", StringComparison.Ordinal)))
            {
                FlushPara();
                var closeDelim = trimmed.StartsWith("$$", StringComparison.Ordinal) ? "$$" : "\\]";
                var closeIdx = -1;
                for (var j = i + 1; j < lines.Length; j++)
                {
                    if (lines[j].IndexOf(closeDelim, StringComparison.Ordinal) >= 0)
                    {
                        closeIdx = j;
                        break;
                    }
                }
                if (closeIdx >= 0 && closeIdx < lines.Length - 1)
                {
                    committed.Add(string.Join('\n', lines, i, closeIdx - i + 1));
                    i = closeIdx + 1;
                    continue;
                }
                if (closeIdx >= 0)
                {
                    activeTail = string.Join('\n', lines, i, lines.Length - i);
                    unclosedDisplayMath = false;
                }
                else
                {
                    activeTail = "";
                    unclosedDisplayMath = true;
                }
                break;
            }

            if (trimmed.Length == 0)
            {
                FlushPara();
                i++;
                continue;
            }

            if (Md.HeadingLevel(trimmed) > 0 ||
                trimmed.StartsWith("- ", StringComparison.Ordinal) ||
                trimmed.StartsWith("* ", StringComparison.Ordinal) ||
                Md.IsOrdered(trimmed, out _) ||
                trimmed.StartsWith("> ", StringComparison.Ordinal))
            {
                FlushPara();
                if (i < lines.Length - 1)
                {
                    committed.Add(line);
                    i++;
                    continue;
                }
                activeTail = line;
                break;
            }

            if (para.Length > 0) para.Append('\n');
            para.Append(line);
            i++;
        }

        if (!unclosedDisplayMath && activeTail.Length == 0 && para.Length > 0)
        {
            activeTail = para.ToString();
        }

        return (committed, activeTail, unclosedDisplayMath);
    }

    internal static string PrepareStreamingTail(string tail)
    {
        if (string.IsNullOrEmpty(tail)) return "";
        if (tail.TrimStart().StartsWith("```", StringComparison.Ordinal))
        {
            return tail;
        }

        var lastNl = tail.LastIndexOf('\n');
        var prefix = lastNl >= 0 ? tail[..(lastNl + 1)] : "";
        var line = lastNl >= 0 ? tail[(lastNl + 1)..] : tail;
        var cleanLine = SanitizeStreamingLine(line);
        return prefix.Length == 0 ? cleanLine : prefix + cleanLine;
    }

    private static string SanitizeStreamingLine(string line)
    {
        if (string.IsNullOrEmpty(line)) return "";

        // Hide incomplete leading heading hash sequence (e.g. "###" or "### ") before title text arrives.
        var trimmedStart = line.TrimStart();
        var nonHashIdx = 0;
        while (nonHashIdx < trimmedStart.Length && trimmedStart[nonHashIdx] == '#') nonHashIdx++;
        if (nonHashIdx <= 6 && (nonHashIdx == trimmedStart.Length || trimmedStart[nonHashIdx..].Trim().Length == 0))
        {
            return "";
        }

        int? openBoldStart = null;
        var i = 0;
        while (i < line.Length)
        {
            var c = line[i];

            if (c == '\\' && i + 1 < line.Length && line[i + 1] == '$')
            {
                i += 2;
                continue;
            }

            if (c == '`')
            {
                var end = line.IndexOf('`', i + 1);
                if (end > i)
                {
                    i = end + 1;
                    continue;
                }
                var closed = i + 1 < line.Length ? line + "`" : line[..i];
                return openBoldStart.HasValue ? CloseUnfinishedBold(closed, openBoldStart.Value) : closed;
            }

            if (c == '\\' && i + 1 < line.Length && line[i + 1] == '(')
            {
                var end = line.IndexOf("\\)", i + 2, StringComparison.Ordinal);
                if (end >= 0)
                {
                    i = end + 2;
                    continue;
                }
                return TruncateBeforeOpenMath(line, i, openBoldStart);
            }

            if (c == '\\' && i + 1 < line.Length && line[i + 1] == '[')
            {
                var end = line.IndexOf("\\]", i + 2, StringComparison.Ordinal);
                if (end >= 0)
                {
                    i = end + 2;
                    continue;
                }
                return TruncateBeforeOpenMath(line, i, openBoldStart);
            }

            if (c == '$' && i + 1 < line.Length && line[i + 1] == '$')
            {
                var end = line.IndexOf("$$", i + 2, StringComparison.Ordinal);
                if (end >= 0)
                {
                    i = end + 2;
                    continue;
                }
                return TruncateBeforeOpenMath(line, i, openBoldStart);
            }

            if (c == '$')
            {
                if (Md.TryFindInlineDollarEnd(line, i + 1, out var end))
                {
                    i = end + 1;
                    continue;
                }
                if (i + 1 == line.Length)
                {
                    return TruncateBeforeOpenMath(line, i, openBoldStart, appendEllipsis: false);
                }
                if (char.IsWhiteSpace(line[i + 1]) || LooksLikeLiteralCurrency(line.AsSpan(i + 1)))
                {
                    i++;
                    continue;
                }
                return TruncateBeforeOpenMath(line, i, openBoldStart);
            }

            if (c == '*' && i + 1 < line.Length && line[i + 1] == '*')
            {
                var boldEnd = line.IndexOf("**", i + 2, StringComparison.Ordinal);
                if (boldEnd > i + 1)
                {
                    i = boldEnd + 2;
                    continue;
                }
                openBoldStart ??= i;
                i += 2;
                continue;
            }

            i++;
        }

        if (openBoldStart.HasValue)
        {
            return CloseUnfinishedBold(line, openBoldStart.Value);
        }

        return TrimDanglingTokenTail(line);
    }

    private static string TruncateBeforeOpenMath(
        string line,
        int mathStart,
        int? openBoldStart,
        bool appendEllipsis = true)
    {
        var head = line[..mathStart];
        if (openBoldStart.HasValue && openBoldStart.Value < mathStart)
        {
            head = CloseUnfinishedBold(head, openBoldStart.Value);
        }
        if (!appendEllipsis)
        {
            return head;
        }
        return head.Length == 0 ? "…" : head + "…";
    }

    private static string CloseUnfinishedBold(string text, int boldStart)
    {
        if (boldStart + 2 >= text.Length)
        {
            return text[..boldStart];
        }
        if (text[^1] == '*' && text.Length - boldStart > 3 && !char.IsWhiteSpace(text[^2]))
        {
            return text + "*";
        }
        return text + "**";
    }

    private static string TrimDanglingTokenTail(string line)
    {
        if (string.IsNullOrEmpty(line)) return line;

        // Trim a partial numeric citation like "[", "[1", "[12" at the very end.
        var lastOpen = line.LastIndexOf('[');
        if (lastOpen >= 0 && line.Length - lastOpen <= 4)
        {
            var onlyDigits = true;
            for (var k = lastOpen + 1; k < line.Length; k++)
            {
                if (!char.IsAsciiDigit(line[k]))
                {
                    onlyDigits = false;
                    break;
                }
            }
            if (onlyDigits)
            {
                return line[..lastOpen];
            }
        }

        // Trim a bare trailing backslash (e.g. start of \( or \[) or single trailing '*' (start of **).
        var lastChar = line[^1];
        if (lastChar == '\\' || (lastChar == '*' && line.Length > 1 && line[^2] != ' '))
        {
            return line[..^1];
        }

        return line;
    }

    private static bool LooksLikeLiteralCurrency(ReadOnlySpan<char> afterDollar)
    {
        if (afterDollar.IsEmpty || !char.IsAsciiDigit(afterDollar[0]))
        {
            return false;
        }

        var sawSpace = false;
        foreach (var ch in afterDollar)
        {
            if (char.IsWhiteSpace(ch))
            {
                sawSpace = true;
                continue;
            }
            if (ch is '\\' or '^' or '_' or '{' or '}' or '=' or '<' or '>' or '+' or '/' or '*' or '|')
            {
                return false;
            }
        }
        return sawSpace;
    }
}
