using LocalNotebook.Services;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
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
        public List<string> Facets { get; } = new();
        public bool Atomic { get; init; }
        public bool Expandable { get; init; }
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
    private readonly CompositeTransform _viewTransform = new() { ScaleX = 1.0, ScaleY = 1.0, TranslateX = 0, TranslateY = 0 };
    private readonly Grid _viewport = new();
    private readonly bool _compact;
    private readonly bool _readOnly;
    private readonly bool _isKnowledgeGraph;
    private readonly Action? _onDownloadSvg;
    private readonly Action<string>? _onQuizConcept;
    private readonly Action<string>? _onExpandMindmap;
    private readonly Action<string>? _onAskConcept;
    private readonly Action<JsonElement>? _onProgressChanged;
    private readonly Func<string, string, Task>? _onRefineNode;
    private readonly Dictionary<string, (string Status, double Score, int Attempts, int Correct)> _mastery = new(StringComparer.Ordinal);
    private readonly List<MindNode> _allNodes = new();
    private TextBlock? _kgProgressText;
    private ProgressBar? _kgProgressBar;
    private bool _allCollapsed = true;
    private bool _isPanning;
    private Point _panStartPoint;
    private double _panStartTx;
    private double _panStartTy;

    private const double MinZoom = 0.25;
    private const double MaxZoom = 3.0;

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
        Action<JsonElement>? onProgressChanged = null,
        Func<string, string, Task>? onRefineNode = null,
        bool readOnly = false)
    {
        _readOnly = readOnly;
        _onRefineNode = onRefineNode;
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
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Top,
            RenderTransform = _viewTransform,
            RenderTransformOrigin = new Point(0, 0),
            Children = { _canvas },
        };

        _viewport.Background = new SolidColorBrush(Color.FromArgb(255, 29, 31, 36));
        _viewport.Children.Add(canvasHost);
        _viewport.SizeChanged += (_, e) =>
        {
            _viewport.Clip = new RectangleGeometry
            {
                Rect = new Windows.Foundation.Rect(0, 0, e.NewSize.Width, e.NewSize.Height),
            };
            FitOnFirstLayout(e.NewSize);
            UpdateRealization(force: true);
        };
        AttachDragPan(_viewport);
        AttachWheelZoom(_viewport);

        var controls = BuildFloatingControls();
        var stageGrid = new Grid { Children = { _viewport, controls } };

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
            UpdateKgProgressBanner();
        }

        if (_allNodes.Count > 400)
        {
            stageGrid.Children.Add(new Border
            {
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Bottom,
                Margin = new Thickness(0, 0, 0, 12),
                Padding = new Thickness(10, 5, 10, 5),
                CornerRadius = new CornerRadius(8),
                IsHitTestVisible = false,
                Background = new SolidColorBrush(Color.FromArgb(220, 30, 42, 56)),
                Child = new TextBlock
                {
                    Text = L10n.Fmt("mindmap.large_hint_fmt", _allNodes.Count),
                    FontSize = 11.5,
                    FontFamily = new FontFamily("Microsoft YaHei UI, Segoe UI"),
                    Foreground = new SolidColorBrush(Color.FromArgb(255, 224, 236, 255)),
                },
            });
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

    /// <summary>
    /// Replaces the node data with a newer payload (streaming preview) while keeping the user's
    /// expand/collapse state, selection and pan/zoom. Returns false when this view cannot be updated in place.
    /// </summary>
    public bool TryUpdate(JsonElement payload)
    {
        if (_viewport.Children.Count == 0) return false;
        var rawNodes = Arr(payload, "nodes").ToList();
        if (rawNodes.Count == 0) return false;

        // id -> collapsed, only for nodes that already had children (otherwise the user could not have toggled them)
        var prior = new Dictionary<string, bool>(StringComparer.Ordinal);
        foreach (var n in _allNodes)
            if (n.Children.Count > 0) prior[n.Id] = n.IsCollapsed;
        var selectedId = _selected?.Id;

        _roots.Clear();
        BuildHierarchy(rawNodes);
        foreach (var n in _allNodes)
            if (n.Children.Count > 0 && prior.TryGetValue(n.Id, out var was)) n.IsCollapsed = was;

        _selected = null;
        _prereqNodes.Clear();
        if (selectedId != null && _byId.TryGetValue(selectedId, out var sel))
        {
            _selected = sel;
            foreach (var name in sel.Prerequisites)
                if (_byLabel.TryGetValue(StripCitationMarkers(name), out var pn) && pn != sel) _prereqNodes.Add(pn);
        }
        UpdateKgProgressBanner();
        Redraw();
        return true;
    }

    private static string StripCitationMarkers(string text)
    {
        if (string.IsNullOrEmpty(text)) return "";
        var cleaned = Regex.Replace(text, @"\s*(?:\[\s*\d+(?:\s*[,，\-–—、]\s*\d+)*\s*\]|【\s*\d+(?:\s*[,，\-–—、]\s*\d+)*\s*】)+", "");
        return Regex.Replace(cleaned, @"\s{2,}", " ").Trim();
    }

    private void AttachDragPan(UIElement viewport)
    {
        viewport.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler((_, e) =>
        {
            var pt = e.GetCurrentPoint(viewport);
            if (!pt.Properties.IsLeftButtonPressed) return;
            _isPanning = true;
            _didPan = false;
            _panStartPoint = pt.Position;
            _panStartTx = _viewTransform.TranslateX;
            _panStartTy = _viewTransform.TranslateY;
        }), handledEventsToo: true);

        viewport.AddHandler(UIElement.PointerMovedEvent, new PointerEventHandler((_, e) =>
        {
            if (!_isPanning) return;
            var pt = e.GetCurrentPoint(viewport);
            if (!pt.Properties.IsLeftButtonPressed)
            {
                _isPanning = false;
                return;
            }
            var dx = pt.Position.X - _panStartPoint.X;
            var dy = pt.Position.Y - _panStartPoint.Y;
            if (Math.Abs(dx) > 4 || Math.Abs(dy) > 4)
            {
                _didPan = true;
                _viewTransform.TranslateX = _panStartTx + dx;
                _viewTransform.TranslateY = _panStartTy + dy;
                UpdateRealization();
            }
        }), handledEventsToo: true);

        viewport.AddHandler(UIElement.PointerReleasedEvent, new PointerEventHandler((_, _) =>
        {
            _isPanning = false;
            if (_didPan) UpdateRealization(force: true);
        }), handledEventsToo: true);
        viewport.AddHandler(UIElement.PointerCanceledEvent, new PointerEventHandler((_, _) => _isPanning = false), handledEventsToo: true);
        viewport.AddHandler(UIElement.PointerExitedEvent, new PointerEventHandler((_, e) =>
        {
            if (!e.GetCurrentPoint(viewport).Properties.IsLeftButtonPressed) _isPanning = false;
        }), handledEventsToo: true);
    }

    private bool _fitted;

    /// <summary>
    /// On first display, shrink a map that is wider than the viewport so it is not cut off at the
    /// right edge, and centre it vertically when it is shorter than the viewport.
    /// </summary>
    private void FitOnFirstLayout(Windows.Foundation.Size vp)
    {
        if (_fitted || vp.Width < 50 || vp.Height < 50 || _canvas.Width <= 0) return;
        _fitted = true;
        var scale = Math.Clamp(vp.Width / _canvas.Width, 0.5, 1.0);
        _viewTransform.ScaleX = scale;
        _viewTransform.ScaleY = scale;
        _viewTransform.TranslateX = 0;
        _viewTransform.TranslateY = Math.Max(0, (vp.Height - _canvas.Height * scale) / 2.0);
    }

    private void ZoomAt(double targetScale, Point anchorInViewport)
    {
        var oldScale = _viewTransform.ScaleX;
        var newScale = Math.Clamp(targetScale, MinZoom, MaxZoom);
        if (Math.Abs(newScale - oldScale) < 0.0001) return;

        var worldX = (anchorInViewport.X - _viewTransform.TranslateX) / oldScale;
        var worldY = (anchorInViewport.Y - _viewTransform.TranslateY) / oldScale;

        _viewTransform.ScaleX = newScale;
        _viewTransform.ScaleY = newScale;
        _viewTransform.TranslateX = anchorInViewport.X - worldX * newScale;
        _viewTransform.TranslateY = anchorInViewport.Y - worldY * newScale;
        UpdateRealization();
    }

    private void AttachWheelZoom(UIElement viewport)
    {
        viewport.AddHandler(UIElement.PointerWheelChangedEvent, new PointerEventHandler((_, e) =>
        {
            var pt = e.GetCurrentPoint(viewport);
            var delta = pt.Properties.MouseWheelDelta;
            if (delta == 0) return;

            e.Handled = true;
            var factor = Math.Pow(1.12, delta / 120.0);
            ZoomAt(_viewTransform.ScaleX * factor, pt.Position);
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
        if (_byId.TryGetValue(nodeId, out var changed)) RecolorNode(changed);
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
            var facets = Arr(n, "facets")
                .Where(f => f.ValueKind == JsonValueKind.String && !string.IsNullOrWhiteSpace(f.GetString()))
                .Select(f => f.GetString()!)
                .ToList();
            var node = new MindNode
            {
                Id = id,
                Label = label,
                Importance = imp,
                Summary = sum,
                Atomic = n.TryGetProperty("atomic", out var at) && at.ValueKind == JsonValueKind.True,
                Expandable = n.TryGetProperty("expandable", out var ex) && ex.ValueKind == JsonValueKind.True,
                Width = EstimateWidth(label, imp.Length > 0) + (facets.Count > 0 ? 26 : 0),
            };
            node.Facets.AddRange(facets);
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

        _byId.Clear();
        _byLabel.Clear();
        foreach (var n in order)
        {
            _byId[n.Id] = n;
            _byLabel.TryAdd(n.Label, n);
            // Room for the "+" marker.
            if (n.Children.Count > 0 || n.Expandable) n.Width = Math.Min(n.Width + 16, 320);
            // Default view: only the first two levels (root + its children); deeper branches start collapsed.
            if (n.Depth >= 1 && n.Children.Count > 0) n.IsCollapsed = true;
        }
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

    private sealed record EdgeGroup(MindNode Parent, Rect Bounds);

    private const double RealizeMargin = 600;
    private const double UnrealizeMargin = 1800;

    private readonly List<MindNode> _laidOut = new();
    private readonly List<EdgeGroup> _edgeGroups = new();
    private readonly Dictionary<MindNode, (Border Card, Run? Dot)> _cards = new();
    private readonly Dictionary<MindNode, (Border Toggle, TextBlock Glyph)> _toggles = new();
    private readonly Dictionary<MindNode, Microsoft.UI.Xaml.Shapes.Path> _edgePaths = new();
    private readonly Dictionary<string, MindNode> _byId = new(StringComparer.Ordinal);
    private readonly Dictionary<string, MindNode> _byLabel = new(StringComparer.Ordinal);
    private readonly List<Microsoft.UI.Xaml.Shapes.Path> _prereqLines = new();
    private readonly HashSet<MindNode> _prereqNodes = new();
    private MindNode? _selected;
    private bool _didPan;
    private double _realizedTx = double.NaN;
    private double _realizedTy = double.NaN;
    private double _realizedScale = double.NaN;

    private static readonly Color SelectedColor = Color.FromArgb(255, 253, 214, 99);
    private static readonly Color PrereqColor = Color.FromArgb(255, 240, 150, 70);

    private string MasteryOf(MindNode node) =>
        _isKnowledgeGraph && node.Depth > 0 && _mastery.TryGetValue(node.Id, out var ms) ? ms.Status : "unstarted";

    private static bool NeedsPlus(MindNode node) =>
        (node.Children.Count > 0 && node.IsCollapsed) || (node.Children.Count == 0 && node.Expandable);

    private Color BorderColorFor(MindNode node)
    {
        if (_selected == node) return SelectedColor;
        if (_prereqNodes.Contains(node)) return PrereqColor;
        var (_, border, _) = NodeColors(node.Depth, _isKnowledgeGraph);
        if (_isKnowledgeGraph && node.Depth > 0)
        {
            return MasteryOf(node) switch
            {
                "mastered" => Color.FromArgb(255, 110, 198, 134),
                "learning" => Color.FromArgb(255, 118, 174, 246),
                "weak" => Color.FromArgb(255, 238, 124, 114),
                _ => border,
            };
        }
        return border;
    }

    private Color DotColorFor(MindNode node)
    {
        if (_isKnowledgeGraph && node.Depth > 0)
        {
            return MasteryOf(node) switch
            {
                "mastered" => Color.FromArgb(255, 129, 201, 149),
                "learning" => Color.FromArgb(255, 138, 180, 248),
                "weak" => Color.FromArgb(255, 242, 139, 130),
                _ => Color.FromArgb(255, 110, 118, 129),
            };
        }
        return node.Importance switch
        {
            "core" => Color.FromArgb(255, 253, 214, 99),
            "key" => Color.FromArgb(255, 138, 180, 248),
            _ => Color.FromArgb(255, 129, 201, 149),
        };
    }

    private double BorderThicknessFor(MindNode node) => _selected == node || _prereqNodes.Contains(node) ? 2 : 1;

    /// <summary>Recolors only the card of one node (mastery change / highlight change); never rebuilds the graph.</summary>
    private void RecolorNode(MindNode node)
    {
        if (!_cards.TryGetValue(node, out var c)) return;
        c.Card.BorderBrush = new SolidColorBrush(BorderColorFor(node));
        c.Card.BorderThickness = new Thickness(BorderThicknessFor(node));
        if (c.Dot != null) c.Dot.Foreground = new SolidColorBrush(DotColorFor(node));
    }

    private void Redraw()
    {
        _canvas.Children.Clear();
        _cards.Clear();
        _toggles.Clear();
        _edgePaths.Clear();
        _prereqLines.Clear();
        _laidOut.Clear();
        _edgeGroups.Clear();
        _realizedTx = _realizedTy = _realizedScale = double.NaN;

        double cursorY = CanvasPadY;
        double maxX = 420;
        double maxY = 320;

        double LayoutNode(MindNode node, double x)
        {
            node.X = x;
            _laidOut.Add(node);
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

        foreach (var node in _laidOut)
        {
            if (node.IsCollapsed || node.Children.Count == 0) continue;
            var startX = node.X + node.Width + ToggleSize + 6;
            var top = Math.Min(node.Y, node.Children[0].Y);
            var bottom = Math.Max(node.Y, node.Children[^1].Y);
            _edgeGroups.Add(new EdgeGroup(node, new Rect(startX, top, Math.Max(1, node.Children[0].X - startX), bottom - top + NodeHeight)));
        }

        RebuildPrereqLines();
        UpdateRealization(force: true);
    }

    private Rect VisibleCanvasRect(double margin)
    {
        var vw = _viewport.ActualWidth > 0 ? _viewport.ActualWidth : 1600;
        var vh = _viewport.ActualHeight > 0 ? _viewport.ActualHeight : 1000;
        var s = Math.Max(0.01, _viewTransform.ScaleX);
        var x = -_viewTransform.TranslateX / s - margin;
        var y = -_viewTransform.TranslateY / s - margin;
        return new Rect(x, y, vw / s + margin * 2, vh / s + margin * 2);
    }

    private static bool Intersects(Rect a, double x, double y, double w, double h) =>
        x < a.X + a.Width && x + w > a.X && y < a.Y + a.Height && y + h > a.Y;

    /// <summary>
    /// Viewport culling: only cards/edges that intersect the visible area (plus a margin) exist as XAML elements.
    /// Elements far outside the viewport are released again.
    /// </summary>
    private void UpdateRealization(bool force = false)
    {
        if (!force
            && Math.Abs(_viewTransform.TranslateX - _realizedTx) < 150
            && Math.Abs(_viewTransform.TranslateY - _realizedTy) < 150
            && Math.Abs(_viewTransform.ScaleX - _realizedScale) < 0.0001)
            return;
        _realizedTx = _viewTransform.TranslateX;
        _realizedTy = _viewTransform.TranslateY;
        _realizedScale = _viewTransform.ScaleX;

        var near = VisibleCanvasRect(RealizeMargin);
        var far = VisibleCanvasRect(UnrealizeMargin);

        if (_cards.Count > 0 || _edgePaths.Count > 0)
        {
            List<MindNode>? dropCards = null;
            foreach (var kv in _cards)
            {
                var n = kv.Key;
                if (!Intersects(far, n.X, n.Y, n.Width + ToggleSize + 6, NodeHeight)) (dropCards ??= new()).Add(n);
            }
            if (dropCards != null)
            {
                foreach (var n in dropCards)
                {
                    _canvas.Children.Remove(_cards[n].Card);
                    _cards.Remove(n);
                    if (_toggles.Remove(n, out var t)) _canvas.Children.Remove(t.Toggle);
                }
            }
            List<MindNode>? dropEdges = null;
            foreach (var g in _edgeGroups)
            {
                if (_edgePaths.ContainsKey(g.Parent) && !Intersects(far, g.Bounds.X, g.Bounds.Y, g.Bounds.Width, g.Bounds.Height))
                    (dropEdges ??= new()).Add(g.Parent);
            }
            if (dropEdges != null)
            {
                foreach (var n in dropEdges)
                {
                    _canvas.Children.Remove(_edgePaths[n]);
                    _edgePaths.Remove(n);
                }
            }
        }

        foreach (var g in _edgeGroups)
        {
            if (_edgePaths.ContainsKey(g.Parent)) continue;
            if (!Intersects(near, g.Bounds.X, g.Bounds.Y, g.Bounds.Width, g.Bounds.Height)) continue;
            var path = BuildEdgePath(g.Parent);
            _edgePaths[g.Parent] = path;
            _canvas.Children.Insert(0, path);
        }

        foreach (var node in _laidOut)
        {
            if (_cards.ContainsKey(node)) continue;
            if (!Intersects(near, node.X, node.Y, node.Width + ToggleSize + 6, NodeHeight)) continue;
            RealizeNode(node);
        }
    }

    private Microsoft.UI.Xaml.Shapes.Path BuildEdgePath(MindNode node)
    {
        double startX = node.X + node.Width + ToggleSize + 6;
        double startY = node.Y + NodeHeight / 2.0;
        var geom = new PathGeometry();
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
            geom.Figures.Add(fig);
        }
        return new Microsoft.UI.Xaml.Shapes.Path
        {
            Data = geom,
            Stroke = EdgeBrush(node.Depth, _isKnowledgeGraph),
            StrokeThickness = 1.6,
            IsHitTestVisible = false,
        };
    }

    private void RealizeNode(MindNode node)
    {
        var (bg, _, fg) = NodeColors(node.Depth, _isKnowledgeGraph);
        var label = new TextBlock
        {
            Foreground = new SolidColorBrush(fg),
            FontFamily = new FontFamily("Microsoft YaHei UI, Segoe UI"),
            FontSize = 13,
            FontWeight = node.Depth == 0 ? FontWeights.SemiBold : FontWeights.Normal,
            VerticalAlignment = VerticalAlignment.Center,
            TextTrimming = TextTrimming.CharacterEllipsis,
            TextWrapping = TextWrapping.NoWrap,
        };
        Run? dot = null;
        if ((_isKnowledgeGraph && node.Depth > 0) || node.Importance.Length > 0)
        {
            dot = new Run { Text = "● ", FontSize = 10, Foreground = new SolidColorBrush(DotColorFor(node)) };
            label.Inlines.Add(dot);
        }
        label.Inlines.Add(new Run { Text = node.Label });
        if (node.Facets.Count > 0)
        {
            label.Inlines.Add(new Run
            {
                Text = " · " + node.Facets.Count,
                FontSize = 10.5,
                Foreground = new SolidColorBrush(Color.FromArgb(255, 170, 182, 204)),
            });
        }
        if (NeedsPlus(node))
        {
            label.Inlines.Add(new Run
            {
                Text = " +",
                FontWeight = FontWeights.Bold,
                Foreground = new SolidColorBrush(SelectedColor),
            });
        }

        var card = new Border
        {
            Width = node.Width,
            Height = NodeHeight,
            CornerRadius = new CornerRadius(8),
            Background = new SolidColorBrush(bg),
            BorderBrush = new SolidColorBrush(BorderColorFor(node)),
            BorderThickness = new Thickness(BorderThicknessFor(node)),
            Padding = new Thickness(11, 0, 11, 0),
            Child = label,
        };
        // ToolTip and menu are created lazily (hover / right-click / tap), not per card at creation.
        card.PointerEntered += (_, _) => ToolTipService.SetToolTip(card, BuildTip(node));
        card.RightTapped += (_, e) =>
        {
            e.Handled = true;
            SelectNode(node);
            ShowMenu(node, card);
        };
        card.Tapped += (_, e) =>
        {
            if (_didPan) return;
            e.Handled = true;
            SelectNode(node);
            if (node.Children.Count > 0 && node.IsCollapsed)
            {
                node.IsCollapsed = false;
                Redraw();
                return;
            }
            ShowMenu(node, card);
        };
        Canvas.SetLeft(card, node.X);
        Canvas.SetTop(card, node.Y);
        _canvas.Children.Add(card);
        _cards[node] = (card, dot);

        if (node.Children.Count > 0)
        {
            var glyph = new TextBlock
            {
                Text = node.IsCollapsed ? ">" : "<",
                FontSize = 11,
                FontWeight = FontWeights.SemiBold,
                Foreground = new SolidColorBrush(Color.FromArgb(255, 224, 228, 238)),
                VerticalAlignment = VerticalAlignment.Center,
                HorizontalAlignment = HorizontalAlignment.Center,
            };
            var toggle = new Border
            {
                Width = ToggleSize,
                Height = ToggleSize,
                CornerRadius = new CornerRadius(ToggleSize / 2.0),
                Background = new SolidColorBrush(Color.FromArgb(255, 52, 56, 68)),
                BorderBrush = new SolidColorBrush(Color.FromArgb(255, 86, 92, 112)),
                BorderThickness = new Thickness(1),
                Child = glyph,
            };
            toggle.Tapped += (_, e) =>
            {
                if (_didPan) return;
                e.Handled = true;
                node.IsCollapsed = !node.IsCollapsed;
                Redraw();
            };
            Canvas.SetLeft(toggle, node.X + node.Width + 6);
            Canvas.SetTop(toggle, node.Y + (NodeHeight - ToggleSize) / 2.0);
            _canvas.Children.Add(toggle);
            _toggles[node] = (toggle, glyph);
        }
    }

    private string BuildTip(MindNode node)
    {
        var tipParts = new List<string> { node.Label };
        if (_isKnowledgeGraph && node.Depth > 0)
        {
            var statusLabel = MasteryOf(node) switch
            {
                "mastered" => L10n.T("kg.status_mastered"),
                "learning" => L10n.T("kg.status_learning"),
                "weak" => L10n.T("kg.status_weak"),
                _ => L10n.T("kg.status_unstarted"),
            };
            tipParts.Add($"{L10n.T("kg.status_label")}: {statusLabel}");
        }
        if (node.Summary.Length > 0) tipParts.Add(node.Summary);
        if (node.Facets.Count > 0)
            tipParts.Add($"{L10n.T("kg.facets")}:\n" + string.Join("\n", node.Facets.Select(f => $"• {f}")));
        if (node.Prerequisites.Count > 0) tipParts.Add($"{L10n.T("kg.prereq")}: {string.Join(", ", node.Prerequisites)}");
        return string.Join("\n", tipParts);
    }

    private void SelectNode(MindNode node)
    {
        var previous = _selected;
        var oldPrereq = _prereqNodes.ToList();
        _selected = node;
        _prereqNodes.Clear();
        foreach (var name in node.Prerequisites)
        {
            if (_byLabel.TryGetValue(StripCitationMarkers(name), out var pn) && pn != node) _prereqNodes.Add(pn);
        }
        if (previous != null) RecolorNode(previous);
        foreach (var n in oldPrereq) RecolorNode(n);
        RecolorNode(node);
        foreach (var n in _prereqNodes) RecolorNode(n);
        RebuildPrereqLines();
    }

    /// <summary>Dashed prerequisite connectors, drawn only for the selected node.</summary>
    private void RebuildPrereqLines()
    {
        foreach (var l in _prereqLines) _canvas.Children.Remove(l);
        _prereqLines.Clear();
        if (_selected == null || _prereqNodes.Count == 0) return;
        var laid = new HashSet<MindNode>(_laidOut);
        if (!laid.Contains(_selected)) return;
        var brush = new SolidColorBrush(PrereqColor);
        foreach (var pre in _prereqNodes)
        {
            if (!laid.Contains(pre)) continue;
            var sx = pre.X + pre.Width;
            var sy = pre.Y + NodeHeight / 2.0;
            var ex = _selected.X;
            var ey = _selected.Y + NodeHeight / 2.0;
            if (ex < sx) { sx = pre.X; ex = _selected.X + _selected.Width; }
            var midX = (sx + ex) / 2.0;
            var fig = new PathFigure { StartPoint = new Point(sx, sy), IsClosed = false };
            fig.Segments.Add(new BezierSegment
            {
                Point1 = new Point(midX, sy - 30),
                Point2 = new Point(midX, ey - 30),
                Point3 = new Point(ex, ey),
            });
            var geom = new PathGeometry();
            geom.Figures.Add(fig);
            var line = new Microsoft.UI.Xaml.Shapes.Path
            {
                Data = geom,
                Stroke = brush,
                StrokeThickness = 1.6,
                StrokeDashArray = new DoubleCollection { 4, 3 },
                IsHitTestVisible = false,
            };
            _prereqLines.Add(line);
            _canvas.Children.Insert(0, line);
        }
    }

    private void ShowMenu(MindNode node, FrameworkElement anchor)
    {
        var menu = BuildMenu(node);
        if (menu != null) menu.ShowAt(anchor);
    }

    private MenuFlyout? BuildMenu(MindNode node)
    {
        if (_readOnly)
        {
            if (node.Children.Count == 0) return null;
            var ro = new MenuFlyout();
            var roItem = new MenuFlyoutItem { Text = node.IsCollapsed ? L10n.T("mindmap.menu_expand_local") : L10n.T("mindmap.menu_collapse_local") };
            roItem.Click += (_, _) =>
            {
                node.IsCollapsed = !node.IsCollapsed;
                Redraw();
            };
            ro.Items.Add(roItem);
            return ro;
        }
        if (!(_onQuizConcept != null || _onExpandMindmap != null || _onAskConcept != null || _onRefineNode != null || (_isKnowledgeGraph && node.Depth > 0)))
            return null;

        var cleanConcept = node.Label.Split('[')[0].Trim();
        var menu = new MenuFlyout();
        if (node.Children.Count > 0)
        {
            var toggleItem = new MenuFlyoutItem { Text = node.IsCollapsed ? L10n.T("mindmap.menu_expand_local") : L10n.T("mindmap.menu_collapse_local") };
            toggleItem.Click += (_, _) =>
            {
                node.IsCollapsed = !node.IsCollapsed;
                Redraw();
            };
            menu.Items.Add(toggleItem);
        }
        if (node.Facets.Count > 0)
        {
            var sub = new MenuFlyoutSubItem { Text = $"{L10n.T("kg.facets")} ({node.Facets.Count})" };
            foreach (var facet in node.Facets)
            {
                var fi = new MenuFlyoutItem { Text = facet };
                var target = $"{cleanConcept}：{facet}";
                if (_onAskConcept != null) fi.Click += (_, _) => _onAskConcept(target);
                else if (_onQuizConcept != null) fi.Click += (_, _) => _onQuizConcept(target);
                else fi.IsEnabled = false;
                sub.Items.Add(fi);
            }
            menu.Items.Add(sub);
        }
        if (_onRefineNode != null && node.Depth > 0)
        {
            var mi = new MenuFlyoutItem
            {
                Text = node.Atomic ? L10n.T("kg.atomic") : L10n.T("kg.action_refine"),
                IsEnabled = !node.Atomic,
            };
            var refineId = node.Id;
            var refineLabel = node.Label;
            mi.Click += async (_, _) => await _onRefineNode(refineId, refineLabel);
            menu.Items.Add(mi);
        }
        if (menu.Items.Count > 0 && (_onQuizConcept != null || _onExpandMindmap != null || _onAskConcept != null))
            menu.Items.Add(new MenuFlyoutSeparator());
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
        return menu;
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
            var center = new Point(_viewport.ActualWidth / 2.0, _viewport.ActualHeight / 2.0);
            ZoomAt(_viewTransform.ScaleX * 1.2, center);
        });

        var zoomOutBtn = MakeRoundBtn(MinusIconPath, L10n.T("mindmap.zoom_out"), (_, _) =>
        {
            var center = new Point(_viewport.ActualWidth / 2.0, _viewport.ActualHeight / 2.0);
            ZoomAt(_viewTransform.ScaleX / 1.2, center);
        });

        var stack = new StackPanel
        {
            Spacing = 8,
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Top,
            // Knowledge graphs show the progress pill top-left; keep the controls clear of it.
            Margin = new Thickness(14, _isKnowledgeGraph ? 74 : 18, 0, 0),
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
