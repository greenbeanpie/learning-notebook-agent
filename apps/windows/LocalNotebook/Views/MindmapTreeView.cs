using LocalNotebook.Services;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using System.Text.RegularExpressions;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using System.Text.Json;
using Windows.Foundation;
using Windows.UI;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>
/// Interactive left-to-right collapsible mind map & Knowledge Graph tree diagram
/// with curved Bezier links, depth-coded / importance-badged rounded nodes,
/// per-branch expand/collapse buttons, left floating controls, and node-level
/// linkage to Quiz, Mind Map expansion, and Chat.
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
        public string Importance { get; init; } = "";
        public string Summary { get; init; } = "";
        public List<string> Prerequisites { get; } = new();
        public int Depth { get; set; }
        public bool IsCollapsed { get; set; }
        public List<MindNode> Children { get; } = new();
        public double X { get; set; }
        public double Y { get; set; }
        public double Width { get; set; }
        public double Height { get; set; } = 38;
    }

    public FrameworkElement Root { get; }

    private readonly List<MindNode> _roots = new();
    private readonly Canvas _canvas = new();
    private readonly ScaleTransform _scale = new() { ScaleX = 1.0, ScaleY = 1.0 };
    private readonly ScrollViewer _scroll;
    private readonly bool _compact;
    private readonly bool _isKnowledgeGraph;
    private readonly Action? _onDownloadSvg;
    private readonly Action<string>? _onQuizConcept;
    private readonly Action<string>? _onExpandMindmap;
    private readonly Action<string>? _onAskConcept;
    private readonly Action<JsonElement>? _onProgressChanged;
    private readonly Dictionary<string, (string Status, double Score, int Attempts, int Correct)> _mastery = new(StringComparer.Ordinal);
    private readonly List<MindNode> _allNodes = new();
    private TextBlock? _kgProgressText;
    private ProgressBar? _kgProgressBar;
    private bool _allCollapsed;
    private bool _isPanning;
    private Point _panStartPoint;
    private double _panStartH;
    private double _panStartV;

    private const double NodeHeight = 38;
    private const double RowGap = 14;
    private const double ColGap = 68;
    private const double ToggleSize = 22;
    private const double CanvasPadX = 72;
    private const double CanvasPadY = 38;

    public MindmapTreeView(
        JsonElement payload,
        Action? onDownloadSvg,
        bool compact,
        FrameworkElement? feedbackBar = null,
        bool isKnowledgeGraph = false,
        Action<string>? onQuizConcept = null,
        Action<string>? onExpandMindmap = null,
        Action<string>? onAskConcept = null,
        JsonElement? progressState = null,
        Action<JsonElement>? onProgressChanged = null)
    {
        _compact = compact;
        _isKnowledgeGraph = isKnowledgeGraph || Str(payload, "kind") == "knowledge_graph";
        _onDownloadSvg = onDownloadSvg;
        _onQuizConcept = onQuizConcept;
        _onExpandMindmap = onExpandMindmap;
        _onAskConcept = onAskConcept;
        _onProgressChanged = onProgressChanged;
        LoadMasteryFromState(progressState);

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
        AttachDragPan(_scroll);
        if (_compact) NestedScrollHelper.AttachToOuterOnBoundary(_scroll);

        var controls = BuildFloatingControls();
        var stageGrid = new Grid { Children = { _scroll, controls } };

        if (_isKnowledgeGraph)
        {
            _kgProgressText = new TextBlock
            {
                FontSize = 11.5,
                FontFamily = new FontFamily("Microsoft YaHei UI, Segoe UI"),
                Foreground = new SolidColorBrush(Color.FromArgb(255, 224, 236, 255)),
            };
            _kgProgressBar = new ProgressBar
            {
                Minimum = 0,
                Maximum = 100,
                Height = 5,
                CornerRadius = new CornerRadius(3),
                Foreground = new SolidColorBrush(Color.FromArgb(255, 129, 201, 149)),
                Background = new SolidColorBrush(Color.FromArgb(120, 62, 74, 92)),
            };
            var progressStack = new StackPanel { Spacing = 5 };
            progressStack.Children.Add(_kgProgressText);
            progressStack.Children.Add(_kgProgressBar);
            var progressPill = new Border
            {
                HorizontalAlignment = HorizontalAlignment.Left,
                VerticalAlignment = VerticalAlignment.Top,
                Margin = new Thickness(16, 14, 0, 0),
                Padding = new Thickness(12, 8, 12, 8),
                CornerRadius = new CornerRadius(10),
                Background = new SolidColorBrush(Color.FromArgb(230, 30, 42, 56)),
                BorderBrush = new SolidColorBrush(Color.FromArgb(255, 80, 122, 166)),
                BorderThickness = new Thickness(1),
                Child = progressStack,
            };
            stageGrid.Children.Add(progressPill);

            var badge = new Border
            {
                HorizontalAlignment = HorizontalAlignment.Right,
                VerticalAlignment = VerticalAlignment.Top,
                Margin = new Thickness(0, 14, 16, 0),
                Padding = new Thickness(10, 4, 10, 4),
                CornerRadius = new CornerRadius(12),
                Background = new SolidColorBrush(Color.FromArgb(225, 36, 56, 72)),
                BorderBrush = new SolidColorBrush(Color.FromArgb(255, 92, 146, 196)),
                BorderThickness = new Thickness(1),
                Child = new TextBlock
                {
                    Text = L10n.T("kg.badge_hint"),
                    FontSize = 11.5,
                    FontFamily = new FontFamily("Microsoft YaHei UI, Segoe UI"),
                    Foreground = new SolidColorBrush(Color.FromArgb(255, 196, 224, 250)),
                },
            };
            stageGrid.Children.Add(badge);
            UpdateKgProgressBanner();
        }

        var stage = new Border
        {
            CornerRadius = new CornerRadius(12),
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            Background = new SolidColorBrush(Color.FromArgb(255, 29, 31, 36)),
            Child = stageGrid,
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

    private static string StripCitationMarkers(string text)
    {
        if (string.IsNullOrEmpty(text)) return "";
        var cleaned = Regex.Replace(text, @"\s*(?:\[\s*\d+(?:\s*[,，\-–—、]\s*\d+)*\s*\]|【\s*\d+(?:\s*[,，\-–—、]\s*\d+)*\s*】)+", "");
        return Regex.Replace(cleaned, @"\s{2,}", " ").Trim();
    }

    private void AttachDragPan(ScrollViewer scroll)
    {
        scroll.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler((_, e) =>
        {
            var pt = e.GetCurrentPoint(scroll);
            if (!pt.Properties.IsLeftButtonPressed) return;
            _isPanning = true;
            _panStartPoint = pt.Position;
            _panStartH = scroll.HorizontalOffset;
            _panStartV = scroll.VerticalOffset;
        }), handledEventsToo: true);

        scroll.AddHandler(UIElement.PointerMovedEvent, new PointerEventHandler((_, e) =>
        {
            if (!_isPanning) return;
            var pt = e.GetCurrentPoint(scroll);
            if (!pt.Properties.IsLeftButtonPressed)
            {
                _isPanning = false;
                return;
            }
            var dx = pt.Position.X - _panStartPoint.X;
            var dy = pt.Position.Y - _panStartPoint.Y;
            if (Math.Abs(dx) > 4 || Math.Abs(dy) > 4)
            {
                var targetH = Math.Max(0, _panStartH - dx);
                var targetV = Math.Max(0, _panStartV - dy);
                scroll.ChangeView(targetH, targetV, null, disableAnimation: true);
            }
        }), handledEventsToo: true);

        scroll.AddHandler(UIElement.PointerReleasedEvent, new PointerEventHandler((_, _) => _isPanning = false), handledEventsToo: true);
        scroll.AddHandler(UIElement.PointerCanceledEvent, new PointerEventHandler((_, _) => _isPanning = false), handledEventsToo: true);
        scroll.AddHandler(UIElement.PointerExitedEvent, new PointerEventHandler((_, e) =>
        {
            if (!e.GetCurrentPoint(scroll).Properties.IsLeftButtonPressed) _isPanning = false;
        }), handledEventsToo: true);
    }

    private void LoadMasteryFromState(JsonElement? progressState)
    {
        _mastery.Clear();
        if (progressState is not { ValueKind: JsonValueKind.Object } ps) return;
        if (!ps.TryGetProperty("mastery", out var m) || m.ValueKind != JsonValueKind.Object) return;
        foreach (var prop in m.EnumerateObject())
        {
            if (prop.Value.ValueKind != JsonValueKind.Object) continue;
            var status = prop.Value.TryGetProperty("status", out var st) && st.ValueKind == JsonValueKind.String
                ? (st.GetString() ?? "unstarted")
                : "unstarted";
            var score = prop.Value.TryGetProperty("score", out var sc) && sc.ValueKind == JsonValueKind.Number
                ? sc.GetDouble()
                : 0.0;
            var attempts = prop.Value.TryGetProperty("attempts", out var at) && at.ValueKind == JsonValueKind.Number
                ? at.GetInt32()
                : 0;
            var correct = prop.Value.TryGetProperty("correct", out var co) && co.ValueKind == JsonValueKind.Number
                ? co.GetInt32()
                : 0;
            _mastery[prop.Name] = (status, score, attempts, correct);
        }
    }

    private void SetNodeMasteryStatus(string nodeId, string status)
    {
        var prev = _mastery.TryGetValue(nodeId, out var existing)
            ? existing
            : (Status: "unstarted", Score: 0.0, Attempts: 0, Correct: 0);
        var score = status switch
        {
            "mastered" => 1.0,
            "learning" => 0.65,
            "weak" => 0.25,
            _ => 0.0,
        };
        _mastery[nodeId] = (status, score, prev.Attempts, prev.Correct);
        UpdateKgProgressBanner();
        Redraw();
        _onProgressChanged?.Invoke(BuildProgressJson());
    }

    private JsonElement BuildProgressJson()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        var dict = new System.Text.Json.Nodes.JsonObject();
        foreach (var kv in _mastery)
        {
            dict[kv.Key] = new System.Text.Json.Nodes.JsonObject
            {
                ["status"] = kv.Value.Status,
                ["score"] = kv.Value.Score,
                ["attempts"] = kv.Value.Attempts,
                ["correct"] = kv.Value.Correct,
                ["last_updated"] = now,
            };
        }
        var root = new System.Text.Json.Nodes.JsonObject
        {
            ["mastery"] = dict,
            ["updated_at"] = now,
        };
        using var doc = JsonDocument.Parse(root.ToJsonString());
        return doc.RootElement.Clone();
    }

    private void UpdateKgProgressBanner()
    {
        if (!_isKnowledgeGraph || _kgProgressText == null || _kgProgressBar == null) return;
        var concepts = _allNodes.Count > 1 ? _allNodes.Where(n => n.Depth > 0).ToList() : _allNodes;
        var total = concepts.Count;
        var mastered = 0;
        var learning = 0;
        var weak = 0;
        foreach (var c in concepts)
        {
            var st = _mastery.TryGetValue(c.Id, out var m) ? m.Status : "unstarted";
            switch (st)
            {
                case "mastered": mastered++; break;
                case "learning": learning++; break;
                case "weak": weak++; break;
            }
        }
        var pct = total == 0 ? 0 : (mastered * 100) / total;
        _kgProgressText.Text = L10n.Fmt("kg.progress_detail_fmt", mastered, total, pct, learning, weak);
        _kgProgressBar.Value = pct;
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
            var label = StripCitationMarkers(Str(n, "label"));
            var imp = Str(n, "importance");
            var sum = Str(n, "summary");
            var node = new MindNode
            {
                Id = id,
                Label = label,
                Importance = imp,
                Summary = sum,
                Width = EstimateWidth(label, imp.Length > 0),
            };
            foreach (var pr in Arr(n, "prerequisites"))
            {
                if (pr.ValueKind == JsonValueKind.String && !string.IsNullOrWhiteSpace(pr.GetString()))
                    node.Prerequisites.Add(pr.GetString()!);
            }
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
        _allNodes.Clear();
        _allNodes.AddRange(order);
    }

    private static void AssignDepth(MindNode node, int depth)
    {
        node.Depth = depth;
        foreach (var child in node.Children)
            AssignDepth(child, depth + 1);
    }

    private static double EstimateWidth(string label, bool hasDot)
    {
        double units = 0;
        foreach (var ch in label)
            units += ch > 127 ? 14.0 : 7.8;
        return Math.Clamp(units + (hasDot ? 42 : 28), 104, 290);
    }

    private void Redraw()
    {
        _canvas.Children.Clear();

        double cursorY = CanvasPadY;
        double maxX = 420;
        double maxY = 320;

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
            maxX = Math.Max(maxX, node.X + node.Width + 68);
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
            var stroke = EdgeBrush(node.Depth, _isKnowledgeGraph);

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
            var (bg, border, fg) = NodeColors(node.Depth, _isKnowledgeGraph);
            var masteryStatus = _isKnowledgeGraph && node.Depth > 0 && _mastery.TryGetValue(node.Id, out var ms)
                ? ms.Status
                : "unstarted";
            if (_isKnowledgeGraph && node.Depth > 0)
            {
                border = masteryStatus switch
                {
                    "mastered" => Color.FromArgb(255, 110, 198, 134),
                    "learning" => Color.FromArgb(255, 118, 174, 246),
                    "weak" => Color.FromArgb(255, 238, 124, 114),
                    _ => border,
                };
            }
            var contentRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 7, VerticalAlignment = VerticalAlignment.Center };
            if (_isKnowledgeGraph && node.Depth > 0)
            {
                var statusColor = masteryStatus switch
                {
                    "mastered" => Color.FromArgb(255, 129, 201, 149),
                    "learning" => Color.FromArgb(255, 138, 180, 248),
                    "weak" => Color.FromArgb(255, 242, 139, 130),
                    _ => Color.FromArgb(255, 110, 118, 129),
                };
                contentRow.Children.Add(new Border
                {
                    Width = 8,
                    Height = 8,
                    CornerRadius = new CornerRadius(4),
                    Background = new SolidColorBrush(statusColor),
                    VerticalAlignment = VerticalAlignment.Center,
                });
            }
            else if (node.Importance.Length > 0)
            {
                var dotColor = node.Importance switch
                {
                    "core" => Color.FromArgb(255, 253, 214, 99),
                    "key" => Color.FromArgb(255, 138, 180, 248),
                    _ => Color.FromArgb(255, 129, 201, 149),
                };
                contentRow.Children.Add(new Border
                {
                    Width = 7,
                    Height = 7,
                    CornerRadius = new CornerRadius(3.5),
                    Background = new SolidColorBrush(dotColor),
                    VerticalAlignment = VerticalAlignment.Center,
                });
            }
            contentRow.Children.Add(new TextBlock
            {
                Text = node.Label,
                Foreground = new SolidColorBrush(fg),
                FontFamily = new FontFamily("Microsoft YaHei UI, Segoe UI"),
                FontSize = 13,
                FontWeight = node.Depth == 0 ? FontWeights.SemiBold : FontWeights.Normal,
                VerticalAlignment = VerticalAlignment.Center,
                TextTrimming = TextTrimming.CharacterEllipsis,
            });

            var card = new Button
            {
                Width = node.Width,
                Height = NodeHeight,
                MinHeight = NodeHeight,
                CornerRadius = new CornerRadius(8),
                Background = new SolidColorBrush(bg),
                BorderBrush = new SolidColorBrush(border),
                BorderThickness = new Thickness(1),
                Padding = new Thickness(11, 0, 11, 0),
                HorizontalContentAlignment = HorizontalAlignment.Left,
                VerticalContentAlignment = VerticalAlignment.Center,
                Content = contentRow,
            };

            var tipParts = new List<string> { node.Label };
            if (_isKnowledgeGraph && node.Depth > 0)
            {
                var statusLabel = masteryStatus switch
                {
                    "mastered" => L10n.T("kg.status_mastered"),
                    "learning" => L10n.T("kg.status_learning"),
                    "weak" => L10n.T("kg.status_weak"),
                    _ => L10n.T("kg.status_unstarted"),
                };
                tipParts.Add($"{L10n.T("kg.status_label")}: {statusLabel}");
            }
            if (node.Summary.Length > 0) tipParts.Add(node.Summary);
            if (node.Prerequisites.Count > 0) tipParts.Add($"{L10n.T("kg.prereq")}: {string.Join(", ", node.Prerequisites)}");
            ToolTipService.SetToolTip(card, string.Join("\n", tipParts));

            if (_onQuizConcept != null || _onExpandMindmap != null || _onAskConcept != null || (_isKnowledgeGraph && node.Depth > 0))
            {
                var cleanConcept = node.Label.Split('[')[0].Trim();
                var menu = new MenuFlyout();
                if (_onQuizConcept != null)
                {
                    var mi = new MenuFlyoutItem { Text = L10n.T("kg.action_quiz") };
                    mi.Click += (_, _) => _onQuizConcept(cleanConcept);
                    menu.Items.Add(mi);
                }
                if (_onExpandMindmap != null)
                {
                    var mi = new MenuFlyoutItem { Text = L10n.T("kg.action_mindmap") };
                    mi.Click += (_, _) => _onExpandMindmap(cleanConcept);
                    menu.Items.Add(mi);
                }
                if (_onAskConcept != null)
                {
                    var mi = new MenuFlyoutItem { Text = L10n.T("kg.action_chat") };
                    mi.Click += (_, _) => _onAskConcept(cleanConcept);
                    menu.Items.Add(mi);
                }
                if (_isKnowledgeGraph && node.Depth > 0)
                {
                    if (menu.Items.Count > 0) menu.Items.Add(new MenuFlyoutSeparator());
                    foreach (var (labelKey, tag) in new[]
                    {
                        ("kg.mark_mastered", "mastered"),
                        ("kg.mark_learning", "learning"),
                        ("kg.mark_weak", "weak"),
                        ("kg.mark_unstarted", "unstarted"),
                    })
                    {
                        var mi = new MenuFlyoutItem { Text = L10n.T(labelKey) };
                        var nodeId = node.Id;
                        var newStatus = tag;
                        mi.Click += (_, _) => SetNodeMasteryStatus(nodeId, newStatus);
                        menu.Items.Add(mi);
                    }
                }
                card.Flyout = menu;
            }

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

    private static (Color Bg, Color Border, Color Fg) NodeColors(int depth, bool isKg) => isKg
        ? depth switch
        {
            0 => (Color.FromArgb(255, 42, 62, 88), Color.FromArgb(255, 92, 138, 196), Color.FromArgb(255, 240, 246, 255)),
            1 => (Color.FromArgb(255, 36, 54, 68), Color.FromArgb(255, 68, 108, 138), Color.FromArgb(255, 228, 240, 248)),
            2 => (Color.FromArgb(255, 35, 58, 56), Color.FromArgb(255, 62, 106, 100), Color.FromArgb(255, 224, 242, 238)),
            _ => (Color.FromArgb(255, 42, 56, 46), Color.FromArgb(255, 72, 102, 82), Color.FromArgb(255, 226, 240, 230)),
        }
        : depth switch
        {
            0 => (Color.FromArgb(255, 67, 70, 99), Color.FromArgb(255, 102, 108, 152), Color.FromArgb(255, 240, 242, 248)),
            1 => (Color.FromArgb(255, 43, 50, 69), Color.FromArgb(255, 68, 80, 108), Color.FromArgb(255, 227, 232, 242)),
            2 => (Color.FromArgb(255, 37, 55, 58), Color.FromArgb(255, 56, 84, 90), Color.FromArgb(255, 224, 238, 236)),
            _ => (Color.FromArgb(255, 39, 56, 46), Color.FromArgb(255, 60, 88, 72), Color.FromArgb(255, 224, 238, 228)),
        };

    private static Brush EdgeBrush(int depth, bool isKg) => new SolidColorBrush(isKg
        ? depth switch
        {
            0 => Color.FromArgb(220, 114, 168, 224),
            1 => Color.FromArgb(220, 104, 186, 182),
            _ => Color.FromArgb(220, 122, 190, 148),
        }
        : depth switch
        {
            0 => Color.FromArgb(220, 132, 140, 210),
            1 => Color.FromArgb(220, 122, 166, 198),
            _ => Color.FromArgb(220, 115, 178, 156),
        });
}
