import SwiftUI

struct UnifiedSettingsSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.dismiss) var dismiss

    @State private var proto = "chat_completion"
    @State private var baseUrl = ""
    @State private var modelId = ""
    @State private var apiKey = ""
    @State private var statusMsg = ""

    var body: some View {
        VStack(spacing: 12) {
            HStack {
                Text(state.t("settings.title")).font(.headline)
                Spacer()
                Button(state.t("settings.close")) { dismiss() }
            }

            TabView {
                // Tab 1: Appearance & Interface
                Form {
                    Picker(state.t("settings.language"), selection: $state.prefs.language) {
                        Text(state.isEnglish ? "Follow system" : "跟随系统").tag("auto")
                        Text("简体中文").tag("zh-CN")
                        Text("English").tag("en-US")
                    }
                    Picker(state.t("settings.theme"), selection: $state.prefs.theme) {
                        Text(state.isEnglish ? "Dark" : "深色").tag("dark")
                        Text(state.isEnglish ? "Light" : "浅色").tag("light")
                        Text(state.isEnglish ? "System" : "跟随系统").tag("system")
                    }
                    Picker(state.t("settings.font_size"), selection: $state.prefs.fontSize) {
                        Text("13px").tag(13.0)
                        Text("14px").tag(14.0)
                        Text("16px").tag(16.0)
                    }
                    Toggle(state.t("settings.fullscreen_anim"), isOn: $state.prefs.fullscreenAnimation)
                    Button(state.t("settings.save")) {
                        Task { await state.savePreferences() }
                    }
                    .buttonStyle(.borderedProminent)
                }
                .padding(14)
                .tabItem { Label(state.t("settings.tab_appearance"), systemImage: "paintbrush") }

                // Tab 2: General & Features
                Form {
                    Toggle(state.t("settings.expand_thoughts"), isOn: $state.prefs.expandThoughtsByDefault)
                    Toggle(state.t("settings.show_suggestions"), isOn: $state.prefs.showSuggestedQuestions)
                    Picker(state.t("settings.default_studio_lang"), selection: $state.prefs.defaultStudioLang) {
                        Text(state.isEnglish ? "Follow sources" : "跟随来源").tag("")
                        Text("简体中文").tag("zh-CN")
                        Text("English").tag("en")
                    }
                    Button(state.t("settings.save")) {
                        Task { await state.savePreferences() }
                    }
                    .buttonStyle(.borderedProminent)
                }
                .padding(14)
                .tabItem { Label(state.t("settings.tab_general"), systemImage: "slider.horizontal.3") }

                // Tab 3: AI Models (moved into Settings)
                VStack(alignment: .leading, spacing: 10) {
                    Picker(state.isEnglish ? "Protocol" : "协议", selection: $proto) {
                        Text("Chat Completions").tag("chat_completion")
                        Text("Responses API").tag("responses")
                        Text("Anthropic Messages").tag("anthropic")
                        Text("Gemini GenerateContent").tag("gemini")
                    }
                    TextField(state.isEnglish ? "Base URL" : "接口地址 (Base URL)", text: $baseUrl)
                    TextField(state.isEnglish ? "Model ID" : "模型 ID", text: $modelId)
                    SecureField(state.isEnglish ? "API Key (stored in Keychain)" : "API Key（存入 macOS Keychain）", text: $apiKey)
                    if !statusMsg.isEmpty {
                        Text(statusMsg).font(.caption).foregroundColor(.secondary)
                    }
                    HStack {
                        Spacer()
                        Button(state.t("settings.save")) {
                            Task {
                                do {
                                    let prof = try await state.core.call(method: "profile.upsert", params: [
                                        "display_name": "\(proto) / \(modelId)",
                                        "protocol": proto,
                                        "base_url": baseUrl,
                                        "model_id": modelId,
                                        "activate": true
                                    ])
                                    if let kref = prof["keychain_ref"] as? String, !apiKey.isEmpty {
                                        _ = try await state.core.call(method: "secret.set", params: ["keychain_ref": kref, "secret": apiKey])
                                    }
                                    statusMsg = state.t("settings.saved")
                                } catch {
                                    statusMsg = error.localizedDescription
                                }
                            }
                        }
                        .buttonStyle(.borderedProminent)
                    }
                }
                .padding(16)
                .tabItem { Label(state.t("settings.tab_models"), systemImage: "cpu") }

                // Tab 4: Semantic Search
                VStack(alignment: .leading, spacing: 12) {
                    Text(state.isEnglish
                         ? "Semantic search uses a local multilingual embedding model to find relevant passages across phrasings and languages."
                         : "语义检索使用本地多语言嵌入模型，帮助跨语言或不同措辞准确检索相关片段。")
                        .font(.callout)
                        .foregroundColor(.secondary)
                    Button(state.isEnglish ? "Download & Enable Model" : "下载并启用嵌入模型") {
                        Task {
                            _ = try? await state.core.call(method: "embedding.download")
                            statusMsg = state.isEnglish ? "Download started." : "已开始下载。"
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    Spacer()
                }
                .padding(16)
                .tabItem { Label(state.t("settings.tab_embedding"), systemImage: "magnifyingglass") }
            }
        }
        .padding(18)
        .frame(width: 540, height: 420)
    }
}
