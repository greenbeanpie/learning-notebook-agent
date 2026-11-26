import SwiftUI

/// Readable (Markdown + math) views for Studio artifacts that previously showed raw JSON.

struct ReportSectionView: View {
    let section: [String: Any]
    let en: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            MarkdownView(markdown: section["body_md"] as? String ?? "")
            let takeaways = (section["key_takeaways"] as? [String] ?? []).filter { !$0.isEmpty }
            if !takeaways.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text(L10n.t("readable.key_points", en: en)).font(.callout.weight(.semibold))
                    MarkdownView(markdown: takeaways.map { "- \($0)" }.joined(separator: "\n"), fontSize: 13)
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.white.opacity(0.05))
                .cornerRadius(10)
            }
        }
    }
}

struct QuizReadableView: View {
    let payload: [String: Any]
    let en: Bool
    @State private var revealed: Set<Int> = []

    private var questions: [[String: Any]] { payload["questions"] as? [[String: Any]] ?? [] }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                ForEach(Array(questions.enumerated()), id: \.offset) { qi, q in
                    questionCard(qi, q)
                }
            }
            .padding(.vertical, 4)
        }
    }

    private func questionCard(_ qi: Int, _ q: [String: Any]) -> some View {
        let options = q["options"] as? [String] ?? []
        let correct = Set(q["answer_indices"] as? [Int] ?? [])
        let isRevealed = revealed.contains(qi)
        return VStack(alignment: .leading, spacing: 8) {
            MarkdownView(markdown: "**\(qi + 1).** " + (q["prompt"] as? String ?? ""))
            ForEach(Array(options.enumerated()), id: \.offset) { oi, opt in
                let right = isRevealed && correct.contains(oi)
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text("\(String(Character(UnicodeScalar(UInt8(65 + min(oi, 25)))))).").foregroundColor(.secondary)
                    MarkdownInlineText(text: opt)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(right ? Color.green.opacity(0.18) : Color.clear)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(right ? Color.green : Color.secondary.opacity(0.3), lineWidth: 1))
                .cornerRadius(8)
            }
            Button(isRevealed ? (L10n.t("readable.hide_answer", en: en)) : (L10n.t("readable.show_answer", en: en))) {
                if isRevealed { revealed.remove(qi) } else { revealed.insert(qi) }
            }
            .buttonStyle(.borderless)
            if isRevealed {
                let accepted = (q["accepted_answers"] as? [String] ?? []).filter { !$0.isEmpty }
                if !accepted.isEmpty {
                    MarkdownView(markdown: (L10n.t("readable.answer", en: en)) + accepted.joined(separator: " / "))
                }
                if let rubric = q["rubric"] as? String, !rubric.isEmpty {
                    MarkdownView(markdown: rubric, fontSize: 13)
                }
                if let explanation = q["explanation"] as? String, !explanation.isEmpty {
                    MarkdownView(markdown: explanation, fontSize: 13, color: .secondary)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.05))
        .cornerRadius(12)
    }
}

struct FlashcardsReadableView: View {
    let payload: [String: Any]
    let en: Bool
    @State private var flipped: Set<Int> = []

    private var cards: [[String: Any]] { payload["cards"] as? [[String: Any]] ?? [] }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                Text(L10n.t("readable.click_a_card_to_flip", en: en)).font(.caption).foregroundColor(.secondary)
                ForEach(Array(cards.enumerated()), id: \.offset) { i, card in
                    let isFlipped = flipped.contains(i)
                    VStack(alignment: .leading, spacing: 6) {
                        Text("\(i + 1) / \(cards.count) · " + (isFlipped ? (L10n.t("readable.back", en: en)) : (L10n.t("readable.front", en: en))))
                            .font(.caption2)
                            .foregroundColor(.secondary)
                        MarkdownView(markdown: card[isFlipped ? "back" : "front"] as? String ?? "")
                        if !isFlipped, let hint = card["hint"] as? String, !hint.isEmpty {
                            MarkdownView(markdown: (L10n.t("readable.hint", en: en)) + hint, fontSize: 12, color: .secondary)
                        }
                    }
                    .padding(14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(isFlipped ? Color.accentColor.opacity(0.15) : Color.white.opacity(0.05))
                    .cornerRadius(12)
                    .contentShape(Rectangle())
                    .onTapGesture {
                        if isFlipped { flipped.remove(i) } else { flipped.insert(i) }
                    }
                }
            }
            .padding(.vertical, 4)
        }
    }
}

struct AudioScriptReadableView: View {
    let payload: [String: Any]

    var body: some View {
        let turns = payload["turns"] as? [[String: Any]] ?? []
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(Array(turns.enumerated()), id: \.offset) { _, t in
                    Text(t["speaker"] as? String ?? "").font(.caption.weight(.semibold)).foregroundColor(.accentColor)
                    MarkdownView(markdown: t["text"] as? String ?? "")
                }
            }
            .padding(.vertical, 4)
        }
    }
}
