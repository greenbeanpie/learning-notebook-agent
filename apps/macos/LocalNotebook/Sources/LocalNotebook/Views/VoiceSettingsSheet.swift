import AVFoundation
import SwiftUI

/// Narration voice of lectures: the system voice (offline) or a cloud TTS service (`tts.set_settings`).
struct VoiceSettingsSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var scheme
    let en: Bool
    /// Called after a successful save so the player drops cached audio and reloads the settings.
    let onSaved: () -> Void
    /// Speaks the preview sentence with the saved settings.
    let onPreview: (String) -> Void
    let onClose: () -> Void

    @State private var engine = "system"
    @State private var systemVoice = ""
    @State private var provider = "qwen"
    @State private var baseUrl = ""
    @State private var model = ""
    @State private var voice = ""
    @State private var apiKey = ""
    @State private var hasKey = false
    @State private var voices: [String] = []
    @State private var loaded = false

    private var p: AppThemePalette { state.palette(scheme) }
    private func t(_ k: String) -> String { L10n.t(k, en: en) }

    private var systemVoices: [AVSpeechSynthesisVoice] {
        AVSpeechSynthesisVoice.speechVoices().sorted { $0.language < $1.language || ($0.language == $1.language && $0.name < $1.name) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(t("tts.title")).font(.headline)
            Picker(t("tts.engine"), selection: $engine) {
                Text(t("tts.engine_system")).tag("system")
                Text(t("tts.engine_cloud")).tag("cloud")
            }
            .pickerStyle(.radioGroup)

            if engine == "system" {
                Picker(t("tts.system_voice"), selection: $systemVoice) {
                    Text(t("tts.default_voice")).tag("")
                    ForEach(systemVoices, id: \.identifier) { v in Text("\(v.name) (\(v.language))").tag(v.identifier) }
                }
            } else {
                Text(t("tts.cloud_warning")).font(.caption).foregroundColor(p.textSecondary)
                Picker(t("tts.provider"), selection: $provider) {
                    Text(t("tts.provider_qwen")).tag("qwen")
                    Text(t("tts.provider_openai")).tag("openai")
                }
                TextField(t("tts.base_url"), text: $baseUrl).textFieldStyle(.roundedBorder)
                TextField(t("tts.model"), text: $model).textFieldStyle(.roundedBorder)
                HStack {
                    TextField(t("tts.voice"), text: $voice).textFieldStyle(.roundedBorder)
                    if !voices.isEmpty {
                        Menu {
                            ForEach(voices, id: \.self) { v in Button(v) { voice = v } }
                        } label: { Image(systemName: "chevron.down") }
                        .menuStyle(.borderlessButton).fixedSize()
                    }
                }
                SecureField(hasKey ? t("tts.api_key_saved") : t("tts.api_key"), text: $apiKey).textFieldStyle(.roundedBorder)
            }

            HStack {
                Button(t("tts.preview")) {
                    Task { if await save() { onPreview(t("tts.preview_text")) } }
                }
                Spacer()
                Button(L10n.t("common.cancel", en: en)) { onClose() }
                Button(t("settings.save")) {
                    Task {
                        if await save() {
                            state.statusMessage = "\(t("tts.title"))\(L10n.t("common.colon", en: en))\(t("tts.saved"))"
                            onClose()
                        }
                    }
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .frame(width: 440)
        .background(p.panelBg)
        .task {
            guard !loaded else { return }
            loaded = true
            guard let s = try? await state.core.call(method: "tts.get_settings") else { return }
            engine = (s["engine"] as? String) == "cloud" ? "cloud" : "system"
            systemVoice = s["system_voice"] as? String ?? ""
            let cloud = s["cloud"] as? [String: Any] ?? [:]
            provider = (cloud["provider"] as? String) == "openai" ? "openai" : "qwen"
            baseUrl = cloud["base_url"] as? String ?? ""
            model = cloud["model"] as? String ?? ""
            voice = cloud["voice"] as? String ?? ""
            hasKey = s["has_api_key"] as? Bool ?? false
            voices = (s["voices"] as? [[String: Any]] ?? []).compactMap { $0["id"] as? String }
        }
    }

    private func save() async -> Bool {
        var params: [String: Any] = [
            "engine": engine,
            "system_voice": systemVoice,
            "provider": provider,
            "base_url": baseUrl.trimmingCharacters(in: .whitespaces),
            "model": model.trimmingCharacters(in: .whitespaces),
            "voice": voice.trimmingCharacters(in: .whitespaces),
        ]
        let key = apiKey.trimmingCharacters(in: .whitespaces)
        if !key.isEmpty { params["api_key"] = key }
        guard await state.run("tts.set_settings", params, what: t("tts.title")) != nil else { return false }
        apiKey = ""
        hasKey = hasKey || !key.isEmpty
        onSaved()
        return true
    }
}
