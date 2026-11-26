import SwiftUI

/// Flip cards, mark each as known or not, then review the missed ones (same flow as the Windows client).
struct FlashcardsView: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var scheme
    let payload: [String: Any]
    var compact: Bool = false

    @State private var order: [Int] = []
    @State private var index = 0
    @State private var flipped = false
    @State private var known = 0
    @State private var unknown: [Int] = []
    @State private var ready = false

    private var cards: [[String: Any]] { payload["cards"] as? [[String: Any]] ?? [] }
    private var p: AppThemePalette { state.palette(scheme) }
    private var finished: Bool { index >= order.count }

    var body: some View {
        if cards.isEmpty {
            Text(state.t("fc.empty")).foregroundColor(p.textMuted)
        } else {
            ScrollView {
                VStack(spacing: 14) {
                    if finished {
                        Text(unknown.isEmpty
                             ? L10n.fmt("fc.done_all", en: state.isEnglish, known, cards.count)
                             : L10n.fmt("fc.done_round", en: state.isEnglish, known, unknown.count))
                            .font(.system(size: 18, weight: .medium))
                            .multilineTextAlignment(.center)
                            .frame(maxWidth: .infinity, minHeight: compact ? 150 : 220)
                            .padding(24)
                            .background(p.elevatedBg)
                            .overlay(RoundedRectangle(cornerRadius: 14).stroke(p.subtleBorder))
                            .cornerRadius(14)
                        Button(unknown.isEmpty ? state.t("fc.restart") : state.t("fc.review_unknown")) { nextRound() }
                            .buttonStyle(.borderedProminent)
                    } else {
                        let c = cards[order[index]]
                        Text("\(index + 1) / \(order.count)").font(.callout).foregroundColor(p.textSecondary)
                        VStack(spacing: 8) {
                            MarkdownView(
                                markdown: (flipped ? c["back"] : c["front"]) as? String ?? "",
                                fontSize: compact ? 17 : 19,
                                color: p.textPrimary, mutedColor: p.textSecondary, accent: p.accent
                            )
                            .multilineTextAlignment(.center)
                        }
                        .frame(maxWidth: .infinity, minHeight: compact ? 150 : 220)
                        .padding(compact ? 20 : 28)
                        .background(flipped ? p.accent.opacity(0.12) : p.elevatedBg)
                        .overlay(RoundedRectangle(cornerRadius: 14).stroke(p.subtleBorder))
                        .cornerRadius(14)
                        .contentShape(Rectangle())
                        .onTapGesture { flipped.toggle() }
                        if !flipped, let hint = c["hint"] as? String, !hint.isEmpty {
                            Text(L10n.fmt("fc.hint", en: state.isEnglish, hint)).font(.callout).foregroundColor(p.textMuted)
                        }
                        HStack(spacing: 10) {
                            Button(state.t("fc.dont_know")) { advance(knew: false) }.buttonStyle(.bordered)
                            Button(flipped ? state.t("fc.show_front") : state.t("fc.flip")) { flipped.toggle() }.buttonStyle(.bordered)
                            Button(state.t("fc.know")) { advance(knew: true) }.buttonStyle(.borderedProminent)
                        }
                    }
                }
                .frame(maxWidth: 640)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 6)
            }
            .onAppear { if !ready { order = Array(cards.indices); ready = true } }
        }
    }

    private func advance(knew: Bool) {
        if knew { known += 1 } else { unknown.append(order[index]) }
        index += 1
        flipped = false
    }

    private func nextRound() {
        order = unknown.isEmpty ? Array(cards.indices) : unknown
        unknown = []
        known = 0
        index = 0
        flipped = false
    }
}
