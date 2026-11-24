import SwiftUI

struct SlideDeckView: View {
    let payload: [String: Any]
    let en: Bool

    @State private var index: Int = 0
    @State private var direction: Int = 1
    @State private var isPlaying: Bool = false
    @State private var showNotes: Bool = false

    private let timer = Timer.publish(every: 5.0, on: .main, in: .common).autoconnect()

    private var slides: [[String: Any]] {
        payload["slides"] as? [[String: Any]] ?? []
    }

    private var deckTitle: String {
        payload["title"] as? String ?? (en ? "Presentation" : "演示文稿")
    }

    var body: some View {
        if slides.isEmpty {
            Text(en ? "No slides available." : "暂无幻灯片内容。")
                .foregroundColor(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            let safeIdx = min(max(0, index), slides.count - 1)
            let slide = slides[safeIdx]
            let layout = (slide["layout"] as? String ?? "bullets").lowercased()
            let isDark = (layout == "title" || layout == "section" || layout == "summary")

            VStack(spacing: 10) {
                // 16:9 Animated Presentation Stage
                ZStack {
                    slideCanvas(slide: slide, idx: safeIdx, isDark: isDark)
                        .id(safeIdx)
                        .transition(
                            .asymmetric(
                                insertion: .move(edge: direction >= 0 ? .trailing : .leading).combined(with: .opacity),
                                removal: .move(edge: direction >= 0 ? .leading : .trailing).combined(with: .opacity)
                            )
                        )
                }
                .aspectRatio(16 / 9, contentMode: .fit)
                .frame(maxWidth: .infinity)
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color.white.opacity(0.12), lineWidth: 1))
                .shadow(color: Color.black.opacity(0.35), radius: 14, y: 6)

                // Playback & Navigation Controls
                HStack(spacing: 12) {
                    Button {
                        isPlaying = false
                        step(-1)
                    } label: {
                        Label(en ? "Previous" : "上一页", systemImage: "chevron.left")
                    }
                    .disabled(safeIdx == 0)

                    Text("\(safeIdx + 1) / \(slides.count)")
                        .font(.caption.monospacedDigit())
                        .foregroundColor(.secondary)

                    Button {
                        isPlaying = false
                        step(1)
                    } label: {
                        Label(en ? "Next" : "下一页", systemImage: "chevron.right")
                    }
                    .disabled(safeIdx >= slides.count - 1)

                    Spacer()

                    Button {
                        isPlaying.toggle()
                    } label: {
                        Label(
                            isPlaying ? (en ? "Pause" : "暂停放映") : (en ? "Play" : "自动放映"),
                            systemImage: isPlaying ? "pause.fill" : "play.fill"
                        )
                    }
                    .buttonStyle(.borderedProminent)

                    Button {
                        withAnimation { showNotes.toggle() }
                    } label: {
                        Label(en ? "Notes" : "备注", systemImage: "note.text")
                    }
                    .buttonStyle(.bordered)
                }

                // Thumbnail Filmstrip
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(Array(slides.enumerated()), id: \.offset) { i, s in
                            Button {
                                isPlaying = false
                                jump(to: i)
                            } label: {
                                Text("\(i + 1). \(s["title"] as? String ?? "")")
                                    .font(.caption2)
                                    .lineLimit(1)
                                    .frame(maxWidth: 130)
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 5)
                                    .background(i == safeIdx ? Color.accentColor.opacity(0.25) : Color.white.opacity(0.06))
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 8)
                                            .stroke(i == safeIdx ? Color.accentColor : Color.white.opacity(0.1), lineWidth: 1)
                                    )
                                    .cornerRadius(8)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }

                if showNotes {
                    let notes = slide["speaker_notes"] as? String ?? ""
                    ScrollView {
                        Text(notes.isEmpty ? (en ? "No speaker notes for this slide." : "本页暂无演讲备注。") : notes)
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .textSelection(.enabled)
                    }
                    .frame(maxHeight: 84)
                    .padding(10)
                    .background(Color.white.opacity(0.05))
                    .cornerRadius(8)
                }
            }
            .onReceive(timer) { _ in
                guard isPlaying else { return }
                if index + 1 < slides.count {
                    step(1)
                } else {
                    isPlaying = false
                }
            }
        }
    }

    private func step(_ delta: Int) {
        let target = min(max(0, index + delta), slides.count - 1)
        jump(to: target)
    }

    private func jump(to target: Int) {
        guard target != index else { return }
        direction = target > index ? 1 : -1
        withAnimation(.spring(response: 0.36, dampingFraction: 0.86)) {
            index = target
        }
    }

    @ViewBuilder
    private func slideCanvas(slide: [String: Any], idx: Int, isDark: Bool) -> some View {
        let layout = (slide["layout"] as? String ?? "bullets").lowercased()
        let title = slide["title"] as? String ?? ""
        let subtitle = slide["subtitle"] as? String ?? ""
        let bullets = slide["bullets"] as? [String] ?? []
        let callout = slide["callout"] as? String ?? ""

        VStack(alignment: .leading, spacing: 10) {
            if !isDark {
                Rectangle().fill(Color(red: 0.11, green: 0.31, blue: 0.85)).frame(height: 4)
            }
            VStack(alignment: .leading, spacing: 8) {
                Text(layout.uppercased())
                    .font(.caption2.bold())
                    .foregroundColor(isDark ? Color(red: 0.22, green: 0.74, blue: 0.97) : Color(red: 0.11, green: 0.31, blue: 0.85))
                Text(title)
                    .font(isDark ? .title.bold() : .title2.bold())
                    .foregroundColor(isDark ? .white : Color(red: 0.06, green: 0.09, blue: 0.16))
                if !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.subheadline)
                        .foregroundColor(isDark ? Color.white.opacity(0.75) : Color.secondary)
                }
                if !isDark { Divider() }

                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        if let formula = slide["formula"] as? String, !formula.isEmpty {
                            Text(MdAttributed.math(formula, MdInlineStyle(fontSize: 20, color: .white, accent: .white, weight: .semibold)))
                                .multilineTextAlignment(.center)
                                .frame(maxWidth: .infinity)
                                .padding(12)
                                .background(Color(red: 0.06, green: 0.09, blue: 0.16))
                                .cornerRadius(8)
                        }
                        if let cards = slide["cards"] as? [[String: Any]], !cards.isEmpty {
                            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 8) {
                                ForEach(Array(cards.prefix(6).enumerated()), id: \.offset) { _, c in
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(c["title"] as? String ?? "").font(.subheadline.bold()).foregroundColor(.primary)
                                        MarkdownInlineText(text: c["body"] as? String ?? "", fontSize: 11, color: .secondary)
                                    }
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(10)
                                    .background(Color.white)
                                    .cornerRadius(8)
                                }
                            }
                        } else if let cols = slide["columns"] as? [[String: Any]], cols.count >= 2 {
                            HStack(alignment: .top, spacing: 10) {
                                ForEach(Array(cols.prefix(2).enumerated()), id: \.offset) { _, c in
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(c["heading"] as? String ?? "").font(.subheadline.bold())
                                        ForEach(c["items"] as? [String] ?? [], id: \.self) { it in
                                            MarkdownInlineText(text: "▸ \(it)", fontSize: 11)
                                        }
                                    }
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(10)
                                    .background(Color.white)
                                    .cornerRadius(8)
                                }
                            }
                        } else if let metrics = slide["metrics"] as? [[String: Any]], !metrics.isEmpty {
                            HStack(spacing: 8) {
                                ForEach(Array(metrics.prefix(4).enumerated()), id: \.offset) { _, m in
                                    VStack(spacing: 4) {
                                        Text(m["value"] as? String ?? "").font(.title2.bold()).foregroundColor(Color(red: 0.11, green: 0.31, blue: 0.85))
                                        Text(m["label"] as? String ?? "").font(.caption.bold())
                                    }
                                    .frame(maxWidth: .infinity)
                                    .padding(10)
                                    .background(Color.white)
                                    .cornerRadius(8)
                                }
                            }
                        }
                        ForEach(bullets, id: \.self) { b in
                            MarkdownInlineText(
                                text: layout == "quote" ? "“\(b)”" : "▸ \(b)",
                                fontSize: 13,
                                color: isDark ? Color.white.opacity(0.85) : Color(red: 0.2, green: 0.25, blue: 0.33)
                            )
                        }
                    }
                }

                if !callout.isEmpty {
                    MarkdownInlineText(text: "★ \(callout)", fontSize: 11, weight: .semibold, color: Color(red: 0.06, green: 0.09, blue: 0.16))
                        .padding(8)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color(red: 0.94, green: 0.96, blue: 0.98))
                        .cornerRadius(6)
                }

                HStack {
                    Text(deckTitle)
                    Spacer()
                    Text("\(idx + 1) / \(slides.count)").bold()
                }
                .font(.caption2)
                .foregroundColor(isDark ? Color.white.opacity(0.6) : .secondary)
            }
            .padding(18)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(isDark ? Color(red: 0.06, green: 0.09, blue: 0.16) : Color(red: 0.97, green: 0.98, blue: 0.99))
    }
}
