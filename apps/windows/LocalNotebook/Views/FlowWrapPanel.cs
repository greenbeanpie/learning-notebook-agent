using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Foundation;

namespace LocalNotebook.Views;

/// <summary>
/// Horizontal flow-wrap panel that places children left-to-right on the current row
/// and wraps to the next row only when the remaining horizontal width is insufficient.
/// </summary>
public sealed partial class FlowWrapPanel : Panel
{
    public double HorizontalSpacing { get; set; } = 8;
    public double VerticalSpacing { get; set; } = 8;

    protected override Size MeasureOverride(Size availableSize)
    {
        var maxWidth = double.IsInfinity(availableSize.Width) || availableSize.Width <= 0
            ? double.PositiveInfinity
            : availableSize.Width;

        double lineWidth = 0;
        double lineHeight = 0;
        double totalHeight = 0;
        double maxLineWidth = 0;

        foreach (var child in Children)
        {
            child.Measure(new Size(maxWidth, double.PositiveInfinity));
            var sz = child.DesiredSize;

            if (lineWidth > 0 && lineWidth + HorizontalSpacing + sz.Width > maxWidth)
            {
                maxLineWidth = Math.Max(maxLineWidth, lineWidth);
                totalHeight += lineHeight + VerticalSpacing;
                lineWidth = sz.Width;
                lineHeight = sz.Height;
            }
            else
            {
                if (lineWidth > 0) lineWidth += HorizontalSpacing;
                lineWidth += sz.Width;
                lineHeight = Math.Max(lineHeight, sz.Height);
            }
        }

        maxLineWidth = Math.Max(maxLineWidth, lineWidth);
        totalHeight += lineHeight;

        return new Size(
            double.IsInfinity(maxWidth) ? maxLineWidth : Math.Min(maxWidth, maxLineWidth),
            totalHeight);
    }

    protected override Size ArrangeOverride(Size finalSize)
    {
        var maxWidth = finalSize.Width > 0 ? finalSize.Width : double.PositiveInfinity;
        double x = 0;
        double y = 0;
        double lineHeight = 0;

        foreach (var child in Children)
        {
            var sz = child.DesiredSize;
            if (x > 0 && x + HorizontalSpacing + sz.Width > maxWidth)
            {
                x = 0;
                y += lineHeight + VerticalSpacing;
                lineHeight = 0;
            }
            else if (x > 0)
            {
                x += HorizontalSpacing;
            }

            var arrangeW = double.IsInfinity(maxWidth) ? sz.Width : Math.Min(sz.Width, Math.Max(0, maxWidth - x));
            child.Arrange(new Rect(x, y, arrangeW, sz.Height));
            x += arrangeW;
            lineHeight = Math.Max(lineHeight, sz.Height);
        }

        return new Size(finalSize.Width, y + lineHeight);
    }
}
