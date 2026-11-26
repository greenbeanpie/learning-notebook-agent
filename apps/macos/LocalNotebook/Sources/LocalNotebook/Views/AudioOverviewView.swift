import AVFoundation
import SwiftUI

/// Plays the generated `overview.wav` and highlights the turn being spoken; the script stays readable.
@MainActor
final class AudioOverviewPlayer: ObservableObject {
    @Published var isPlaying = false
    @Published var position: Double = 0
    @Published var duration: Double = 0
    private var player: AVAudioPlayer?
    private var timer: Timer?

    func load(path: String) -> Bool {
        guard FileManager.default.fileExists(atPath: path),
              let p = try? AVAudioPlayer(contentsOf: URL(fileURLWithPath: path)) else { return false }
        p.prepareToPlay()
        player = p
        duration = p.duration
        return true
    }

    func toggle() {
        guard let player else { return }
        if player.isPlaying { player.pause(); stopTimer() } else { player.play(); startTimer() }
        isPlaying = player.isPlaying
    }

    func seek(_ t: Double) {
        player?.currentTime = max(0, min(duration, t))
        position = player?.currentTime ?? 0
    }

    func stop() {
        player?.stop()
        stopTimer()
        isPlaying = false
    }

    private func startTimer() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 0.3, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self, let p = self.player else { return }
                self.position = p.currentTime
                if !p.isPlaying { self.isPlaying = false; self.stopTimer() }
            }
        }
    }

    private func stopTimer() { timer?.invalidate(); timer = nil }
    deinit { timer?.invalidate() }
}

struct AudioOverviewView: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var scheme
    let artifact: StudioArtifactItem
    @StateObject private var player = AudioOverviewPlayer()
    @State private var hasAudio = false

    private var p: AppThemePalette { state.palette(scheme) }
    private var turns: [[String: Any]] { artifact.payload["turns"] as? [[String: Any]] ?? [] }

    private var roles: [String: String] {
        var d: [String: String] = [:]
        for sp in artifact.payload["speakers"] as? [[String: Any]] ?? [] {
            if let id = sp["id"] as? String, let role = sp["role"] as? String, !id.isEmpty, !role.isEmpty { d[id] = role }
        }
        return d
    }

    /// Index of the turn being spoken, estimated from the share of text already played.
    private var activeTurn: Int? {
        guard player.isPlaying, player.duration > 0 else { return nil }
        let total = max(1, turns.reduce(0) { $0 + (($1["text"] as? String)?.count ?? 0) })
        let spoken = player.position / player.duration * Double(total)
        var acc = 0.0
        for (i, t) in turns.enumerated() {
            let n = Double((t["text"] as? String)?.count ?? 0)
            acc += n
            if spoken <= acc && spoken > acc - n { return i }
        }
        return nil
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if hasAudio {
                    HStack(spacing: 12) {
                        Button { player.toggle() } label: {
                            Image(systemName: player.isPlaying ? "pause.circle.fill" : "play.circle.fill").font(.system(size: 30))
                        }
                        .buttonStyle(.plain)
                        Slider(value: Binding(get: { player.position }, set: { player.seek($0) }), in: 0...max(player.duration, 0.1))
                        Text("\(timeText(player.position)) / \(timeText(player.duration))")
                            .font(.caption.monospacedDigit()).foregroundColor(p.textMuted)
                    }
                    .padding(12)
                    .background(p.elevatedBg)
                    .cornerRadius(12)
                } else {
                    Text(state.t("audio.missing_wav")).foregroundColor(Color(hex: 0xFFC878)).font(.callout)
                }
                let active = activeTurn
                ForEach(Array(turns.enumerated()), id: \.offset) { i, t in
                    let who = t["speaker"] as? String ?? ""
                    MarkdownView(
                        markdown: "**\(roles[who] ?? who)\(L10n.t("common.colon", en: state.isEnglish))**\(t["text"] as? String ?? "")",
                        fontSize: 14, color: p.textPrimary, mutedColor: p.textSecondary, accent: p.accent
                    )
                    .opacity(active == nil || active == i ? 1 : 0.5)
                    .textSelection(.enabled)
                }
            }
            .padding(.vertical, 4)
        }
        .onAppear {
            if !artifact.outputDir.isEmpty {
                hasAudio = player.load(path: (artifact.outputDir as NSString).appendingPathComponent("overview.wav"))
            }
        }
        .onDisappear { player.stop() }
    }

    private func timeText(_ s: Double) -> String {
        let t = Int(s.isFinite ? s : 0)
        return String(format: "%d:%02d", t / 60, t % 60)
    }
}
