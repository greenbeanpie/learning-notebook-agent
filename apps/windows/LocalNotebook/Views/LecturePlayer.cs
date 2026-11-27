using LocalNotebook.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using System.Text.Json;
using System.Text.Json.Nodes;
using Windows.Media.Core;
using Windows.Media.Playback;
using Windows.Media.SpeechSynthesis;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>
/// Narrated lecture ("讲解") player: the slide deck HTML runs in lecture mode inside WebView2
/// and this class drives it sentence by sentence — speech (WinRT system voice or the core's
/// cached cloud voice), slide changes, step builds and the laser pointer, captions,
/// checkpoints, interruption questions and adaptations of the remaining slides.
/// </summary>
internal sealed class LecturePlayer : IDisposable
{
    public FrameworkElement Root { get; }

    private sealed record Sentence(int Slide, string SlideId, string Id, string Text, JsonElement Cues, bool LastOfSlide, JsonElement? Checkpoint);

    private readonly string _artifactId;
    private readonly Action<InfoBarSeverity, string, string> _notify;
    private JsonElement _art;
    private string _outputDir;
    private JsonElement _payload;
    private List<Sentence> _seq = new();
    private int _pos;
    private int _shownSlide = -1;
    private bool _playing;
    private int _gen;
    private bool _disposed;

    private readonly WebView2 _web;
    private bool _webReady;
    private readonly MediaPlayer _player = new();
    private SpeechSynthesizer? _synth;
    private TaskCompletionSource<bool>? _ended;
    private readonly Dictionary<string, Task<string?>> _cloudAudio = new();

    // settings
    private string _engine = "system";
    private string _systemVoice = "";
    private double _rate = 1.0;
    private bool _captions = true;
    private string _adaptPolicy = "auto";

    // UI
    private readonly Button _playBtn;
    private readonly TextBlock _counter;
    private readonly StackPanel _adaptHost;
    private readonly Border _side;
    private readonly StackPanel _qaList;
    private readonly ScrollViewer _qaScroll;
    private readonly TextBox _askBox;
    private readonly ImageAttachments _askImages;
    private readonly StackPanel _checkpointHost;
    private readonly List<JsonElement> _history = new();

    public LecturePlayer(string artifactId, JsonElement artifact, Action<InfoBarSeverity, string, string> notify, Func<nint> hwnd)
    {
        _artifactId = artifactId;
        _art = artifact;
        _notify = notify;
        _outputDir = Str(artifact, "output_dir");
        _payload = Obj(artifact, "payload");
        BuildSequence();
        _player.MediaEnded += (_, _) => _ended?.TrySetResult(true);
        _player.MediaFailed += (_, e) => _ended?.TrySetResult(true);

        _web = new WebView2
        {
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            MinHeight = 300,
            DefaultBackgroundColor = Windows.UI.Color.FromArgb(255, 19, 20, 24),
        };
        _web.Loaded += async (_, _) => await InitWebAsync();

        // --- control bar ---
        Button IconBtn(string glyph, string tip, RoutedEventHandler click)
        {
            var b = new Button { Content = new FontIcon { Glyph = glyph, FontSize = 14 }, Padding = new Thickness(10, 6, 10, 6) };
            ToolTipService.SetToolTip(b, tip);
            b.Click += click;
            return b;
        }
        _playBtn = IconBtn("", L10n.T("lec.play"), (_, _) => TogglePlay());
        var prevSlide = IconBtn("", L10n.T("lec.prev_slide"), (_, _) => JumpSlide(-1));
        var prevSent = IconBtn("", L10n.T("lec.prev_sentence"), (_, _) => JumpSentence(-1));
        var nextSent = IconBtn("", L10n.T("lec.next_sentence"), (_, _) => JumpSentence(1));
        var nextSlide = IconBtn("", L10n.T("lec.next_slide"), (_, _) => JumpSlide(1));
        _counter = new TextBlock { VerticalAlignment = VerticalAlignment.Center, FontSize = 12, Opacity = 0.75, Margin = new Thickness(6, 0, 6, 0) };

        var speed = new ComboBox { MinWidth = 86, VerticalAlignment = VerticalAlignment.Center };
        ToolTipService.SetToolTip(speed, L10n.T("lec.speed"));
        foreach (var r in new[] { 0.75, 1.0, 1.25, 1.5, 1.75, 2.0 })
            speed.Items.Add(new ComboBoxItem { Content = $"{r:0.##}×", Tag = r });
        speed.SelectedIndex = 1;
        speed.SelectionChanged += (_, _) =>
        {
            if ((speed.SelectedItem as ComboBoxItem)?.Tag is double r)
            {
                _rate = r;
                if (_engine == "cloud") _player.PlaybackSession.PlaybackRate = r;
            }
        };

        var captions = new ToggleButton { Content = L10n.T("lec.captions"), IsChecked = true, VerticalAlignment = VerticalAlignment.Center };
        captions.Click += async (_, _) =>
        {
            _captions = captions.IsChecked == true;
            await Js($"lecture.caption({(_captions && _pos < _seq.Count ? JsStr(_seq[_pos].Text) : "''")})");
        };
        var askBtn = new Button { Content = L10n.T("lec.ask"), Style = (Style)Application.Current.Resources["AccentButtonStyle"], VerticalAlignment = VerticalAlignment.Center };
        askBtn.Click += (_, _) => OpenAsk();

        var more = new DropDownButton { Content = new FontIcon { Glyph = "", FontSize = 14 }, VerticalAlignment = VerticalAlignment.Center };
        var moreMenu = new MenuFlyout();
        var voiceItem = new MenuFlyoutItem { Text = L10n.T("lec.voice_settings") };
        voiceItem.Click += async (_, _) => await ShowVoiceSettingsAsync();
        moreMenu.Items.Add(voiceItem);
        var policyMenu = new MenuFlyoutSubItem { Text = L10n.T("lec.adapt_policy") };
        foreach (var (key, label) in new[] { ("auto", "lec.policy_auto"), ("ask", "lec.policy_ask"), ("never", "lec.policy_never") })
        {
            var it = new RadioMenuFlyoutItem { Text = L10n.T(label), GroupName = "lecPolicy", Tag = key };
            it.Click += async (_, _) => { _adaptPolicy = key; await SavePrefsAsync(); };
            policyMenu.Items.Add(it);
        }
        moreMenu.Items.Add(policyMenu);
        moreMenu.Opening += (_, _) =>
        {
            foreach (var i in policyMenu.Items.OfType<RadioMenuFlyoutItem>()) i.IsChecked = (string)i.Tag == _adaptPolicy;
        };
        var restore = new MenuFlyoutItem { Text = L10n.T("lec.restore_original") };
        restore.Click += async (_, _) => await UndoAsync(null, all: true);
        moreMenu.Items.Add(restore);
        more.Flyout = moreMenu;

        // wrap the buttons onto a second line in a narrow pane instead of scrolling sideways
        var bar = new FlowWrapPanel { HorizontalSpacing = 6, VerticalSpacing = 6 };
        foreach (var c in new FrameworkElement[] { prevSlide, prevSent, _playBtn, nextSent, nextSlide, _counter, speed, captions, askBtn, more })
            bar.Children.Add(c);
        _adaptHost = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center };
        var barRow = new StackPanel { Spacing = 6 };
        barRow.Children.Add(bar);
        barRow.Children.Add(_adaptHost);

        var stage = new Grid { RowSpacing = 8 };
        stage.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        stage.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        var webFrame = new Border
        {
            CornerRadius = new CornerRadius(12),
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            Child = _web,
        };
        stage.Children.Add(webFrame);
        Grid.SetRow(barRow, 1);
        stage.Children.Add(barRow);
        // the host may measure us with unbounded width, which would leave the wrapped second row unaccounted for
        stage.SizeChanged += (_, e) => { if (e.NewSize.Width > 0) bar.Width = e.NewSize.Width; };

        // --- side panel: checkpoint + questions ---
        _checkpointHost = new StackPanel { Spacing = 8 };
        _qaList = new StackPanel { Spacing = 10 };
        _qaScroll = new ScrollViewer { Content = _qaList, VerticalScrollBarVisibility = ScrollBarVisibility.Auto };
        _askBox = new TextBox { PlaceholderText = L10n.T("lec.ask_placeholder"), AcceptsReturn = false, TextWrapping = TextWrapping.Wrap };
        _askBox.KeyDown += async (_, e) =>
        {
            if (e.Key == Windows.System.VirtualKey.Enter)
            {
                e.Handled = true;
                await SendQuestionAsync();
            }
        };
        _askImages = new ImageAttachments(
            hwnd,
            () => Root.XamlRoot,
            msg => notify(InfoBarSeverity.Warning, L10n.T("img.title"), msg));
        var send = new Button { Content = L10n.T("lec.send"), HorizontalAlignment = HorizontalAlignment.Right };
        send.Click += async (_, _) => await SendQuestionAsync();
        var close = new Button { Content = new FontIcon { Glyph = "", FontSize = 12 }, Padding = new Thickness(6), HorizontalAlignment = HorizontalAlignment.Right };
        close.Click += (_, _) => _side!.Visibility = Visibility.Collapsed;
        var sideGrid = new Grid { RowSpacing = 8 };
        sideGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        sideGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        sideGrid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        sideGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        sideGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        var head = new Grid();
        head.Children.Add(new TextBlock { Text = L10n.T("lec.qa_title"), FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, VerticalAlignment = VerticalAlignment.Center });
        head.Children.Add(close);
        sideGrid.Children.Add(head);
        Grid.SetRow(_checkpointHost, 1);
        sideGrid.Children.Add(_checkpointHost);
        Grid.SetRow(_qaScroll, 2);
        sideGrid.Children.Add(_qaScroll);
        var askBar = new Grid { ColumnSpacing = 6 };
        askBar.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        askBar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        Grid.SetColumn(_askImages.AttachButton, 0);
        Grid.SetColumn(_askBox, 1);
        askBar.Children.Add(_askImages.AttachButton);
        askBar.Children.Add(_askBox);
        var askStack = new StackPanel { Spacing = 6 };
        askStack.Children.Add(_askImages.Strip);
        askStack.Children.Add(askBar);
        Grid.SetRow(askStack, 3);
        sideGrid.Children.Add(askStack);
        Grid.SetRow(send, 4);
        sideGrid.Children.Add(send);
        _side = new Border
        {
            Width = 340,
            Padding = new Thickness(12),
            CornerRadius = new CornerRadius(12),
            Background = (Brush)Application.Current.Resources["ElevatedBgBrush"],
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            Child = sideGrid,
            Visibility = Visibility.Collapsed,
        };

        var root = new Grid { ColumnSpacing = 10 };
        root.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        root.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        root.Children.Add(stage);
        Grid.SetColumn(_side, 1);
        root.Children.Add(_side);
        Root = root;

        // resume where the learner stopped
        if (_art.TryGetProperty("progress_state", out var ps) && ps.ValueKind == JsonValueKind.Object && Str(ps, "position") is { Length: > 0 } posId)
        {
            var i = _seq.FindIndex(s => s.Id == posId);
            if (i >= 0) _pos = i;
        }
        UpdateCounter();
        _ = LoadSettingsAsync();
    }

    // ------------------------------------------------------------------ model

    private void BuildSequence()
    {
        var seq = new List<Sentence>();
        var slides = Arr(_payload, "slides").ToList();
        for (var si = 0; si < slides.Count; si++)
        {
            var s = slides[si];
            var sid = Str(s, "id");
            JsonElement? cp = s.TryGetProperty("checkpoint", out var c) && c.ValueKind == JsonValueKind.Object ? c : null;
            var narr = Arr(s, "narration").Where(n => Str(n, "text").Trim().Length > 0).ToList();
            if (narr.Count == 0)
            {
                // display-only slide: a silent pause
                seq.Add(new Sentence(si, sid, sid + ".0", "", default, true, cp));
                continue;
            }
            for (var ni = 0; ni < narr.Count; ni++)
            {
                var n = narr[ni];
                var cues = n.TryGetProperty("cues", out var cu) && cu.ValueKind == JsonValueKind.Array ? cu : default;
                seq.Add(new Sentence(si, sid, Str(n, "id").Length > 0 ? Str(n, "id") : $"{sid}.{ni + 1}", Str(n, "text"), cues, ni == narr.Count - 1, cp));
            }
        }
        _seq = seq;
        if (_pos >= _seq.Count) _pos = Math.Max(0, _seq.Count - 1);
    }

    private static IEnumerable<JsonElement> Cues(Sentence s) =>
        s.Cues.ValueKind == JsonValueKind.Array ? s.Cues.EnumerateArray() : Enumerable.Empty<JsonElement>();

    /// Highest build step revealed by sentences of the same slide before position `pos`.
    private int StepBefore(int pos)
    {
        var slide = _seq[pos].Slide;
        var step = 0;
        for (var i = pos - 1; i >= 0 && _seq[i].Slide == slide; i--)
            foreach (var c in Cues(_seq[i]))
                if (Str(c, "type") == "step" && c.TryGetProperty("n", out var n) && n.TryGetInt32(out var k))
                    step = Math.Max(step, k);
        return step;
    }

    // ------------------------------------------------------------------ web

    private async Task InitWebAsync()
    {
        if (_webReady || _disposed) return;
        try
        {
            await _web.EnsureCoreWebView2Async();
            _web.CoreWebView2.Settings.AreDefaultContextMenusEnabled = false;
            _web.CoreWebView2.Settings.AreDevToolsEnabled = false;
            _web.CoreWebView2.WebMessageReceived += OnWebMessage;
            // large decks (inline figure SVGs) can exceed NavigateToString's 2 MB limit: use a file
            var html = SlideDeckViewer.BuildPresentationHtml(SlideDeckViewer.PayloadWithFigures(_payload, _outputDir), L10n.IsEnglish, lectureMode: true);
            var dir = _outputDir.Length > 0 ? _outputDir : Path.Combine(Path.GetTempPath(), "lna-lecture");
            Directory.CreateDirectory(dir);
            var file = Path.Combine(dir, "lecture_view.html");
            await File.WriteAllTextAsync(file, html);
            _web.Source = new Uri(file);
        }
        catch (Exception ex)
        {
            _notify(InfoBarSeverity.Error, L10n.T("studio.lecture"), ex.Message);
        }
    }

    private async void OnWebMessage(Microsoft.Web.WebView2.Core.CoreWebView2 sender, Microsoft.Web.WebView2.Core.CoreWebView2WebMessageReceivedEventArgs args)
    {
        string raw;
        try { raw = args.TryGetWebMessageAsString(); } catch { return; }
        JsonNode? msg;
        try { msg = JsonNode.Parse(raw); } catch { return; }
        var type = msg?["type"]?.GetValue<string>();
        if (type == "ready")
        {
            _webReady = true;
            await ShowPositionAsync();
        }
        else if (type == "key")
        {
            switch (msg?["key"]?.GetValue<string>())
            {
                case " ": TogglePlay(); break;
                case "ArrowRight": JumpSentence(1); break;
                case "ArrowLeft": JumpSentence(-1); break;
                case "PageDown": JumpSlide(1); break;
                case "PageUp": JumpSlide(-1); break;
                case "Escape": Pause(); break;
            }
        }
    }

    private async Task<string?> Js(string script)
    {
        if (!_webReady || _disposed) return null;
        try { return await _web.CoreWebView2.ExecuteScriptAsync(script); }
        catch { return null; }
    }

    private static string JsStr(string s) => "\"" + JsonEncodedText.Encode(s ?? "").ToString() + "\"";

    /// Shows the slide/step/caption for the current position without speaking.
    private async Task ShowPositionAsync()
    {
        if (_seq.Count == 0) return;
        var s = _seq[_pos];
        await Js($"lecture.show({s.Slide},{StepBefore(_pos)})");
        _shownSlide = s.Slide;
        await Js($"lecture.caption({(_captions ? JsStr(s.Text) : "''")})");
        UpdateCounter();
    }

    private void UpdateCounter()
    {
        var slides = Arr(_payload, "slides").Count();
        var slide = _seq.Count > 0 ? _seq[_pos].Slide + 1 : 0;
        var mins = _payload.TryGetProperty("estimated_minutes", out var m) && m.TryGetDouble(out var mv) ? mv : 0;
        _counter.Text = $"{slide} / {slides}" + (mins > 0 ? "  ·  " + L10n.Fmt("lec.minutes_fmt", Math.Max(1, Math.Round(mins))) : "");
    }

    // ------------------------------------------------------------------ playback

    private void TogglePlay()
    {
        if (_playing) Pause();
        else _ = PlayAsync();
    }

    private void Pause()
    {
        _playing = false;
        _gen++;
        try { _player.Pause(); } catch { }
        _ended?.TrySetResult(false);
        _ = Js("lecture.stopLaser()");
        SetPlayIcon();
        _ = SaveProgressAsync();
    }

    private void SetPlayIcon()
    {
        _playBtn.Content = new FontIcon { Glyph = _playing ? "" : "", FontSize = 14 };
        ToolTipService.SetToolTip(_playBtn, L10n.T(_playing ? "lec.pause" : "lec.play"));
    }

    private async Task PlayAsync()
    {
        if (_seq.Count == 0) return;
        var gen = ++_gen;
        _playing = true;
        SetPlayIcon();
        _checkpointHost.Children.Clear();
        while (_playing && gen == _gen && _pos < _seq.Count && !_disposed)
        {
            var s = _seq[_pos];
            if (s.Slide != _shownSlide)
            {
                await Js($"lecture.show({s.Slide},{StepBefore(_pos)})");
                _shownSlide = s.Slide;
                UpdateCounter();
                _ = SaveProgressAsync();
            }
            foreach (var c in Cues(s))
                if (Str(c, "type") == "step" && c.TryGetProperty("n", out var n) && n.TryGetInt32(out var k))
                    await Js($"lecture.step({k})");
            await Js($"lecture.caption({(_captions ? JsStr(s.Text) : "''")})");
            PrefetchCloud(_pos + 1);
            PrefetchCloud(_pos + 2);

            var lasers = Cues(s).Where(c => Str(c, "type") == "laser").Select(c => c.GetRawText()).ToList();
            var ok = await SpeakAsync(s.Text, gen, async () =>
            {
                // the page times each pointer cue by where its words fall in the sentence
                if (gen == _gen && lasers.Count > 0)
                    await Js($"lecture.lasers([{string.Join(",", lasers)}],{JsStr(s.Text)},{_rate.ToString(System.Globalization.CultureInfo.InvariantCulture)})");
            });
            if (!ok || gen != _gen || !_playing) return;
            if (s.LastOfSlide && s.Checkpoint is { } cp)
            {
                _playing = false;
                SetPlayIcon();
                ShowCheckpoint(cp, s);
                return;
            }
            if (_pos >= _seq.Count - 1)
            {
                _playing = false;
                SetPlayIcon();
                await Js($"lecture.caption({JsStr(L10n.T("lec.ended"))})");
                await SaveProgressAsync();
                return;
            }
            _pos++;
        }
    }

    /// Speaks `text` and returns true when it finished (false when interrupted).
    private async Task<bool> SpeakAsync(string text, int gen, Func<Task>? onStart = null)
    {
        if (string.IsNullOrWhiteSpace(text))
        {
            // display-only slide
            if (onStart != null) _ = onStart();
            await Task.Delay(3000);
            return gen == _gen && _playing;
        }
        try
        {
            if (_engine == "cloud")
            {
                var path = await CloudAudioAsync(text);
                if (gen != _gen) return false;
                if (path is null) return await SpeakSystemAsync(text, gen, onStart);
                _player.Source = MediaSource.CreateFromUri(new Uri(path));
                _player.PlaybackSession.PlaybackRate = _rate;
            }
            else
            {
                return await SpeakSystemAsync(text, gen, onStart);
            }
            _ended = new TaskCompletionSource<bool>();
            _player.Play();
            if (onStart != null) _ = onStart();
            var done = await _ended.Task;
            return done && gen == _gen;
        }
        catch (Exception ex)
        {
            _notify(InfoBarSeverity.Warning, L10n.T("lec.voice_settings"), ex.Message);
            return false;
        }
    }

    private async Task<bool> SpeakSystemAsync(string text, int gen, Func<Task>? onStart)
    {
        _synth ??= new SpeechSynthesizer();
        _synth.Options.SpeakingRate = Math.Clamp(_rate, 0.5, 3.0);
        var voice = PickVoice(text);
        if (voice != null) _synth.Voice = voice;
        var stream = await _synth.SynthesizeTextToStreamAsync(text);
        if (gen != _gen) return false;
        _player.Source = MediaSource.CreateFromStream(stream, stream.ContentType);
        _player.PlaybackSession.PlaybackRate = 1.0;
        _ended = new TaskCompletionSource<bool>();
        _player.Play();
        if (onStart != null) _ = onStart();
        var done = await _ended.Task;
        return done && gen == _gen;
    }

    private VoiceInformation? PickVoice(string text)
    {
        var all = SpeechSynthesizer.AllVoices;
        if (_systemVoice.Length > 0 && all.FirstOrDefault(v => v.Id == _systemVoice) is { } chosen) return chosen;
        var cjk = text.Any(ch => ch >= 0x4E00 && ch <= 0x9FFF);
        return cjk
            ? all.FirstOrDefault(v => v.Language.StartsWith("zh", StringComparison.OrdinalIgnoreCase))
            : all.FirstOrDefault(v => v.Language.StartsWith("en", StringComparison.OrdinalIgnoreCase));
    }

    private void PrefetchCloud(int pos)
    {
        if (_engine != "cloud" || pos < 0 || pos >= _seq.Count) return;
        var t = _seq[pos].Text;
        if (t.Trim().Length > 0) _ = CloudAudioAsync(t);
    }

    private Task<string?> CloudAudioAsync(string text)
    {
        lock (_cloudAudio)
        {
            if (_cloudAudio.TryGetValue(text, out var t)) return t;
            t = SynthCloudAsync(text);
            _cloudAudio[text] = t;
            return t;
        }
    }

    private async Task<string?> SynthCloudAsync(string text)
    {
        var (res, err) = await CallAsync("tts.synthesize", Args(("text", text)));
        if (err is { } e)
        {
            lock (_cloudAudio) _cloudAudio.Remove(text);
            _notify(InfoBarSeverity.Warning, L10n.T("tts.engine_cloud"), e.Message);
            return null;
        }
        return res is { } r && Str(r, "path") is { Length: > 0 } p ? p : null;
    }

    private void JumpSentence(int delta)
    {
        if (_seq.Count == 0) return;
        var wasPlaying = _playing;
        Pause();
        _pos = Math.Clamp(_pos + delta, 0, _seq.Count - 1);
        _ = AfterJumpAsync(wasPlaying);
    }

    private void JumpSlide(int delta)
    {
        if (_seq.Count == 0) return;
        var wasPlaying = _playing;
        Pause();
        var target = Math.Clamp(_seq[_pos].Slide + delta, 0, _seq[^1].Slide);
        var i = _seq.FindIndex(s => s.Slide == target);
        if (i >= 0) _pos = i;
        _ = AfterJumpAsync(wasPlaying);
    }

    private void JumpToSlideId(string slideId)
    {
        var i = _seq.FindIndex(s => s.SlideId == slideId);
        if (i < 0) return;
        var wasPlaying = _playing;
        Pause();
        _pos = i;
        _ = AfterJumpAsync(wasPlaying);
    }

    private async Task AfterJumpAsync(bool play)
    {
        _shownSlide = -1;
        await ShowPositionAsync();
        _shownSlide = _seq[_pos].Slide;
        if (play) await PlayAsync();
    }

    // ------------------------------------------------------------------ checkpoints

    private void ShowCheckpoint(JsonElement cp, Sentence s)
    {
        _side.Visibility = Visibility.Visible;
        _checkpointHost.Children.Clear();
        var attempt = Str(cp, "kind") == "attempt";
        var card = new StackPanel { Spacing = 8 };
        card.Children.Add(new TextBlock { Text = L10n.T(attempt ? "lec.attempt" : "lec.checkpoint"), FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        card.Children.Add(Md.Render(Str(cp, "question"), null, 14));
        var buttons = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        TextBox? answer = null;
        if (attempt)
        {
            answer = new TextBox { PlaceholderText = L10n.T("lec.your_answer"), AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 60 };
            card.Children.Add(answer);
            var submit = new Button { Content = L10n.T("lec.submit"), Style = (Style)Application.Current.Resources["AccentButtonStyle"] };
            submit.Click += async (_, _) =>
            {
                var a = answer.Text.Trim();
                if (a.Length == 0) return;
                await AskAsync(L10n.Fmt("lec.attempt_answer_fmt", Str(cp, "question"), a), adapt: false);
            };
            buttons.Children.Add(submit);
        }
        var cont = new Button { Content = L10n.T("lec.continue") };
        cont.Click += async (_, _) =>
        {
            _checkpointHost.Children.Clear();
            if (_pos < _seq.Count - 1) _pos++;
            await PlayAsync();
        };
        buttons.Children.Add(cont);
        card.Children.Add(buttons);
        _checkpointHost.Children.Add(new Border
        {
            Padding = new Thickness(10),
            CornerRadius = new CornerRadius(8),
            BorderThickness = new Thickness(1),
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            Child = card,
        });
    }

    // ------------------------------------------------------------------ questions

    private void OpenAsk()
    {
        Pause();
        _side.Visibility = Visibility.Visible;
        _askBox.Focus(FocusState.Programmatic);
    }

    private async Task SendQuestionAsync()
    {
        var q = _askBox.Text.Trim();
        if (q.Length == 0 && !_askImages.HasItems) return;
        if (q.Length == 0) q = L10n.T("img.ask_default");
        var images = _askImages.ToArgs();
        var shown = _askImages.Snapshot();
        _askBox.Text = "";
        _askImages.Clear();
        if (_playing) Pause();
        await AskAsync(q, adapt: true, images, shown);
    }

    private async Task AskAsync(string question, bool adapt, JsonElement? images = null, IReadOnlyList<PendingItem>? shown = null)
    {
        if (_seq.Count == 0) return;
        _side.Visibility = Visibility.Visible;
        var s = _seq[_pos];
        var entry = new StackPanel { Spacing = 6 };
        entry.Children.Add(new Border
        {
            Background = (Brush)Application.Current.Resources["SubtleFillColorSecondaryBrush"],
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(10, 6, 10, 6),
            HorizontalAlignment = HorizontalAlignment.Right,
            Child = new TextBlock { Text = question, TextWrapping = TextWrapping.Wrap },
        });
        if (shown is { Count: > 0 })
            entry.Children.Insert(0, ImageAttachments.PendingGallery(shown, () => Root.XamlRoot));
        var thinking = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        thinking.Children.Add(new ProgressRing { IsActive = true, Width = 16, Height = 16 });
        thinking.Children.Add(new TextBlock { Text = L10n.T("lec.thinking"), Opacity = 0.7 });
        entry.Children.Add(thinking);
        _qaList.Children.Add(entry);
        _qaScroll.ChangeView(null, double.MaxValue, null);

        var hist = new JsonArray();
        foreach (var h in _history.TakeLast(6)) hist.Add(JsonNode.Parse(h.GetRawText()));
        var histEl = JsonDocument.Parse(hist.ToJsonString()).RootElement.Clone();
        var args = ImageAttachments.Merge(
            Args(("artifact_id", _artifactId), ("slide_id", s.SlideId), ("sentence_id", s.Id), ("question", question), ("history", histEl)),
            images);
        var (res, err) = await CallAsync("lecture.ask", args);
        entry.Children.Remove(thinking);
        if (err is { } e || res is not { } r)
        {
            entry.Children.Add(new TextBlock { Text = (err?.Message) ?? "", Foreground = new SolidColorBrush(Microsoft.UI.Colors.IndianRed), TextWrapping = TextWrapping.Wrap });
            return;
        }
        var answer = Str(r, "answer");
        var speak = Str(r, "speak");
        entry.Children.Add(Md.Render(answer, null, 14));
        var speakBtn = new HyperlinkButton { Content = L10n.T("lec.speak_answer"), Padding = new Thickness(0) };
        speakBtn.Click += async (_, _) => { Pause(); var g = ++_gen; await SpeakAsync(speak.Length > 0 ? speak : answer, g); };
        entry.Children.Add(speakBtn);
        _history.Add(Args(("role", "user"), ("text", question)));
        _history.Add(Args(("role", "assistant"), ("text", answer)));
        _qaScroll.ChangeView(null, double.MaxValue, null);

        var ad = Obj(r, "adapt");
        var needed = ad.ValueKind == JsonValueKind.Object && ad.TryGetProperty("needed", out var nd) && nd.ValueKind == JsonValueKind.True;
        if (!adapt || !needed || _adaptPolicy == "never") return;
        var kind = Str(ad, "kind");
        var reason = Str(ad, "reason");
        if (_adaptPolicy == "auto")
        {
            _ = AdaptAsync(s.SlideId, kind, reason, question);
            return;
        }
        // ask: show the suggestion with its reason
        var sug = new StackPanel { Spacing = 6 };
        sug.Children.Add(new TextBlock { Text = $"{L10n.T("lec.adapt_suggest")}{(L10n.IsEnglish ? ": " : "：")}{KindLabel(kind)}", FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap });
        if (reason.Length > 0) sug.Children.Add(new TextBlock { Text = reason, Opacity = 0.75, TextWrapping = TextWrapping.Wrap });
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        var apply = new Button { Content = L10n.T("lec.adapt_apply") };
        var skip = new Button { Content = L10n.T("lec.adapt_skip") };
        apply.Click += (_, _) => { sug.Visibility = Visibility.Collapsed; _ = AdaptAsync(s.SlideId, kind, reason, question); };
        skip.Click += (_, _) => sug.Visibility = Visibility.Collapsed;
        row.Children.Add(apply);
        row.Children.Add(skip);
        sug.Children.Add(row);
        entry.Children.Add(sug);
    }

    private static string KindLabel(string kind) => kind switch
    {
        "extend" => L10n.T("lec.kind_extend"),
        "slower" => L10n.T("lec.kind_slower"),
        "simpler" => L10n.T("lec.kind_simpler"),
        "more_examples" => L10n.T("lec.kind_more_examples"),
        "skip_known" => L10n.T("lec.kind_skip_known"),
        _ => kind,
    };

    // ------------------------------------------------------------------ adaptation

    private async Task AdaptAsync(string slideId, string kind, string reason, string question)
    {
        _adaptHost.Children.Clear();
        var busy = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        busy.Children.Add(new ProgressRing { IsActive = true, Width = 14, Height = 14 });
        busy.Children.Add(new TextBlock { Text = L10n.T("lec.adapting"), FontSize = 12, VerticalAlignment = VerticalAlignment.Center });
        _adaptHost.Children.Add(busy);
        // the learner keeps listening to the original while the new slides are generated
        var (res, err) = await CallAsync("lecture.adapt", Args(("artifact_id", _artifactId), ("slide_id", slideId), ("kind", kind), ("reason", reason), ("question", question)));
        _adaptHost.Children.Clear();
        if (err is { } e || res is not { } r)
        {
            _notify(InfoBarSeverity.Warning, L10n.T("lec.adapt_failed"), err?.Message ?? "");
            return;
        }
        await ApplyArtifactAsync(Obj(r, "artifact"));
        ShowAdaptChip(Obj(r, "adaptation"));
    }

    private void ShowAdaptChip(JsonElement a)
    {
        _adaptHost.Children.Clear();
        var panel = new StackPanel { Spacing = 8, MaxWidth = 360 };
        panel.Children.Add(new TextBlock { Text = KindLabel(Str(a, "kind")), FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        if (Str(a, "reason") is { Length: > 0 } reason)
            panel.Children.Add(new TextBlock { Text = $"{L10n.T("lec.reason")}{(L10n.IsEnglish ? ": " : "：")}{reason}", TextWrapping = TextWrapping.Wrap });
        var flyout = new Flyout { Content = panel };
        var inserted = Arr(a, "inserted").Select(x => x.GetString() ?? "").ToList();
        var insertedTitles = Arr(a, "inserted_titles").Select(x => x.GetString() ?? "").ToList();
        if (inserted.Count > 0)
        {
            panel.Children.Add(new TextBlock { Text = L10n.T("lec.inserted"), Opacity = 0.75 });
            for (var i = 0; i < inserted.Count; i++)
            {
                var id = inserted[i];
                var link = new HyperlinkButton { Content = i < insertedTitles.Count ? insertedTitles[i] : id, Padding = new Thickness(0) };
                link.Click += (_, _) => { flyout.Hide(); JumpToSlideId(id); };
                panel.Children.Add(link);
            }
        }
        var removed = Arr(a, "removed_titles").Select(x => x.GetString() ?? "").Where(x => x.Length > 0).ToList();
        if (removed.Count > 0)
        {
            panel.Children.Add(new TextBlock { Text = L10n.T("lec.replaced"), Opacity = 0.75 });
            panel.Children.Add(new TextBlock { Text = string.Join(L10n.IsEnglish ? ", " : "、", removed), TextWrapping = TextWrapping.Wrap });
        }
        var undo = new Button { Content = L10n.T("lec.undo") };
        var aid = Str(a, "id");
        undo.Click += async (_, _) => { flyout.Hide(); await UndoAsync(aid, all: false); };
        panel.Children.Add(undo);
        var chip = new Button
        {
            Content = new StackPanel
            {
                Orientation = Orientation.Horizontal,
                Spacing = 6,
                Children = { new FontIcon { Glyph = "", FontSize = 12 }, new TextBlock { Text = L10n.T("lec.adapted"), FontSize = 12 } },
            },
            Flyout = flyout,
            Padding = new Thickness(10, 4, 10, 4),
        };
        _adaptHost.Children.Add(chip);
    }

    private async Task UndoAsync(string? adaptationId, bool all)
    {
        var kv = new List<(string, object?)> { ("artifact_id", _artifactId), ("all", all) };
        if (adaptationId != null) kv.Add(("adaptation_id", adaptationId));
        var (res, err) = await CallAsync("lecture.undo", Args(kv.ToArray()));
        if (err is { } e || res is not { } r)
        {
            _notify(InfoBarSeverity.Warning, L10n.T("lec.undo"), err?.Message ?? "");
            return;
        }
        _adaptHost.Children.Clear();
        await ApplyArtifactAsync(Obj(r, "artifact"));
    }

    /// Replaces the lecture content while keeping the learner's current sentence.
    private async Task ApplyArtifactAsync(JsonElement art)
    {
        if (art.ValueKind != JsonValueKind.Object) return;
        var curId = _seq.Count > 0 ? _seq[_pos].Id : "";
        var curSlideId = _seq.Count > 0 ? _seq[_pos].SlideId : "";
        _art = art;
        _payload = Obj(art, "payload");
        if (Str(art, "output_dir") is { Length: > 0 } od) _outputDir = od;
        BuildSequence();
        var i = _seq.FindIndex(s => s.Id == curId);
        if (i < 0) i = _seq.FindIndex(s => s.SlideId == curSlideId);
        _pos = Math.Max(0, i);
        var withFigures = SlideDeckViewer.PayloadWithFigures(_payload, _outputDir);
        var slidesJson = JsonNode.Parse(withFigures)?["slides"]?.ToJsonString() ?? "[]";
        if (_seq.Count > 0)
        {
            await Js($"lecture.setSlides({slidesJson},{_seq[_pos].Slide},{StepBefore(_pos)})");
            _shownSlide = _seq[_pos].Slide;
        }
        UpdateCounter();
    }

    // ------------------------------------------------------------------ settings & progress

    private async Task LoadSettingsAsync()
    {
        var (tts, _) = await CallAsync("tts.get_settings", Args());
        if (tts is { } t)
        {
            _engine = Str(t, "engine") == "cloud" ? "cloud" : "system";
            _systemVoice = Str(t, "system_voice");
        }
        var (pref, _) = await CallAsync("settings.get", Args(("key", "lecture.prefs")));
        if (pref is { } p && Obj(p, "value") is { ValueKind: JsonValueKind.Object } v && Str(v, "adapt_policy") is { Length: > 0 } pol)
            _adaptPolicy = pol;
    }

    private async Task SavePrefsAsync() =>
        await CallAsync("settings.set", Args(("key", "lecture.prefs"), ("value", Args(("adapt_policy", _adaptPolicy)))));

    private async Task SaveProgressAsync()
    {
        if (_seq.Count == 0) return;
        await CallAsync("studio.save_progress", Args(("artifact_id", _artifactId), ("state", Args(("position", _seq[_pos].Id)))));
    }

    private async Task ShowVoiceSettingsAsync()
    {
        var (cur, err) = await CallAsync("tts.get_settings", Args());
        if (cur is not { } s) return;
        var cloud = Obj(s, "cloud");
        var panel = new StackPanel { Spacing = 10, MinWidth = 420 };
        var engine = new RadioButtons { Header = L10n.T("tts.engine") };
        engine.Items.Add(L10n.T("tts.engine_system"));
        engine.Items.Add(L10n.T("tts.engine_cloud"));
        engine.SelectedIndex = Str(s, "engine") == "cloud" ? 1 : 0;
        panel.Children.Add(engine);

        var sysVoice = new ComboBox { Header = L10n.T("tts.system_voice"), HorizontalAlignment = HorizontalAlignment.Stretch };
        sysVoice.Items.Add(new ComboBoxItem { Content = L10n.T("tts.default_voice"), Tag = "" });
        foreach (var v in SpeechSynthesizer.AllVoices)
            sysVoice.Items.Add(new ComboBoxItem { Content = $"{v.DisplayName} ({v.Language})", Tag = v.Id });
        sysVoice.SelectedIndex = Math.Max(0, sysVoice.Items.Cast<ComboBoxItem>().ToList().FindIndex(i => (string)i.Tag == Str(s, "system_voice")));

        var cloudPanel = new StackPanel { Spacing = 8 };
        cloudPanel.Children.Add(new TextBlock { Text = L10n.T("tts.cloud_warning"), FontSize = 12, Opacity = 0.75, TextWrapping = TextWrapping.Wrap });
        var provider = new ComboBox { Header = L10n.T("tts.provider"), HorizontalAlignment = HorizontalAlignment.Stretch };
        provider.Items.Add(new ComboBoxItem { Content = L10n.T("tts.provider_qwen"), Tag = "qwen" });
        provider.Items.Add(new ComboBoxItem { Content = L10n.T("tts.provider_openai"), Tag = "openai" });
        provider.SelectedIndex = Str(cloud, "provider") == "openai" ? 1 : 0;
        var baseUrl = new TextBox { Header = L10n.T("tts.base_url"), Text = Str(cloud, "base_url") };
        var model = new TextBox { Header = L10n.T("tts.model"), Text = Str(cloud, "model") };
        var voice = new ComboBox { Header = L10n.T("tts.voice"), IsEditable = true, HorizontalAlignment = HorizontalAlignment.Stretch, Text = Str(cloud, "voice") };
        foreach (var v in Arr(s, "voices")) voice.Items.Add(Str(v, "id"));
        var key = new PasswordBox
        {
            Header = L10n.T("tts.api_key"),
            PlaceholderText = s.TryGetProperty("has_api_key", out var hk) && hk.ValueKind == JsonValueKind.True ? L10n.T("tts.api_key_saved") : "",
        };
        foreach (var c in new FrameworkElement[] { provider, baseUrl, model, voice, key }) cloudPanel.Children.Add(c);
        panel.Children.Add(sysVoice);
        panel.Children.Add(cloudPanel);
        void Sync()
        {
            var isCloud = engine.SelectedIndex == 1;
            cloudPanel.Visibility = isCloud ? Visibility.Visible : Visibility.Collapsed;
            sysVoice.Visibility = isCloud ? Visibility.Collapsed : Visibility.Visible;
        }
        engine.SelectionChanged += (_, _) => Sync();
        Sync();

        async Task<bool> SaveAsync()
        {
            var kv = new List<(string, object?)>
            {
                ("engine", engine.SelectedIndex == 1 ? "cloud" : "system"),
                ("system_voice", (sysVoice.SelectedItem as ComboBoxItem)?.Tag as string ?? ""),
                ("provider", (provider.SelectedItem as ComboBoxItem)?.Tag as string ?? "qwen"),
                ("base_url", baseUrl.Text.Trim()),
                ("model", model.Text.Trim()),
                ("voice", (voice.Text ?? "").Trim()),
            };
            if (key.Password.Trim().Length > 0) kv.Add(("api_key", key.Password.Trim()));
            var (_, e) = await CallAsync("tts.set_settings", Args(kv.ToArray()));
            if (e is { } er)
            {
                _notify(InfoBarSeverity.Error, L10n.T("tts.title"), er.Message);
                return false;
            }
            lock (_cloudAudio) _cloudAudio.Clear();
            await LoadSettingsAsync();
            return true;
        }

        var preview = new Button { Content = L10n.T("tts.preview") };
        preview.Click += async (_, _) =>
        {
            if (!await SaveAsync()) return;
            Pause();
            var g = ++_gen;
            await SpeakAsync(L10n.T("tts.preview_text"), g);
        };
        panel.Children.Add(preview);

        var dlg = new ContentDialog
        {
            Title = L10n.T("tts.title"),
            Content = new ScrollViewer { Content = panel, MaxHeight = 560 },
            PrimaryButtonText = L10n.T("common.save"),
            CloseButtonText = L10n.T("common.cancel"),
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = Root.XamlRoot,
        };
        if (await dlg.ShowAsync() == ContentDialogResult.Primary && await SaveAsync())
            _notify(InfoBarSeverity.Success, L10n.T("tts.title"), L10n.T("tts.saved"));
    }

    public void Dispose()
    {
        if (_disposed) return;
        _ = SaveProgressAsync();
        _disposed = true;
        _gen++;
        _ended?.TrySetResult(false);
        try { _player.Pause(); _player.Dispose(); } catch { }
        try { _synth?.Dispose(); } catch { }
    }
}
