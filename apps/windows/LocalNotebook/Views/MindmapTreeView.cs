using LocalNotebook.Services;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using System.Text.Json;
using Windows.Foundation;
using Windows.UI;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>
/// Interactive left-to-right collapsible mind map tree diagram with curved Bezier links,
/// depth-coded rounded nodes, per-branch expand/collapse buttons, and left floating zoom/export controls.
/// </summary>
internal sealed class MindmapTreeView
{
    private const string ExpandCollapseIconPath = "M12,5.83L15.17,9L16.58,7.59L12,3L7.41,7.59L8.83,9L12,5.83M12,18.17L8.83,15L7.42,16.41L12,21L16.59,16.41L15.17,15L12,18.17Z";
    private const string PlusIconPath = "M19,13H13V19H11V13H5V11H11V5H13V11H19V13Z";
    private const string MinusIconPath = "M19,13H5V11H19V13Z";
    private const string DownloadIconPath = "M5,20H19V18H5M19,9H15V3H9V9H5L12,16L19,9Z";

    private sealed class MindNode
    {
        public string Id { get; init; } = "";
        public string Label { get; init; } = "";
        public int Depth { get; set; }
        public bool IsCollapsed { get; set; }
        public List<MindNode> Children { get; } = new();
        public double X { get; set; }
        public double Y { get; set; }
        public double Width { get; set; }
        public double Height { get; set; } = 36;
    }

    public FrameworkElement Root { get; }

    private readonly List<MindNode> _roots = new();
    private readonly Canvas _canvas = new();
    private readonly ScaleTransform _scale = new() { ScaleX = 1.0, ScaleY = 1.0 };
    private readonly ScrollViewer _scroll;
    private readonly bool _compact;
    private readonly Action? _onDownloadSvg;
    private bool _allCollapsed;

    private const double NodeHeight = 36;
    private const double RowGap = 14;
    private const double ColGap = 68;
    private const double ToggleSize = 22;
    private const double CanvasPadX = 72;
    private const double CanvasPadY = 36;

    public MindmapTreeView(JsonElement payload, Action? onDownloadSvg, bool compact, FrameworkElement? feedbackBar = null)
    {
        _compact = compact;
        _onDownloadSvg = onDownloadSvg;

        var rawNodes = Arr(payload, "nodes").ToList();
        if (rawNodes.Count == 0)
        {
            Root = new TextBlock { Text = L10n.T("mindmap.empty"), Margin = new Thickness(0, 12, 0, 12) };
            return;
        }

        BuildHierarchy(rawNodes);

        var canvasHost = new Grid
        {
            RenderTransform = _scale,
            RenderTransformOrigin = new Point(0, 0),
            Children = { _canvas },
        };

        _scroll = new ScrollViewer
        {
            Content = canvasHost,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            Background = new SolidColorBrush(Color.FromArgb(255, 29, 31, 36)),
        };

        var controls = BuildFloatingControls();

        var stage = new Border
        {
            CornerRadius = new CornerRadius(12),
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            Background = new SolidColorBrush(Color.FromArgb(255, 29, 31, 36)),
            Child = new Grid
            {
                Children = { _scroll, controls },
            },
        };

        var outer = new Grid { RowSpacing = 8 };
        if (_compact) outer.Height = 420;
        outer.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        outer.Children.Add(stage);
        if (feedbackBar != null)
        {
            outer.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            Grid.SetRow(feedbackBar, 1);
            outer.Children.Add(feedbackBar);
        }

        Root = outer;
        Redraw();
    }

    private void BuildHierarchy(List<JsonElement> rawNodes)
    {
        var map = new Dictionary<string, MindNode>();
        var parentById = new Dictionary<string, string>();
        var order = new List<MindNode>();

        foreach (var n in rawNodes)
        {
            var id = Str(n, "id");
            if (id.Length == 0) id = Guid.NewGuid().ToString("N");
            var label = Str(n, "label");
            var nums = Arr(n, "citations").Where(c => c.ValueKind == JsonValueKind.Number).Select(c => $"[{c.GetInt32()}]").ToArray();
            if (nums.Length > 0) label += " " + string.Join("", nums);
            var node = new MindNode
            {
                Id = id,
                Label = label,
                Width = EstimateWidth(label),
            };
            map[id] = node;
            parentById[id] = n.TryGetProperty("parent_id", out var p) && p.ValueKind == JsonValueKind.String ? (p.GetString() ?? "") : "";
            order.Add(node);
        }

        foreach (var node in order)
        {
            var pid = parentById[node.Id];
            if (pid.Length > 0 && pid != node.Id && map.TryGetValue(pid, out var parent))
                parent.Children.Add(node);
            else
                _roots.Add(node);
        }

        if (_roots.Count == 0 && order.Count > 0)
            _roots.Add(order[0]);

        foreach (var r in _roots)
            AssignDepth(r, 0);
    }

    private static void AssignDepth(MindNode node, int depth)
    {
        node.Depth = depth;
        foreach (var child in node.Children)
            AssignDepth(child, depth + 1);
    }

    private static double EstimateWidth(string label)
    {
        double units = 0;
        foreach (var ch in label)
            units += ch > 127 ? 13.5 : 7.5;
        return Math.Clamp(units + 28, 96, 280);
    }

    private void Redraw()
    {
        _canvas.Children.Clear();

        double cursorY = CanvasPadY;
        double maxX = 400;
        double maxY = 300;

        double LayoutNode(MindNode node, double x)
        {
            node.X = x;
            double childX = x + node.Width + ColGap;
            if (node.Children.Count == 0 || node.IsCollapsed)
            {
                node.Y = cursorY;
                cursorY += NodeHeight + RowGap;
            }
            else
            {
                var firstY = LayoutNode(node.Children[0], childX);
                var lastY = firstY;
                for (int i = 1; i < node.Children.Count; i++)
                    lastY = LayoutNode(node.Children[i], childX);
                node.Y = (firstY + lastY) / 2.0;
            }
            maxX = Math.Max(maxX, node.X + node.Width + 60);
            maxY = Math.Max(maxY, node.Y + NodeHeight + 48);
            return node.Y;
        }

        foreach (var root in _roots)
        {
            LayoutNode(root, CanvasPadX);
            cursorY += RowGap;
        }

        _canvas.Width = maxX;
        _canvas.Height = maxY;

        void DrawEdges(MindNode node)
        {
            if (node.IsCollapsed || node.Children.Count == 0) return;
            double startX = node.X + node.Width + ToggleSize + 6;
            double startY = node.Y + NodeHeight / 2.0;
            var stroke = EdgeBrush(node.Depth);

            foreach (var child in node.Children)
            {
                double endX = child.X;
                double endY = child.Y + NodeHeight / 2.0;
                double midX = (startX + endX) / 2.0;

                var fig = new PathFigure { StartPoint = new Point(startX, startY), IsClosed = false };
                fig.Segments.Add(new BezierSegment
                {
                    Point1 = new Point(midX, startY),
                    Point2 = new Point(midX, endY),
                    Point3 = new Point(endX, endY),
                });
                var geom = new PathGeometry();
                geom.Figures.Add(fig);
                var path = new Microsoft.UI.Xaml.Shapes.Path
                {
                    Data = geom,
                    Stroke = stroke,
                    StrokeThickness = 1.6,
                };
                _canvas.Children.Add(path);
                DrawEdges(child);
            }
        }

        void DrawNodes(MindNode node)
        {
            var (bg, border, fg) = NodeColors(node.Depth);
            var card = new Border
            {
                Width = node.Width,
                Height = NodeHeight,
                CornerRadius = new CornerRadius(8),
                Background = new SolidColorBrush(bg),
                BorderBrush = new SolidColorBrush(border),
                BorderThickness = new Thickness(1),
                Padding = new Thickness(12, 0, 12, 0),
                Child = new TextBlock
                {
                    Text = node.Label,
                    Foreground = new SolidColorBrush(fg),
                    FontSize = 12.5,
                    FontWeight = node.Depth == 0 ? FontWeights.SemiBold : FontWeights.Normal,
                    VerticalAlignment = VerticalAlignment.Center,
                    TextTrimming = TextTrimming.CharacterEllipsis,
                    IsTextSelectionEnabled = true,
                },
            };
            ToolTipService.SetToolTip(card, node.Label);
            Canvas.SetLeft(card, node.X);
            Canvas.SetTop(card, node.Y);
            _canvas.Children.Add(card);

            if (node.Children.Count > 0)
            {
                var target = node;
                var toggleBtn = new Button
                {
                    Width = ToggleSize,
                    Height = ToggleSize,
                    MinWidth = ToggleSize,
                    MinHeight = ToggleSize,
                    Padding = new Thickness(0),
                    CornerRadius = new CornerRadius(ToggleSize / 2.0),
                    Background = new SolidColorBrush(Color.FromArgb(255, 52, 56, 68)),
                    BorderBrush = new SolidColorBrush(Color.FromArgb(255, 86, 92, 112)),
                    BorderThickness = new Thickness(1),
                    HorizontalContentAlignment = HorizontalAlignment.Center,
                    VerticalContentAlignment = VerticalAlignment.Center,
                    Content = new TextBlock
                    {
                        Text = target.IsCollapsed ? ">" : "<",
                        FontSize = 11,
                        FontWeight = FontWeights.SemiBold,
                        Foreground = new SolidColorBrush(Color.FromArgb(255, 224, 228, 238)),
                        VerticalAlignment = VerticalAlignment.Center,
                        HorizontalAlignment = HorizontalAlignment.Center,
                    },
                };
                toggleBtn.Click += (_, _) =>
                {
                    target.IsCollapsed = !target.IsCollapsed;
                    Redraw();
                };
                Canvas.SetLeft(toggleBtn, node.X + node.Width + 6);
                Canvas.SetTop(toggleBtn, node.Y + (NodeHeight - ToggleSize) / 2.0);
                _canvas.Children.Add(toggleBtn);
            }

            if (!node.IsCollapsed)
            {
                foreach (var child in node.Children)
                    DrawNodes(child);
            }
        }

        foreach (var root in _roots) DrawEdges(root);
        foreach (var root in _roots) DrawNodes(root);
    }

    private FrameworkElement BuildFloatingControls()
    {
        static Geometry Geom(string d) =>
            (Geometry)Microsoft.UI.Xaml.Markup.XamlBindingHelper.ConvertValue(typeof(Geometry), d);

        Button MakeRoundBtn(string pathData, string tip, RoutedEventHandler onClick)
        {
            var btn = new Button
            {
                Width = 36,
                Height = 36,
                MinWidth = 36,
                MinHeight = 36,
                Padding = new Thickness(0),
                CornerRadius = new CornerRadius(18),
                Background = new SolidColorBrush(Color.FromArgb(230, 38, 41, 48)),
                BorderBrush = new SolidColorBrush(Color.FromArgb(255, 66, 70, 82)),
                BorderThickness = new Thickness(1),
                HorizontalContentAlignment = HorizontalAlignment.Center,
                VerticalContentAlignment = VerticalAlignment.Center,
                Content = new Viewbox
                {
                    Width = 16,
                    Height = 16,
                    Child = new PathIcon
                    {
                        Width = 24,
                        Height = 24,
                        Data = Geom(pathData),
                        Foreground = new SolidColorBrush(Color.FromArgb(255, 227, 227, 227)),
                    },
                },
            };
            ToolTipService.SetToolTip(btn, tip);
            btn.Click += onClick;
            return btn;
        }

        var expandBtn = MakeRoundBtn(ExpandCollapseIconPath, L10n.T("mindmap.expand_collapse_all"), (_, _) =>
        {
            _allCollapsed = !_allCollapsed;
            void Walk(MindNode n)
            {
                if (n.Depth > 0 && n.Children.Count > 0) n.IsCollapsed = _allCollapsed;
                foreach (var c in n.Children) Walk(c);
            }
            foreach (var r in _roots) Walk(r);
            Redraw();
        });

        var zoomInBtn = MakeRoundBtn(PlusIconPath, L10n.T("mindmap.zoom_in"), (_, _) =>
        {
            var next = Math.Min(2.2, _scale.ScaleX + 0.15);
            _scale.ScaleX = _scale.ScaleY = next;
        });

        var zoomOutBtn = MakeRoundBtn(MinusIconPath, L10n.T("mindmap.zoom_out"), (_, _) =>
        {
            var next = Math.Max(0.45, _scale.ScaleX - 0.15);
            _scale.ScaleX = _scale.ScaleY = next;
        });

        var stack = new StackPanel
        {
            Spacing = 8,
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Top,
            Margin = new Thickness(14, 18, 0, 0),
        };
        stack.Children.Add(expandBtn);

        var zoomGroup = new Border
        {
            CornerRadius = new CornerRadius(18),
            Background = new SolidColorBrush(Color.FromArgb(230, 38, 41, 48)),
            BorderBrush = new SolidColorBrush(Color.FromArgb(255, 66, 70, 82)),
            BorderThickness = new Thickness(1),
            Padding = new Thickness(0, 2, 0, 2),
            Child = new StackPanel { Spacing = 2, Children = { zoomInBtn, zoomOutBtn } },
        };
        zoomInBtn.BorderThickness = new Thickness(0);
        zoomOutBtn.BorderThickness = new Thickness(0);
        zoomInBtn.Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent);
        zoomOutBtn.Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent);
        stack.Children.Add(zoomGroup);

        if (_onDownloadSvg != null)
        {
            stack.Children.Add(MakeRoundBtn(DownloadIconPath, L10n.T("mindmap.download"), (_, _) => _onDownloadSvg()));
        }

        return stack;
    }

    private static (Color Bg, Color Border, Color Fg) NodeColors(int depth) => depth switch
    {
        0 => (Color.FromArgb(255, 67, 70, 99), Color.FromArgb(255, 102, 108, 152), Color.FromArgb(255, 240, 242, 248)),
        1 => (Color.FromArgb(255, 43, 50, 69), Color.FromArgb(255, 68, 80, 108), Color.FromArgb(255, 227, 232, 242)),
        2 => (Color.FromArgb(255, 37, 55, 58), Color.FromArgb(255, 56, 84, 90), Color.FromArgb(255, 224, 238, 236)),
        _ => (Color.FromArgb(255, 39, 56, 46), Color.FromArgb(255, 60, 88, 72), Color.FromArgb(255, 224, 238, 228)),
    };

    private static Brush EdgeBrush(int depth) => new SolidColorBrush(depth switch
    {
        0 => Color.FromArgb(220, 132, 140, 210),
        1 => Color.FromArgb(220, 122, 166, 198),
        _ => Color.FromArgb(220, 115, 178, 156),
    });
}
