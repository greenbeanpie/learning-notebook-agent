import SwiftUI

/// One question of a quiz payload.
struct QuizQuestion: Identifiable {
    let id: String
    let number: Int
    let type: String          // single | multi | fill | short
    let prompt: String
    let options: [String]
    let answerIndices: [Int]
    let acceptedAnswers: [String]
    let explanation: String

    init(number: Int, _ q: [String: Any]) {
        self.number = number
        id = q["id"] as? String ?? "q\(number)"
        type = q["type"] as? String ?? "single"
        prompt = q["prompt"] as? String ?? ""
        options = (q["options"] as? [Any] ?? []).compactMap { $0 as? String }
        answerIndices = (q["answer_indices"] as? [Any] ?? []).compactMap { ($0 as? NSNumber)?.intValue }
        acceptedAnswers = (q["accepted_answers"] as? [Any] ?? []).compactMap { $0 as? String }.filter { !$0.isEmpty }
        explanation = q["explanation"] as? String ?? ""
    }

    var isChoice: Bool { type == "single" || type == "multi" }
}

struct QuizResult {
    /// nil for a self-evaluated question (short answer).
    var correct: Bool?
    var selfEval: Bool
    var rubric: String
}

/// Answers, grading and score of one quiz run. Grading is done by the core (`studio.grade_answer`).
@MainActor
final class QuizModel: ObservableObject {
    let questions: [QuizQuestion]
    @Published var single: [String: Int] = [:]
    @Published var multi: [String: Set<Int>] = [:]
    @Published var text: [String: String] = [:]
    @Published var results: [String: QuizResult] = [:]
    @Published var grading: Set<String> = []
    @Published var kgUpdatedNodes: Int = 0
    private var kgSynced = false

    init(payload: [String: Any]) {
        let qs = payload["questions"] as? [[String: Any]] ?? []
        questions = qs.enumerated().map { QuizQuestion(number: $0.offset + 1, $0.element) }
    }

    var submittedCount: Int { results.count }
    var objectiveCount: Int { results.values.filter { !$0.selfEval }.count }
    var correctCount: Int { results.values.filter { $0.correct == true }.count }
    var selfEvalCount: Int { results.values.filter { $0.selfEval }.count }
    var isDone: Bool { !questions.isEmpty && submittedCount >= questions.count }

    func hasAnswer(_ q: QuizQuestion) -> Bool {
        switch q.type {
        case "single": return single[q.id] != nil
        case "multi": return !(multi[q.id] ?? []).isEmpty
        default: return !(text[q.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
    }

    var answeredCount: Int { questions.filter { hasAnswer($0) }.count }

    private func answerValue(_ q: QuizQuestion) -> Any {
        switch q.type {
        case "single": return single[q.id] ?? -1
        case "multi": return (multi[q.id] ?? []).sorted()
        default: return text[q.id] ?? ""
        }
    }

    /// Grades one question; the inputs lock once the core has answered.
    func submit(_ q: QuizQuestion, state: AppState, artifactId: String, notebookId: String) async {
        guard results[q.id] == nil, !grading.contains(q.id) else { return }
        grading.insert(q.id)
        defer { grading.remove(q.id) }
        guard let g = await state.run(
            "studio.grade_answer",
            ["artifact_id": artifactId, "question_id": q.id, "answer": answerValue(q)],
            what: state.t("quiz.op_grade")
        ) else { return }
        let selfEval = g["self_eval"] as? Bool ?? false
        results[q.id] = QuizResult(
            correct: selfEval ? nil : (g["correct"] as? Bool ?? false),
            selfEval: selfEval,
            rubric: g["rubric"] as? String ?? ""
        )
        if isDone { await syncKnowledgeGraph(state: state, artifactId: artifactId, notebookId: notebookId) }
    }

    func submitAll(state: AppState, artifactId: String, notebookId: String) async {
        for q in questions where results[q.id] == nil && hasAnswer(q) {
            await submit(q, state: state, artifactId: artifactId, notebookId: notebookId)
        }
    }

    /// A finished quiz updates the understanding of the knowledge-graph concepts it covers.
    private func syncKnowledgeGraph(state: AppState, artifactId: String, notebookId: String) async {
        guard !kgSynced, !notebookId.isEmpty, !artifactId.isEmpty else { return }
        kgSynced = true
        let rows: [[String: Any]] = questions.map { q in
            let r = results[q.id]
            let ok = r?.correct ?? true
            return ["question_id": q.id, "is_correct": ok, "score": r?.correct == true ? 1.0 : ((r?.selfEval ?? false) ? 0.7 : 0.0)]
        }
        if let res = try? await state.core.call(
            method: "studio.apply_quiz_to_kg",
            params: ["notebook_id": notebookId, "artifact_id": artifactId, "results": rows]
        ), let n = (res["updated_nodes"] as? NSNumber)?.intValue, n > 0 {
            kgUpdatedNodes = n
        }
    }
}

struct QuizView: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var scheme
    let artifactId: String
    let notebookId: String
    let payload: [String: Any]
    var compact: Bool = false

    @StateObject private var model: QuizModel
    @State private var started = false
    @State private var viewMode = "continuous"
    @State private var submitMode = "manual"
    @State private var currentIndex = 0
    @State private var jumpTarget: String? = nil

    init(artifactId: String, notebookId: String, payload: [String: Any], compact: Bool = false) {
        self.artifactId = artifactId
        self.notebookId = notebookId
        self.payload = payload
        self.compact = compact
        _model = StateObject(wrappedValue: QuizModel(payload: payload))
    }

    private var p: AppThemePalette { state.palette(scheme) }

    var body: some View {
        if model.questions.isEmpty {
            Text(state.t("quiz.empty")).foregroundColor(p.textMuted)
        } else {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        if !started {
                            pickerCard
                        } else {
                            workspace
                        }
                    }
                    .padding(.vertical, 4)
                }
                .onChange(of: jumpTarget) { _, id in
                    if let id { withAnimation { proxy.scrollTo(id, anchor: .top) }; jumpTarget = nil }
                }
            }
            .onAppear {
                viewMode = state.prefs.quizViewMode
                submitMode = state.prefs.quizSubmitMode
                if state.prefs.quizModeConfigured { started = true }
            }
        }
    }

    // MARK: Mode picker

    private var pickerCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(state.t("quiz.mode_title")).font(.headline)
            Text(L10n.fmt("quiz.mode_subtitle", en: state.isEnglish, model.questions.count))
                .font(.callout).foregroundColor(p.textSecondary)
            Text(state.t("quiz.view_label")).font(.caption.weight(.semibold)).foregroundColor(p.textMuted)
            choice(state.t("quiz.view_continuous"), state.t("quiz.view_continuous_desc"), viewMode == "continuous") { viewMode = "continuous" }
            choice(state.t("quiz.view_one"), state.t("quiz.view_one_desc"), viewMode == "one_by_one") { viewMode = "one_by_one" }
            Text(state.t("quiz.submit_mode_label")).font(.caption.weight(.semibold)).foregroundColor(p.textMuted)
            choice(state.t("quiz.submit_manual"), state.t("quiz.submit_manual_desc"), submitMode == "manual") { submitMode = "manual" }
            choice(state.t("quiz.submit_auto"), state.t("quiz.submit_auto_desc"), submitMode == "auto") { submitMode = "auto" }
            HStack {
                Spacer()
                Button(state.t("quiz.start")) {
                    state.prefs.quizViewMode = viewMode
                    state.prefs.quizSubmitMode = submitMode
                    state.prefs.quizModeConfigured = true
                    Task { await state.savePreferences() }
                    started = true
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(16)
        .background(p.elevatedBg)
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(p.subtleBorder))
        .cornerRadius(12)
    }

    private func choice(_ title: String, _ desc: String, _ active: Bool, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.system(size: 14, weight: .semibold))
                Text(desc).font(.caption).foregroundColor(p.textSecondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14).padding(.vertical, 10)
            .background(active ? p.accent.opacity(0.16) : p.panelBg)
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(active ? p.accent : p.subtleBorder, lineWidth: active ? 2 : 1))
            .cornerRadius(10)
        }
        .buttonStyle(.plain)
    }

    // MARK: Workspace

    private var workspace: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(model.submittedCount == 0 && submitMode == "manual"
                 ? L10n.fmt("quiz.sheet_progress", en: state.isEnglish, model.answeredCount, model.questions.count)
                 : L10n.fmt("quiz.score_progress", en: state.isEnglish, model.submittedCount, model.questions.count, model.correctCount))
                .font(.callout).foregroundColor(p.textSecondary)

            answerSheet

            if viewMode == "one_by_one" {
                let q = model.questions[min(currentIndex, model.questions.count - 1)]
                Text(L10n.fmt("quiz.one_progress", en: state.isEnglish, currentIndex + 1, model.questions.count, model.submittedCount))
                    .font(.caption).foregroundColor(p.textMuted)
                ProgressView(value: Double(model.submittedCount), total: Double(model.questions.count))
                QuizQuestionCard(model: model, question: q, auto: submitMode == "auto", artifactId: artifactId, notebookId: notebookId, palette: p)
                HStack {
                    Button(state.t("quiz.prev")) { currentIndex = max(0, currentIndex - 1) }
                        .disabled(currentIndex == 0)
                    Spacer()
                    Button(state.t("quiz.next")) { currentIndex = min(model.questions.count - 1, currentIndex + 1) }
                        .disabled(currentIndex >= model.questions.count - 1)
                }
            } else {
                ForEach(model.questions) { q in
                    QuizQuestionCard(model: model, question: q, auto: submitMode == "auto", artifactId: artifactId, notebookId: notebookId, palette: p)
                        .id(q.id)
                }
            }

            if submitMode == "manual" {
                HStack {
                    Spacer()
                    Button(model.isDone ? state.t("quiz.submitted_all") : state.t("quiz.submit_all")) {
                        Task { await model.submitAll(state: state, artifactId: artifactId, notebookId: notebookId) }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.isDone || model.answeredCount == 0)
                }
            }

            if model.isDone { summaryCard }
        }
    }

    private var answerSheet: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(state.t("quiz.sheet_title")).font(.caption.weight(.semibold)).foregroundColor(p.textMuted)
            WrapFlowLayout(spacing: 6) {
                ForEach(model.questions) { q in
                    let r = model.results[q.id]
                    Button {
                        if viewMode == "one_by_one" { currentIndex = q.number - 1 } else { jumpTarget = q.id }
                    } label: {
                        Text("\(q.number)")
                            .font(.system(size: 12, weight: .semibold))
                            .frame(width: 30, height: 28)
                            .foregroundColor(r == nil ? p.textPrimary : Color.black.opacity(0.8))
                            .background(pillColor(q, r))
                            .clipShape(RoundedRectangle(cornerRadius: 8))
                            .overlay(RoundedRectangle(cornerRadius: 8).stroke(viewMode == "one_by_one" && currentIndex == q.number - 1 ? p.accent : p.subtleBorder, lineWidth: viewMode == "one_by_one" && currentIndex == q.number - 1 ? 2 : 1))
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .padding(12)
        .background(p.elevatedBg)
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(p.subtleBorder))
        .cornerRadius(12)
    }

    private func pillColor(_ q: QuizQuestion, _ r: QuizResult?) -> Color {
        if let r {
            if r.selfEval { return Color(hex: 0x8AB4F8) }
            return r.correct == true ? Color(hex: 0x78DC8C) : Color(hex: 0xFF8C82)
        }
        return model.hasAnswer(q) ? p.accent.opacity(0.35) : p.panelBg
    }

    private var summaryCard: some View {
        let obj = model.objectiveCount
        let pct = obj > 0 ? (Double(model.correctCount) * 1000 / Double(obj)).rounded() / 10 : 100
        let tint: Color = obj == 0 ? p.accent : (pct >= 80 ? Color(hex: 0x78DC8C) : (pct >= 60 ? Color(hex: 0xF5BE5A) : Color(hex: 0xFF8C82)))
        return VStack(alignment: .leading, spacing: 8) {
            Text(state.t("quiz.summary_title")).font(.headline)
            ProgressView(value: pct, total: 100).tint(tint)
            Text(obj > 0
                 ? (model.selfEvalCount > 0
                    ? L10n.fmt("quiz.summary_accuracy_with_self", en: state.isEnglish, pct, model.correctCount, obj, model.selfEvalCount)
                    : L10n.fmt("quiz.summary_accuracy", en: state.isEnglish, pct, model.correctCount, obj))
                 : L10n.fmt("quiz.summary_self_only", en: state.isEnglish, model.selfEvalCount))
                .foregroundColor(tint).font(.callout.weight(.semibold))
            if model.kgUpdatedNodes > 0 {
                Text(L10n.fmt("kg.quiz_synced_fmt", en: state.isEnglish, model.kgUpdatedNodes))
                    .font(.caption).foregroundColor(p.textSecondary)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(p.elevatedBg)
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(tint))
        .cornerRadius(12)
    }
}

private struct QuizQuestionCard: View {
    @EnvironmentObject var state: AppState
    @ObservedObject var model: QuizModel
    let question: QuizQuestion
    let auto: Bool
    let artifactId: String
    let notebookId: String
    let palette: AppThemePalette

    private var result: QuizResult? { model.results[question.id] }
    private var locked: Bool { result != nil || model.grading.contains(question.id) }

    private var typeLabel: String {
        state.t("quiz.type_" + question.type)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("\(question.number)")
                    .font(.system(size: 12, weight: .bold))
                    .padding(.horizontal, 7).padding(.vertical, 2)
                    .background(palette.hoverBg).clipShape(Capsule())
                Text(typeLabel).font(.caption).foregroundColor(palette.textMuted)
                Spacer()
            }
            MarkdownView(markdown: question.prompt, fontSize: state.prefs.fontSize, color: palette.textPrimary, mutedColor: palette.textSecondary, accent: palette.accent)
            inputs
            if !auto || !question.isChoice || question.type == "multi" {
                if result == nil {
                    Button(model.grading.contains(question.id) ? "…" : state.t("quiz.submit")) {
                        if question.type == "single" && model.single[question.id] == nil {
                            state.statusMessage = state.t("quiz.select_warning")
                            return
                        }
                        Task { await model.submit(question, state: state, artifactId: artifactId, notebookId: notebookId) }
                    }
                    .buttonStyle(.bordered)
                    .disabled(model.grading.contains(question.id))
                } else {
                    Text(state.t("quiz.submitted")).font(.caption).foregroundColor(palette.textMuted)
                }
            } else if result == nil {
                Text(state.t("quiz.auto_hint")).font(.caption).foregroundColor(palette.textMuted)
            }
            if let r = result { resultView(r) }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.elevatedBg)
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(palette.subtleBorder))
        .cornerRadius(12)
    }

    @ViewBuilder
    private var inputs: some View {
        switch question.type {
        case "single":
            ForEach(Array(question.options.enumerated()), id: \.offset) { i, opt in
                optionRow(i, opt, selected: model.single[question.id] == i) {
                    guard !locked else { return }
                    model.single[question.id] = i
                    if auto { Task { await model.submit(question, state: state, artifactId: artifactId, notebookId: notebookId) } }
                }
            }
        case "multi":
            ForEach(Array(question.options.enumerated()), id: \.offset) { i, opt in
                optionRow(i, opt, selected: (model.multi[question.id] ?? []).contains(i)) {
                    guard !locked else { return }
                    var set = model.multi[question.id] ?? []
                    if set.contains(i) { set.remove(i) } else { set.insert(i) }
                    model.multi[question.id] = set
                }
            }
        case "short":
            TextEditor(text: Binding(get: { model.text[question.id] ?? "" }, set: { model.text[question.id] = $0 }))
                .frame(minHeight: 90, maxHeight: 160)
                .disabled(locked)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(palette.strongBorder))
                .overlay(alignment: .topLeading) {
                    if (model.text[question.id] ?? "").isEmpty {
                        Text(state.t("quiz.placeholder_short")).foregroundColor(palette.textMuted).padding(8).allowsHitTesting(false)
                    }
                }
        default:
            TextField(state.t("quiz.placeholder_fill"), text: Binding(get: { model.text[question.id] ?? "" }, set: { model.text[question.id] = $0 }))
                .textFieldStyle(.roundedBorder)
                .disabled(locked)
        }
    }

    private func optionRow(_ i: Int, _ text: String, selected: Bool, _ action: @escaping () -> Void) -> some View {
        let multi = question.type == "multi"
        let letter = String(Character(UnicodeScalar(UInt8(65 + min(i, 25)))))
        var mark = Color.clear
        if let r = result, !r.selfEval {
            let isAnswer = question.answerIndices.contains(i)
            if isAnswer { mark = Color.green.opacity(0.18) } else if selected { mark = Color.red.opacity(0.15) }
        }
        return Button(action: action) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: multi ? (selected ? "checkmark.square.fill" : "square") : (selected ? "largecircle.fill.circle" : "circle"))
                    .foregroundColor(selected ? palette.accent : palette.textMuted)
                Text("\(letter).").foregroundColor(palette.textMuted)
                MarkdownInlineText(text: text, fontSize: state.prefs.fontSize, color: palette.textPrimary, accent: palette.accent)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 10).padding(.vertical, 7)
            .background(mark)
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(selected ? palette.accent : palette.subtleBorder))
            .cornerRadius(8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private func resultView(_ r: QuizResult) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if r.selfEval {
                MarkdownView(markdown: L10n.fmt("quiz.rubric", en: state.isEnglish, r.rubric), fontSize: 13, color: palette.textPrimary, mutedColor: palette.textSecondary, accent: palette.accent)
                Text(state.t("quiz.self_check")).font(.caption).foregroundColor(palette.textMuted)
            } else {
                let ok = r.correct == true
                Label(ok ? state.t("quiz.correct") : state.t("quiz.wrong"), systemImage: ok ? "checkmark" : "xmark")
                    .font(.callout.weight(.semibold))
                    .foregroundColor(ok ? Color(hex: 0x78DC8C) : Color(hex: 0xFF8C82))
                let ans = question.answerIndices.filter { $0 >= 0 && $0 < question.options.count }
                    .map { "\(String(Character(UnicodeScalar(UInt8(65 + min($0, 25)))))). \(question.options[$0])" }
                let ref = ans.isEmpty ? question.acceptedAnswers.joined(separator: " / ") : ans.joined(separator: "; ")
                if !ok && !ref.isEmpty {
                    MarkdownView(markdown: L10n.fmt("quiz.ref_answer", en: state.isEnglish, ref), fontSize: 13, color: palette.textPrimary, mutedColor: palette.textSecondary, accent: palette.accent)
                }
            }
            if !question.explanation.isEmpty {
                MarkdownView(markdown: L10n.fmt("quiz.explanation", en: state.isEnglish, question.explanation), fontSize: 13, color: palette.textSecondary, mutedColor: palette.textMuted, accent: palette.accent)
            }
        }
    }
}
