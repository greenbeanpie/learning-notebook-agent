import SwiftUI

/// A request to configure (and then create or update) a Studio artifact.
struct StudioConfigRequest: Identifiable {
    let id = UUID()
    let kind: String
    var initial: [String: Any] = [:]
    var isUpdate = false
    var artifactId = ""
}

/// One option row: a title and a set of exclusive choices (same options as the Windows client).
private struct ConfigOption {
    let key: String
    let headerKey: String
    let choices: [(labelKey: String, value: String)]
    let fallback: String
}

private func options(for kind: String) -> [ConfigOption] {
    let qty = ConfigOption(key: "quantity", headerKey: "studio.cfg.quantity_header",
                           choices: [("studio.cfg.qty_fewer", "fewer"), ("studio.cfg.qty_standard", "standard"), ("studio.cfg.qty_more", "more")],
                           fallback: "standard")
    let diff = ConfigOption(key: "difficulty", headerKey: "studio.cfg.difficulty_header",
                            choices: [("studio.cfg.diff_auto", "auto"), ("studio.cfg.diff_mixed", "mixed"), ("studio.cfg.diff_easy", "easy"),
                                      ("studio.cfg.diff_medium", "medium"), ("studio.cfg.diff_hard", "hard")],
                            fallback: "auto")
    switch kind {
    case "mindmap", "knowledge_graph":
        return [ConfigOption(key: "detail", headerKey: "studio.cfg.detail_header",
                             choices: [("studio.cfg.level_concise", "concise"), ("studio.cfg.level_balanced", "standard"), ("studio.cfg.level_detailed", "detailed")],
                             fallback: "standard")]
    case "quiz", "flashcards":
        return [qty, diff]
    case "report":
        return [
            ConfigOption(key: "format", headerKey: "studio.cfg.format_header",
                         choices: [("studio.cfg.report_doc", "document"), ("studio.cfg.report_interactive", "interactive")], fallback: "document"),
            ConfigOption(key: "depth", headerKey: "studio.cfg.detail_header",
                         choices: [("studio.cfg.level_concise", "brief"), ("studio.cfg.level_balanced", "standard"), ("studio.cfg.level_detailed", "deep")],
                         fallback: "standard"),
        ]
    case "slides":
        return [ConfigOption(key: "length", headerKey: "studio.cfg.slides_length_header",
                             choices: [("studio.cfg.level_concise", "concise"), ("studio.cfg.level_balanced", "standard"), ("studio.cfg.level_detailed", "detailed")],
                             fallback: "standard")]
    case "lecture":
        return [
            ConfigOption(key: "mode", headerKey: "studio.cfg.lecture_mode_header",
                         choices: [("studio.cfg.lecture_mode_general", "general"), ("studio.cfg.lecture_mode_preview", "preview"),
                                   ("studio.cfg.lecture_mode_review", "review"), ("studio.cfg.lecture_mode_problem", "problem")],
                         fallback: "general"),
            ConfigOption(key: "detail", headerKey: "studio.cfg.detail_header",
                         choices: [("studio.cfg.level_auto", "auto"), ("studio.cfg.level_concise", "brief"),
                                   ("studio.cfg.level_balanced", "standard"), ("studio.cfg.level_detailed", "detailed")],
                         fallback: "auto"),
        ]
    case "infographic":
        return [ConfigOption(key: "orientation", headerKey: "studio.cfg.orientation_header",
                             choices: [("studio.cfg.orient_landscape", "landscape"), ("studio.cfg.orient_portrait", "portrait"), ("studio.cfg.orient_square", "square")],
                             fallback: "landscape")]
    case "audio":
        return [
            ConfigOption(key: "format", headerKey: "studio.cfg.format_header",
                         choices: [("studio.cfg.audio_deep_dive", "deep_dive"), ("studio.cfg.audio_debate", "debate"),
                                   ("studio.cfg.audio_brief", "brief"), ("studio.cfg.audio_critique", "critique")],
                         fallback: "deep_dive"),
            ConfigOption(key: "length", headerKey: "studio.cfg.audio_length_header",
                         choices: [("studio.cfg.level_concise", "short"), ("studio.cfg.level_balanced", "standard"), ("studio.cfg.level_detailed", "long")],
                         fallback: "standard"),
        ]
    default:
        return []
    }
}

struct StudioConfigSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var scheme
    let request: StudioConfigRequest
    /// (config, attachments, finish) — `finish` is true when the result should be created without an outline review step.
    let onSubmit: ([String: Any], [String: Any]) -> Void
    let onCancel: () -> Void

    @State private var topic = ""
    @State private var lang = ""
    @State private var chosen: [String: String] = [:]
    @State private var types: Set<String> = ["single", "multi"]
    @State private var refArtifactIds: Set<String> = []
    @StateObject private var attach = AttachmentStore()
    @State private var loaded = false

    private var kind: String { request.kind }
    private var p: AppThemePalette { state.palette(scheme) }
    private var kindName: String { state.t("studio.\(kind)") }
    private var supportsOutlineSplit: Bool { kind == "slides" || kind == "report" }
    private var isLecture: Bool { kind == "lecture" }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Text(request.isUpdate
                     ? L10n.fmt("studio.cfg.title_update", en: state.isEnglish, kindName)
                     : L10n.fmt("studio.cfg.title_create", en: state.isEnglish, kindName))
                    .font(.headline)
                if request.isUpdate {
                    Text(state.t("studio.cfg.update_hint")).font(.caption).foregroundColor(p.textSecondary)
                }

                Text(state.t(isLecture ? "studio.cfg.lecture_topic_header" : "studio.cfg.topic_header"))
                    .font(.subheadline).foregroundColor(p.textSecondary)
                TextEditor(text: $topic)
                    .scrollContentBackground(.hidden)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 8)
                    .frame(maxWidth: .infinity, minHeight: 96, maxHeight: 200)
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(p.strongBorder))
                    .overlay(alignment: .topLeading) {
                        if topic.isEmpty {
                            Text(state.t(isLecture ? "studio.cfg.lecture_topic_placeholder" : "studio.cfg.topic_placeholder"))
                                .foregroundColor(p.textMuted)
                                .padding(.horizontal, 11).padding(.vertical, 8)
                                .allowsHitTesting(false)
                        }
                    }

                HStack(spacing: 8) {
                    AttachMenuButton(store: attach, en: state.isEnglish, tint: p.textSecondary)
                    Text(state.t(request.isUpdate ? "img.cfg_hint_update" : "img.cfg_hint")).font(.caption).foregroundColor(p.textMuted)
                }
                AttachChips(store: attach, en: state.isEnglish)

                Picker(state.t("studio.cfg.lang_header"), selection: $lang) {
                    Text(state.t("studio.cfg.lang_source")).tag("")
                    Text(state.t("studio.cfg.lang_zh")).tag("zh-CN")
                    Text(state.t("studio.cfg.lang_en")).tag("en")
                }

                ForEach(options(for: kind), id: \.key) { opt in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(state.t(opt.headerKey)).font(.subheadline).foregroundColor(p.textSecondary)
                        let selection = Binding(get: { chosen[opt.key] ?? opt.fallback }, set: { chosen[opt.key] = $0 })
                        // Segmented when the labels fit the sheet, a menu otherwise (long labels used to widen the whole sheet).
                        ViewThatFits(in: .horizontal) {
                            Picker("", selection: selection) {
                                ForEach(opt.choices, id: \.value) { c in Text(state.t(c.labelKey)).tag(c.value) }
                            }
                            .pickerStyle(.segmented)
                            .labelsHidden()
                            Picker("", selection: selection) {
                                ForEach(opt.choices, id: \.value) { c in Text(state.t(c.labelKey)).tag(c.value) }
                            }
                            .pickerStyle(.menu)
                            .labelsHidden()
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }

                if kind == "quiz" {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(state.t("studio.cfg.types_header")).font(.subheadline).foregroundColor(p.textSecondary)
                        WrapFlowLayout(spacing: 8) {
                            ForEach([("single", "studio.cfg.type_single"), ("multi", "studio.cfg.type_multi"),
                                     ("fill", "studio.cfg.type_fill"), ("short", "studio.cfg.type_short")], id: \.0) { code, key in
                                Toggle(state.t(key), isOn: Binding(
                                    get: { types.contains(code) },
                                    set: { on in if on { types.insert(code) } else if types.count > 1 { types.remove(code) } }
                                ))
                                .toggleStyle(.button)
                            }
                        }
                    }
                }
                let readyArtifacts = state.artifacts.filter { $0.status == "READY" }
                if !readyArtifacts.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(state.t("studio.cfg.ref_artifacts")).font(.subheadline).foregroundColor(p.textSecondary)
                        WrapFlowLayout(spacing: 8) {
                            ForEach(readyArtifacts, id: \.id) { art in
                                Toggle(art.title, isOn: Binding(
                                    get: { refArtifactIds.contains(art.id) },
                                    set: { on in if on { refArtifactIds.insert(art.id) } else { refArtifactIds.remove(art.id) } }
                                ))
                                .toggleStyle(.button)
                            }
                        }
                    }
                }
                if isLecture {
                    Text(state.t("studio.cfg.lecture_hint")).font(.caption).foregroundColor(p.textMuted)
                }
                if kind == "audio" {
                    Text(state.t("studio.cfg.audio_hint")).font(.caption).foregroundColor(p.textMuted)
                }

                HStack {
                    Spacer()
                    Button(state.t("common.cancel")) { onCancel() }
                        .keyboardShortcut(.cancelAction)
                    if isLecture {
                        Button(state.t("studio.cfg.btn_one_click")) { submit(review: false) }
                        Button(state.t("studio.cfg.btn_lecture_confirm_scope")) { submit(review: true) }
                            .buttonStyle(.borderedProminent)
                    } else if supportsOutlineSplit {
                        Button(state.t("studio.cfg.btn_outline_preview")) { submit(review: true) }
                        Button(state.t("studio.cfg.btn_one_click")) { submit(review: false) }
                            .buttonStyle(.borderedProminent)
                    } else {
                        Button(request.isUpdate ? state.t("studio.cfg.btn_update") : state.t("studio.cfg.btn_create")) { submit(review: false) }
                            .buttonStyle(.borderedProminent)
                    }
                }
            }
            .padding(20)
            .frame(width: 500, alignment: .leading)
        }
        .frame(width: 500)
        .frame(maxHeight: 640)
        .background(p.panelBg)
        .preferredColorScheme(AppThemeId.resolve(state.prefs.theme).preferredColorScheme)
        .attachDrop(attach, en: state.isEnglish)
        .onAppear {
            guard !loaded else { return }
            loaded = true
            topic = request.initial["topic"] as? String ?? ""
            lang = (request.initial["lang"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? state.prefs.defaultStudioLang
            for opt in options(for: kind) {
                if let v = request.initial[opt.key] as? String, !v.isEmpty { chosen[opt.key] = v }
            }
            if let t = request.initial["types"] as? [String], !t.isEmpty { types = Set(t) }
            if let refs = request.initial["ref_artifact_ids"] as? [String], !refs.isEmpty { refArtifactIds = Set(refs) }
        }
    }

    private func value(_ key: String, _ fallback: String) -> String {
        chosen[key] ?? options(for: kind).first(where: { $0.key == key })?.fallback ?? fallback
    }

    private func submit(review: Bool) {
        var cfg: [String: Any] = [:]
        let t = topic.trimmingCharacters(in: .whitespacesAndNewlines)
        if !t.isEmpty { cfg["topic"] = t }
        if !lang.isEmpty { cfg["lang"] = lang }
        if !refArtifactIds.isEmpty { cfg["ref_artifact_ids"] = Array(refArtifactIds) }
        switch kind {
        case "quiz":
            let q = value("quantity", "standard")
            cfg["quantity"] = q
            cfg["count"] = q == "fewer" ? 4 : (q == "more" ? 15 : 8)
            cfg["difficulty"] = value("difficulty", "auto")
            let order = ["single", "multi", "fill", "short"].filter { types.contains($0) }
            if !order.isEmpty { cfg["types"] = order }
        case "mindmap", "knowledge_graph":
            cfg["detail"] = value("detail", "standard")
        case "flashcards":
            let q = value("quantity", "standard")
            cfg["quantity"] = q
            cfg["count"] = q == "fewer" ? 6 : (q == "more" ? 24 : 12)
            cfg["difficulty"] = value("difficulty", "auto")
        case "report":
            cfg["format"] = value("format", "document")
            cfg["depth"] = value("depth", "standard")
            cfg["reviewOutline"] = review
        case "slides":
            cfg["length"] = value("length", "standard")
            cfg["reviewOutline"] = review
        case "infographic":
            cfg["orientation"] = value("orientation", "landscape")
        case "lecture":
            cfg["mode"] = value("mode", "general")
            cfg["detail"] = value("detail", "auto")
            cfg["reviewOutline"] = review
        case "audio":
            cfg["format"] = value("format", "deep_dive")
            cfg["length"] = value("length", "standard")
        default:
            break
        }
        var extra: [String: Any] = [:]
        attach.merge(into: &extra)
        attach.clear()
        onSubmit(cfg, extra)
    }
}
