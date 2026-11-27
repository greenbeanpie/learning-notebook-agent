import AppKit
import AVFoundation
import SwiftUI
import WebKit

// The narrated lecture ("讲解") player: the shared deck page in lecture mode inside a WKWebView,
// driven sentence by sentence with the system speech synthesizer or the core's cached cloud
// voice, plus captions, checkpoints, interruption questions and adaptations of the slides ahead.
// The logic that needs no UI lives in Models/LectureModel.swift.

/// Receives the page's `postMessage` calls.
final class LecWebBridge: NSObject, WKScriptMessageHandler {
    var onMessage: ((String) -> Void)?

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let text = message.body as? String else { return }
        DispatchQueue.main.async { self.onMessage?(text) }
    }
}

/// Tells when speech or an audio file has finished.
final class LecAudioBridge: NSObject, AVSpeechSynthesizerDelegate, AVAudioPlayerDelegate {
    var onSpeech: ((AVSpeechUtterance, Bool) -> Void)?
    var onFile: ((AVAudioPlayer, Bool) -> Void)?

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        DispatchQueue.main.async { self.onSpeech?(utterance, true) }
    }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        DispatchQueue.main.async { self.onSpeech?(utterance, false) }
    }

    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        DispatchQueue.main.async { self.onFile?(player, true) }
    }
}

@MainActor
final class LectureController: ObservableObject {
    let artifactId: String
    let en: Bool
    let webView: WKWebView

    @Published var payload: [String: Any]
    @Published var seq: [LecSentence]
    @Published var pos: Int
    @Published var playing = false
    @Published var captions = true
    @Published var rate: Double = 1.0
    @Published var engine = "system"
    /// `AVSpeechSynthesisVoice` identifier chosen in the voice settings ("" = default for the language).
    @Published var systemVoice = ""
    @Published var policy = "auto"
    @Published var checkpoint: LecSentence?
    @Published var showAsk = false
    @Published var qa: [LecQA] = []
    @Published var adapting = false
    @Published var adaptation: [String: Any]?
    @Published var status = ""

    private let core: CoreClient
    private var outputDir: String
    private let web = LecWebBridge()
    private let audio = LecAudioBridge()
    private let synth = AVSpeechSynthesizer()
    private var player: AVAudioPlayer?
    private var webReady = false
    private var playTask: Task<Void, Never>?
    private var cloudCache: [String: Task<String?, Never>] = [:]
    private var speechCont: CheckedContinuation<Bool, Never>?
    private var fileCont: CheckedContinuation<Bool, Never>?
    /// The utterance / file being played; a late callback of an older one is ignored.
    private var currentUtterance: AVSpeechUtterance?

    init(artifact: StudioArtifactItem, core: CoreClient, en: Bool) {
        self.artifactId = artifact.id
        self.core = core
        self.en = en
        self.payload = artifact.payload
        self.outputDir = artifact.outputDir
        let sequence = Lecture.buildSequence(artifact.payload)
        self.seq = sequence
        let saved = artifact.progressState["position"] as? String ?? ""
        self.pos = max(sequence.firstIndex { $0.id == saved } ?? 0, 0)

        let config = WKWebViewConfiguration()
        let content = WKUserContentController()
        config.userContentController = content
        self.webView = WKWebView(frame: .zero, configuration: config)
        content.add(web, name: "lna")
        web.onMessage = { [weak self] msg in self?.handle(msg) }
        synth.delegate = audio
        audio.onSpeech = { [weak self] utterance, ok in
            guard let self, self.currentUtterance === utterance else { return }
            self.currentUtterance = nil
            self.speechCont?.resume(returning: ok)
            self.speechCont = nil
        }
        audio.onFile = { [weak self] finished, ok in
            guard let self, self.player === finished else { return }
            self.fileCont?.resume(returning: ok)
            self.fileCont = nil
        }
    }

    // MARK: page

    /// Loads the settings and the page; called once when the player appears.
    func start(settingsOnly: Bool = false) async {
        if let s = try? await core.call(method: "tts.get_settings") {
            engine = (s["engine"] as? String) == "cloud" ? "cloud" : "system"
            systemVoice = s["system_voice"] as? String ?? ""
        }
        if let r = try? await core.call(method: "settings.get", params: ["key": "lecture.prefs"]),
           let v = r["value"] as? [String: Any], let p = v["adapt_policy"] as? String, !p.isEmpty {
            policy = p
        }
        if settingsOnly { return }
        guard let url = Bundle.module.url(forResource: "deck_player", withExtension: "html"),
              let template = try? String(contentsOf: url, encoding: .utf8) else {
            status = L10n.t("lecture.player_missing", en: en)
            return
        }
        let bridge = "<script>window.chrome={webview:{postMessage:function(m){window.webkit.messageHandlers.lna.postMessage(m)}}};</script>"
        let html = Lecture.deckHtml(
            template: template,
            payload: Lecture.payloadWithFigures(payload, outputDir: outputDir),
            en: en,
            bridge: bridge
        )
        webView.loadHTMLString(html, baseURL: nil)
    }

    private func handle(_ message: String) {
        guard let data = message.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        if (obj["type"] as? String) == "ready" {
            webReady = true
            showPosition()
        }
    }

    private func js(_ script: String) {
        guard webReady else { return }
        webView.evaluateJavaScript(script, completionHandler: nil)
    }

    private func caption(_ text: String) {
        js("lecture.caption(\(captions ? Lecture.jsString(text) : "''"))")
    }

    func toggleCaptions() {
        captions.toggle()
        if let s = seq[safe: pos] { caption(s.text) }
    }

    private func showPosition() {
        guard let s = seq[safe: pos] else { return }
        js("lecture.show(\(s.slide),\(Lecture.stepBefore(seq, pos)))")
        caption(s.text)
    }

    private func saveProgress() {
        guard let s = seq[safe: pos] else { return }
        let id = artifactId
        let c = core
        Task {
            _ = try? await c.call(method: "studio.save_progress", params: ["artifact_id": id, "state": ["position": s.id]])
        }
    }

    /// Stops everything; called when the player goes away.
    func shutdown() {
        playTask?.cancel()
        stopAudio()
        saveProgress()
        webView.configuration.userContentController.removeScriptMessageHandler(forName: "lna")
    }

    // MARK: speech

    private func cloudAudio(_ text: String) -> Task<String?, Never> {
        if let t = cloudCache[text] { return t }
        let c = core
        let t = Task<String?, Never> {
            let r = try? await c.call(method: "tts.synthesize", params: ["text": text])
            let path = r?["path"] as? String
            return (path?.isEmpty ?? true) ? nil : path
        }
        cloudCache[text] = t
        return t
    }

    private func speakSystem(_ text: String) async -> Bool {
        await withCheckedContinuation { (cont: CheckedContinuation<Bool, Never>) in
            speechCont = cont
            let u = AVSpeechUtterance(string: text)
            currentUtterance = u
            u.voice = AVSpeechSynthesisVoice(identifier: systemVoice) ?? AVSpeechSynthesisVoice(language: en ? "en-US" : "zh-CN")
            let base = Double(AVSpeechUtteranceDefaultSpeechRate)
            let wanted = base * rate
            u.rate = Float(min(max(wanted, Double(AVSpeechUtteranceMinimumSpeechRate)), Double(AVSpeechUtteranceMaximumSpeechRate)))
            synth.speak(u)
        }
    }

    private func speakFile(_ path: String) async -> Bool {
        await withCheckedContinuation { (cont: CheckedContinuation<Bool, Never>) in
            guard let p = try? AVAudioPlayer(contentsOf: URL(fileURLWithPath: path)) else {
                cont.resume(returning: true)
                return
            }
            p.delegate = audio
            p.enableRate = true
            p.rate = Float(rate)
            player = p
            fileCont = cont
            p.play()
        }
    }

    private func speak(_ text: String) async -> Bool {
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            return !Task.isCancelled
        }
        if engine == "cloud", let path = await cloudAudio(text).value {
            return await speakFile(path)
        }
        return await speakSystem(text)
    }

    private func stopAudio() {
        currentUtterance = nil
        synth.stopSpeaking(at: .immediate)
        player?.stop()
        // a stopped file never reports back, so release whoever waits for it
        fileCont?.resume(returning: false)
        fileCont = nil
        speechCont?.resume(returning: false)
        speechCont = nil
    }

    // MARK: playback

    func pause() {
        playTask?.cancel()
        playTask = nil
        playing = false
        stopAudio()
        js("lecture.stopLaser()")
        saveProgress()
    }

    func play() {
        guard !seq.isEmpty else { return }
        playTask?.cancel()
        checkpoint = nil
        playing = true
        playTask = Task { [weak self] in await self?.runLoop() }
    }

    private func runLoop() async {
        var shown = -1
        while !Task.isCancelled && pos < seq.count {
            let s = seq[pos]
            if s.slide != shown {
                js("lecture.show(\(s.slide),\(Lecture.stepBefore(seq, pos)))")
                shown = s.slide
                saveProgress()
            }
            var lasers: [String] = []
            for cue in s.cues {
                switch cue["type"] as? String {
                case "step":
                    let n = (cue["n"] as? Int) ?? Int((cue["n"] as? Double) ?? 0)
                    js("lecture.step(\(n))")
                case "laser":
                    if let d = try? JSONSerialization.data(withJSONObject: cue), let t = String(data: d, encoding: .utf8) {
                        lasers.append(t)
                    }
                default:
                    break
                }
            }
            caption(s.text)
            if engine == "cloud" {
                for ahead in 1...2 {
                    if let t = seq[safe: pos + ahead]?.text, !t.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        _ = cloudAudio(t)
                    }
                }
            }
            // the page times each pointer cue by where its words fall in the sentence
            if !lasers.isEmpty {
                js("lecture.lasers([\(lasers.joined(separator: ","))],\(Lecture.jsString(s.text)),\(rate))")
            }
            let ok = await speak(s.text)
            if !ok || Task.isCancelled { return }
            if s.lastOfSlide, s.checkpoint != nil {
                playing = false
                checkpoint = s
                showAsk = true
                return
            }
            if pos >= seq.count - 1 {
                playing = false
                caption(L10n.t("lec.ended", en: en))
                saveProgress()
                return
            }
            pos += 1
        }
    }

    func jumpTo(_ newPos: Int) {
        let was = playing
        pause()
        pos = min(max(newPos, 0), max(seq.count - 1, 0))
        showPosition()
        if was { play() }
    }

    func slideStart(delta: Int) -> Int {
        guard let cur = seq[safe: pos], let last = seq.last else { return 0 }
        let target = min(max(cur.slide + delta, 0), last.slide)
        return max(seq.firstIndex { $0.slide == target } ?? 0, 0)
    }

    func cycleRate() {
        rate = Lecture.nextRate(rate)
        player?.rate = Float(rate)
    }

    func continueAfterCheckpoint() {
        checkpoint = nil
        if pos < seq.count - 1 { pos += 1 }
        play()
    }

    func speakAnswer(_ entry: LecQA) {
        pause()
        let text = entry.speak.isEmpty ? entry.answer : entry.speak
        playTask = Task { [weak self] in _ = await self?.speak(text) }
    }

    // MARK: questions and adaptations

    /// Voice settings changed: cached cloud audio is stale, the engine and voice are re-read.
    func reloadVoiceSettings() {
        cloudCache.removeAll()
        Task { await start(settingsOnly: true) }
    }

    /// Speaks `text` now with the current settings (voice preview).
    func preview(_ text: String) {
        pause()
        playTask = Task { [weak self] in _ = await self?.speak(text) }
    }

    func setPolicy(_ p: String) {
        policy = p
        let c = core
        Task {
            _ = try? await c.call(method: "settings.set", params: ["key": "lecture.prefs", "value": ["adapt_policy": p]])
        }
    }

    private func apply(artifact art: [String: Any]) {
        guard let p = art["payload"] as? [String: Any] else { return }
        let curId = seq[safe: pos]?.id
        let curSlide = seq[safe: pos]?.slideId
        payload = p
        if let d = art["output_dir"] as? String, !d.isEmpty { outputDir = d }
        seq = Lecture.buildSequence(p)
        pos = seq.firstIndex { $0.id == curId } ?? max(seq.firstIndex { $0.slideId == curSlide } ?? 0, 0)
        let slides = Lecture.payloadWithFigures(p, outputDir: outputDir)["slides"] ?? []
        if let s = seq[safe: pos], JSONSerialization.isValidJSONObject(slides),
           let d = try? JSONSerialization.data(withJSONObject: slides), let t = String(data: d, encoding: .utf8) {
            js("lecture.setSlides(\(t),\(s.slide),\(Lecture.stepBefore(seq, pos)))")
        }
    }

    func adapt(slideId: String, kind: String, reason: String, question: String) {
        adapting = true
        let id = artifactId
        let c = core
        Task {
            do {
                let r = try await c.call(
                    method: "lecture.adapt",
                    params: ["artifact_id": id, "slide_id": slideId, "kind": kind, "reason": reason, "question": question]
                )
                if let art = r["artifact"] as? [String: Any] { apply(artifact: art) }
                adaptation = r["adaptation"] as? [String: Any]
            } catch {
                status = L10n.t("lec.adapt_failed", en: en) + ": " + error.localizedDescription
            }
            adapting = false
        }
    }

    func undo(all: Bool) {
        let id = artifactId
        let c = core
        let adaptationId = adaptation?["id"] as? String
        Task {
            do {
                var params: [String: Any] = ["artifact_id": id, "all": all]
                if !all, let a = adaptationId, !a.isEmpty { params["adaptation_id"] = a }
                let r = try await c.call(method: "lecture.undo", params: params)
                if let art = r["artifact"] as? [String: Any] { apply(artifact: art) }
                adaptation = nil
            } catch {
                status = error.localizedDescription
            }
        }
    }

    /// Asks the teacher a question about the sentence being spoken; `extra` carries attachments.
    func ask(_ question: String, allowAdapt: Bool, extra: [String: Any] = [:]) {
        guard let s = seq[safe: pos], !question.isEmpty || !extra.isEmpty else { return }
        if playing { pause() }
        let idx = qa.count
        qa.append(LecQA(question: question))
        var history: [[String: Any]] = []
        for e in qa.suffix(5) where !e.pending {
            history.append(["role": "user", "text": e.question])
            history.append(["role": "assistant", "text": e.answer])
        }
        var params: [String: Any] = [
            "artifact_id": artifactId, "slide_id": s.slideId, "sentence_id": s.id,
            "question": question, "history": history,
        ]
        params.merge(extra) { _, new in new }
        let c = core
        Task {
            do {
                let r = try await c.call(method: "lecture.ask", params: params)
                let ad = r["adapt"] as? [String: Any] ?? [:]
                let needed = (ad["needed"] as? Bool ?? false) && allowAdapt && policy != "never"
                let stored = r["attachments"] as? [[String: Any]] ?? []
                guard qa.indices.contains(idx) else { return }
                qa[idx].images = stored.compactMap { ($0["path"] as? String).flatMap { $0.isEmpty ? nil : $0 } }
                qa[idx].answer = r["answer"] as? String ?? ""
                qa[idx].speak = r["speak"] as? String ?? ""
                qa[idx].pending = false
                qa[idx].suggestReason = ad["reason"] as? String ?? ""
                if needed && policy == "ask" { qa[idx].suggestKind = ad["kind"] as? String ?? "" }
                if needed && policy == "auto" {
                    adapt(slideId: s.slideId, kind: ad["kind"] as? String ?? "", reason: ad["reason"] as? String ?? "", question: question)
                }
            } catch {
                guard qa.indices.contains(idx) else { return }
                qa[idx].pending = false
                qa[idx].error = error.localizedDescription
            }
        }
    }
}

extension Array {
    /// The element at `i`, or nil when out of range.
    subscript(safe i: Int) -> Element? {
        indices.contains(i) ? self[i] : nil
    }
}

struct LectureWebView: NSViewRepresentable {
    let webView: WKWebView

    func makeNSView(context: Context) -> WKWebView { webView }
    func updateNSView(_ nsView: WKWebView, context: Context) {}
}

struct LecturePlayerView: View {
    let en: Bool
    @StateObject private var lc: LectureController
    @StateObject private var askAttach = AttachmentStore()
    @State private var question = ""
    @State private var attemptAnswer = ""
    @State private var showAdaptDetails = false
    @State private var showVoiceSettings = false
    @EnvironmentObject private var appState: AppState

    @MainActor
    init(artifact: StudioArtifactItem, core: CoreClient, en: Bool) {
        self.en = en
        _lc = StateObject(wrappedValue: LectureController(artifact: artifact, core: core, en: en))
    }

    private func t(_ key: String) -> String { L10n.t(key, en: en) }

    var body: some View {
        VStack(spacing: 8) {
            LectureWebView(webView: lc.webView)
                .frame(maxWidth: .infinity, minHeight: 240, maxHeight: .infinity)
                .clipShape(RoundedRectangle(cornerRadius: 10))
            controls
            options
            if !lc.status.isEmpty {
                Text(lc.status).font(.caption).foregroundColor(.red)
            }
            if lc.showAsk {
                askPanel
                    .frame(maxHeight: 280)
            }
        }
        .task { await lc.start() }
        .onDisappear { lc.shutdown() }
        .sheet(isPresented: $showAdaptDetails) { adaptDetails }
        .sheet(isPresented: $showVoiceSettings) {
            VoiceSettingsSheet(en: en, onSaved: { lc.reloadVoiceSettings() }, onPreview: { lc.preview($0) }, onClose: { showVoiceSettings = false })
                .environmentObject(appState)
        }
    }

    // MARK: controls

    private var controls: some View {
        HStack(spacing: 6) {
            Button { lc.jumpTo(lc.slideStart(delta: -1)) } label: { Image(systemName: "backward.end.fill") }
                .help(t("lec.prev_slide"))
            Button { lc.jumpTo(lc.pos - 1) } label: { Image(systemName: "backward.fill") }
                .help(t("lec.prev_sentence"))
            Button {
                if lc.playing { lc.pause() } else { lc.play() }
            } label: {
                Image(systemName: lc.playing ? "pause.fill" : "play.fill").frame(width: 22)
            }
            .buttonStyle(.borderedProminent)
            .help(t(lc.playing ? "lec.pause" : "lec.play"))
            Button { lc.jumpTo(lc.pos + 1) } label: { Image(systemName: "forward.fill") }
                .help(t("lec.next_sentence"))
            Button { lc.jumpTo(lc.slideStart(delta: 1)) } label: { Image(systemName: "forward.end.fill") }
                .help(t("lec.next_slide"))
            let total = (lc.payload["slides"] as? [Any])?.count ?? 0
            Text("\((lc.seq[safe: lc.pos]?.slide ?? 0) + 1)/\(total)")
                .font(.caption)
                .padding(.horizontal, 4)
            Spacer()
            Button { lc.cycleRate() } label: { Text(String(format: "%g×", lc.rate)).font(.caption) }
                .help(t("lec.speed"))
        }
        .buttonStyle(.bordered)
    }

    private var options: some View {
        HStack(spacing: 8) {
            Toggle(t("lec.captions"), isOn: Binding(get: { lc.captions }, set: { _ in lc.toggleCaptions() }))
                .toggleStyle(.checkbox)
            Button(t("lec.ask")) {
                lc.pause()
                lc.showAsk = true
            }
            .buttonStyle(.borderedProminent)
            Menu {
                Button(t("lec.voice_settings")) { showVoiceSettings = true }
                Menu(t("lec.adapt_policy")) {
                    ForEach([("auto", "lec.policy_auto"), ("ask", "lec.policy_ask"), ("never", "lec.policy_never")], id: \.0) { code, key in
                        Button((lc.policy == code ? "✓ " : "") + t(key)) { lc.setPolicy(code) }
                    }
                }
                Button(t("lec.restore_original")) { lc.undo(all: true) }
            } label: {
                Image(systemName: "ellipsis")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            if lc.adapting {
                ProgressView().controlSize(.small)
                Text(t("lec.adapting")).font(.caption)
            } else if lc.adaptation != nil {
                Button(t("lec.adapted")) { showAdaptDetails = true }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
            Spacer()
        }
    }

    // MARK: questions

    private var askPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(t("lec.qa_title")).font(.headline)
                Spacer()
                Button { lc.showAsk = false } label: { Image(systemName: "xmark") }
                    .buttonStyle(.plain)
            }
            if let cp = lc.checkpoint { checkpointView(cp) }
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(lc.qa) { entry in
                            qaRow(entry).id(entry.id)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: lc.qa.count) { _, _ in
                    if let last = lc.qa.last { proxy.scrollTo(last.id, anchor: .bottom) }
                }
            }
            AttachChips(store: askAttach, en: en)
            HStack(spacing: 8) {
                AttachMenuButton(store: askAttach, en: en)
                TextField(t("lec.ask_placeholder"), text: $question)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { send() }
                Button(t("lec.send")) { send() }
                    .buttonStyle(.borderedProminent)
                    .disabled(question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && askAttach.isEmpty)
            }
            .attachDrop(askAttach, en: en)
        }
        .padding(10)
        .background(Color.primary.opacity(0.06))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    private func send() {
        var q = question.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty || !askAttach.isEmpty else { return }
        if q.isEmpty { q = L10n.t("ctx.ask_default", en: en) }
        var extra: [String: Any] = [:]
        askAttach.merge(into: &extra)
        question = ""
        askAttach.clear()
        lc.ask(q, allowAdapt: true, extra: extra)
    }

    private func checkpointView(_ cp: LecSentence) -> some View {
        let c = cp.checkpoint ?? [:]
        let attempt = (c["kind"] as? String) == "attempt"
        let prompt = c["question"] as? String ?? ""
        return VStack(alignment: .leading, spacing: 6) {
            Text(t(attempt ? "lec.attempt" : "lec.checkpoint")).font(.subheadline.weight(.semibold))
            Text(prompt)
            if attempt {
                TextField(t("lec.your_answer"), text: $attemptAnswer)
                    .textFieldStyle(.roundedBorder)
            }
            HStack {
                if attempt {
                    Button(t("lec.submit")) {
                        let text = L10n.t("lec.attempt_answer_fmt", en: en)
                            .replacingOccurrences(of: "{0}", with: prompt)
                            .replacingOccurrences(of: "{1}", with: attemptAnswer)
                        attemptAnswer = ""
                        lc.ask(text, allowAdapt: false)
                    }
                    .buttonStyle(.borderedProminent)
                }
                Button(t("lec.continue")) { lc.continueAfterCheckpoint() }
            }
        }
    }

    private func qaRow(_ e: LecQA) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            if !e.images.isEmpty {
                HStack {
                    ForEach(e.images, id: \.self) { path in
                        if let img = NSImage(contentsOfFile: path) {
                            Image(nsImage: img).resizable().scaledToFit().frame(maxHeight: 90)
                                .clipShape(RoundedRectangle(cornerRadius: 6))
                        }
                    }
                }
            }
            Text(e.question).fontWeight(.medium)
            if e.pending {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text(t("lec.thinking")).font(.caption)
                }
            } else if !e.error.isEmpty {
                Text(e.error).font(.caption).foregroundColor(.red)
            } else {
                Text(e.answer).textSelection(.enabled)
                Button(t("lec.speak_answer")) { lc.speakAnswer(e) }
                    .buttonStyle(.link)
                    .font(.caption)
                if !e.suggestKind.isEmpty {
                    Text("\(t("lec.adapt_suggest"))\(L10n.t("common.colon", en: en))\(t("lec.kind_\(e.suggestKind)"))")
                        .font(.subheadline.weight(.semibold))
                    if !e.suggestReason.isEmpty { Text(e.suggestReason).font(.caption) }
                    HStack {
                        Button(t("lec.adapt_apply")) {
                            clearSuggestion(e)
                            if let s = lc.seq[safe: lc.pos] {
                                lc.adapt(slideId: s.slideId, kind: e.suggestKind, reason: e.suggestReason, question: e.question)
                            }
                        }
                        .buttonStyle(.borderedProminent)
                        Button(t("lec.adapt_skip")) { clearSuggestion(e) }
                    }
                }
            }
        }
    }

    private func clearSuggestion(_ e: LecQA) {
        if let i = lc.qa.firstIndex(where: { $0.id == e.id }) { lc.qa[i].suggestKind = "" }
    }

    private var adaptDetails: some View {
        let a = lc.adaptation ?? [:]
        let kind = a["kind"] as? String ?? ""
        let inserted = a["inserted"] as? [String] ?? []
        let insertedTitles = a["inserted_titles"] as? [String] ?? []
        let removed = a["removed_titles"] as? [String] ?? []
        return VStack(alignment: .leading, spacing: 10) {
            Text(t("lec.kind_\(kind)")).font(.headline)
            if let reason = a["reason"] as? String, !reason.isEmpty {
                Text("\(t("lec.reason"))\(L10n.t("common.colon", en: en))\(reason)")
            }
            if !inserted.isEmpty {
                Text(t("lec.inserted")).font(.subheadline.weight(.semibold))
                ForEach(Array(inserted.enumerated()), id: \.offset) { i, id in
                    Button(insertedTitles[safe: i] ?? id) {
                        showAdaptDetails = false
                        if let at = lc.seq.firstIndex(where: { $0.slideId == id }) { lc.jumpTo(at) }
                    }
                    .buttonStyle(.link)
                }
            }
            if !removed.isEmpty {
                Text(t("lec.replaced")).font(.subheadline.weight(.semibold))
                Text(removed.joined(separator: L10n.t("lecture.list_sep", en: en)))
            }
            HStack {
                Button(t("lec.restore_original")) {
                    showAdaptDetails = false
                    lc.undo(all: true)
                }
                Spacer()
                Button(t("lec.undo")) {
                    showAdaptDetails = false
                    lc.undo(all: false)
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .frame(width: 420)
    }
}
