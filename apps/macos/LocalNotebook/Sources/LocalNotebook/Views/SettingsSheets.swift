import SwiftUI

struct ProviderPresetItem: Identifiable {
    let id: String
    let name: String
    let nameEn: String
    let protocolName: String
    let baseUrl: String
    let defaultModel: String
    let suggestedModels: [String]
    let contextWindow: Int
    let maxOutputTokens: Int
    let supportsVision: Bool
    let supportsAudio: Bool
    let supportsVideo: Bool
    let extraHeaders: [String: String]
}

struct ModelProfileItem: Identifiable {
    let id: String
    let displayName: String
    let protocolName: String
    let baseUrl: String
    let modelId: String
    let keychainRef: String
    let hasSecret: Bool
    let isActive: Bool
    let allowInsecureHttp: Bool
    let showThinking: Bool
    let effort: String
    let contextWindow: Int
    let maxOutputTokens: Int
    let inputPricePerM: Double?
    let outputPricePerM: Double?
    let supportsVision: Bool
    let supportsAudio: Bool
    let supportsVideo: Bool
    let extraHeaders: [String: String]
    /// false: probed and lacking tool calling or structured output (cannot be used)
    var eligible: Bool = true
    var unsupportedReason: String? = nil
}

struct UnifiedSettingsSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.dismiss) var dismiss
    @Environment(\.colorScheme) private var systemColorScheme

    // AI Models tab state
    @State private var profiles: [ModelProfileItem] = []
    @State private var presets: [ProviderPresetItem] = []
    @State private var selectedProfileId: String = ""
    @State private var selectedPresetId: String = ""
    @State private var profileId: String? = nil
    @State private var keychainRef: String = ""
    @State private var hasSecret: Bool = false

    @State private var proto = "chat_completion"
    @State private var baseUrl = ""
    @State private var modelId = ""
    @State private var modelCandidates: [String] = []
    @State private var apiKey = ""
    @State private var allowInsecureHttp = false
    @State private var activateChat = true
    @State private var supportsVision = false
    @State private var supportsAudio = false
    @State private var supportsVideo = false
    @State private var audioVideoEnabled = true
    @State private var visionProfileId = ""
    @State private var audioProfileId = ""
    @State private var videoProfileId = ""
    @State private var showThinking = true
    @State private var effort = ""
    @State private var contextWindowText = "32000"
    @State private var maxOutputTokensText = "8000"
    @State private var priceInText = ""
    @State private var priceOutText = ""
    @State private var extraHeaders: [(id: UUID, key: String, value: String)] = []
    @State private var showAdvanced = false
    @State private var statusMsg = ""
    @State private var isTesting = false
    @State private var isDiscovering = false

    // Semantic Search tab state
    @State private var embStatusText = ""
    @State private var embInstalled = false
    @State private var embSizeMb = 0
    @State private var embActive = false
    @State private var embRunning = false

    private var themeId: AppThemeId { AppThemeId.resolve(state.prefs.theme) }
    private var palette: AppThemePalette { themeId.palette(systemScheme: systemColorScheme) }

    var body: some View {
        VStack(spacing: 12) {
            HStack(spacing: 10) {
                AppLogoView(size: 24, accent: palette.accent)
                Text(state.t("settings.title"))
                    .font(.headline)
                    .foregroundColor(palette.textPrimary)
                Spacer()
                Button(state.t("settings.close")) { dismiss() }
                    .buttonStyle(.plain)
                    .font(.callout)
                    .foregroundColor(palette.textSecondary)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(palette.elevatedBg)
                    .overlay(Capsule().stroke(palette.subtleBorder, lineWidth: 1))
                    .clipShape(Capsule())
            }

            TabView {
                appearanceTab
                    .tabItem { Label(state.t("settings.tab_appearance"), systemImage: "paintbrush") }

                generalTab
                    .tabItem { Label(state.t("settings.tab_general"), systemImage: "slider.horizontal.3") }

                aiModelsTab
                    .tabItem { Label(state.t("settings.tab_models"), systemImage: "cpu") }

                embeddingTab
                    .tabItem { Label(state.t("settings.tab_embedding"), systemImage: "magnifyingglass") }
            }
        }
        .padding(18)
        .frame(width: 640, height: 600)
        .background(palette.panelBg)
        .preferredColorScheme(themeId.preferredColorScheme)
        .task {
            await loadModelsData()
            await refreshEmbeddingStatus()
        }
        .onChange(of: state.embeddingDownloadRunning) { _, _ in
            Task { await refreshEmbeddingStatus() }
        }
    }

    // MARK: - Tab 1: Appearance & Interface

    private var appearanceTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                VStack(alignment: .leading, spacing: 8) {
                    Text(state.t("settings.theme"))
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(palette.textPrimary)

                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
                        ForEach(AppThemeId.allCases) { option in
                            ThemeSwatchCard(
                                option: option,
                                isSelected: themeId == option,
                                en: state.isEnglish,
                                systemScheme: systemColorScheme,
                                activePalette: palette
                            ) {
                                state.prefs.theme = option.rawValue
                                Task { await state.savePreferences() }
                            }
                        }
                    }
                }

                Divider().opacity(0.35)

                Form {
                    Picker(state.t("settings.language"), selection: $state.prefs.language) {
                        Text(state.isEnglish ? "Follow system" : "跟随系统").tag("auto")
                        Text("简体中文").tag("zh-CN")
                        Text("English").tag("en-US")
                    }
                    .onChange(of: state.prefs.language) { _, _ in
                        Task { await state.savePreferences() }
                    }

                    Picker(state.t("settings.font_size"), selection: $state.prefs.fontSize) {
                        Text(state.isEnglish ? "Compact (14px)" : "紧凑 (14px)").tag(14.0)
                        Text(state.isEnglish ? "Normal (15px)" : "标准 (15px)").tag(15.0)
                        Text(state.isEnglish ? "Large (17px)" : "大号 (17px)").tag(17.0)
                    }
                    .onChange(of: state.prefs.fontSize) { _, _ in
                        Task { await state.savePreferences() }
                    }

                    Toggle(state.t("settings.fullscreen_anim"), isOn: $state.prefs.fullscreenAnimation)
                        .onChange(of: state.prefs.fullscreenAnimation) { _, _ in
                            Task { await state.savePreferences() }
                        }
                }
                .formStyle(.grouped)
                .scrollContentBackground(.hidden)
                .frame(height: 148)
            }
            .padding(14)
        }
    }

    // MARK: - Tab 2: General & Features

    private var generalTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Form {
                    Toggle(state.t("settings.expand_thoughts"), isOn: $state.prefs.expandThoughtsByDefault)
                        .onChange(of: state.prefs.expandThoughtsByDefault) { _, _ in
                            Task { await state.savePreferences() }
                        }
                    Toggle(state.t("settings.show_suggestions"), isOn: $state.prefs.showSuggestedQuestions)
                        .onChange(of: state.prefs.showSuggestedQuestions) { _, _ in
                            Task { await state.savePreferences() }
                        }
                    Picker(state.t("settings.default_studio_lang"), selection: $state.prefs.defaultStudioLang) {
                        Text(state.isEnglish ? "Follow sources" : "跟随来源").tag("")
                        Text("简体中文").tag("zh-CN")
                        Text("English").tag("en")
                    }
                    .onChange(of: state.prefs.defaultStudioLang) { _, _ in
                        Task { await state.savePreferences() }
                    }
                    Picker(state.t("settings.quiz_view_mode"), selection: $state.prefs.quizViewMode) {
                        Text(state.t("quiz.view_continuous")).tag("continuous")
                        Text(state.t("quiz.view_one")).tag("one_by_one")
                    }
                    .onChange(of: state.prefs.quizViewMode) { _, _ in
                        Task { await state.savePreferences() }
                    }
                    Picker(state.t("settings.quiz_submit_mode"), selection: $state.prefs.quizSubmitMode) {
                        Text(state.t("quiz.submit_manual")).tag("manual")
                        Text(state.t("quiz.submit_auto")).tag("auto")
                    }
                    .onChange(of: state.prefs.quizSubmitMode) { _, _ in
                        Task { await state.savePreferences() }
                    }
                }
                .formStyle(.grouped)
                .scrollContentBackground(.hidden)
                .frame(height: 230)

                HStack(spacing: 10) {
                    Button(state.t("nb.privacy")) {
                        state.showPrivacySheet = true
                    }
                    .buttonStyle(.bordered)

                    Button(state.t("nb.export_diag")) {
                        state.exportDiagnostics()
                    }
                    .buttonStyle(.bordered)

                    Spacer()
                }
                .padding(.horizontal, 14)
            }
            .padding(.vertical, 8)
        }
    }

    // MARK: - Tab 3: AI Models (synced with Windows MainWindow.Settings.cs)

    private var aiModelsTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                // Saved Profiles row + New / Delete
                HStack(spacing: 8) {
                    Picker(state.isEnglish ? "Saved Profiles" : "已保存的配置", selection: $selectedProfileId) {
                        Text(state.isEnglish ? "— New Profile —" : "— 新建配置 —").tag("")
                        ForEach(profiles) { p in
                            Text(profileDisplayLabel(p)).tag(p.id)
                        }
                    }
                    .onChange(of: selectedProfileId) { _, newId in
                        if let found = profiles.first(where: { $0.id == newId }) {
                            fillProfile(found)
                        } else if newId.isEmpty {
                            fillProfile(nil)
                        }
                    }

                    Button(state.isEnglish ? "New" : "新建") {
                        selectedProfileId = ""
                        selectedPresetId = ""
                        fillProfile(nil)
                    }
                    .buttonStyle(.bordered)

                    Button(state.isEnglish ? "Delete" : "删除", role: .destructive) {
                        Task { await deleteCurrentProfile() }
                    }
                    .buttonStyle(.bordered)
                    .disabled(profileId == nil)
                }

                // Provider Preset (Quick Fill)
                if !presets.isEmpty {
                    Picker(state.isEnglish ? "Provider Preset" : "AI 提供商预设", selection: $selectedPresetId) {
                        Text(state.isEnglish ? "Choose a preset to auto-fill..." : "选择预设以自动填充协议、地址与模型…").tag("")
                        ForEach(presets) { pr in
                            Text(state.isEnglish ? pr.nameEn : pr.name).tag(pr.id)
                        }
                    }
                    .onChange(of: selectedPresetId) { _, pid in
                        if let pr = presets.first(where: { $0.id == pid }) {
                            applyPreset(pr)
                        }
                    }
                }

                Divider().opacity(0.35)

                // Protocol + Base URL
                Picker(state.isEnglish ? "Protocol" : "协议", selection: $proto) {
                    Text("chat_completion (OpenAI Compatible)").tag("chat_completion")
                    Text("responses (OpenAI Responses)").tag("responses")
                    Text("anthropic (Anthropic Messages)").tag("anthropic")
                    Text("gemini (Gemini GenerateContent)").tag("gemini")
                }

                VStack(alignment: .leading, spacing: 4) {
                    Text(state.isEnglish ? "Base URL" : "接口地址 (Base URL)")
                        .font(.caption).foregroundColor(palette.textSecondary)
                    TextField("https://api.example.com/v1", text: $baseUrl)
                        .textFieldStyle(.roundedBorder)
                }

                // Model ID + Candidates menu + Fetch Models
                VStack(alignment: .leading, spacing: 4) {
                    Text(state.isEnglish ? "Model ID" : "模型 ID")
                        .font(.caption).foregroundColor(palette.textSecondary)
                    HStack(spacing: 8) {
                        TextField("gpt-4o / claude-sonnet-4 / gemini-2.5-flash", text: $modelId)
                            .textFieldStyle(.roundedBorder)

                        if !modelCandidates.isEmpty {
                            Menu {
                                ForEach(modelCandidates, id: \.self) { m in
                                    Button(m) { modelId = m }
                                }
                            } label: {
                                Image(systemName: "chevron.down.circle")
                                    .font(.system(size: 15))
                            }
                            .menuStyle(.borderlessButton)
                            .fixedSize()
                            .help(state.isEnglish ? "Choose model" : "选择模型")
                        }

                        Button(isDiscovering ? (state.isEnglish ? "Fetching..." : "获取中…") : (state.isEnglish ? "Fetch Models" : "获取模型列表")) {
                            Task { await discoverModels() }
                        }
                        .buttonStyle(.bordered)
                        .disabled(isDiscovering)
                    }
                }

                // API Key
                VStack(alignment: .leading, spacing: 4) {
                    Text("API Key")
                        .font(.caption).foregroundColor(palette.textSecondary)
                    SecureField(
                        hasSecret
                            ? (state.isEnglish ? "Saved in Keychain (leave blank to keep)" : "已存入 Keychain，留空则保持不变")
                            : (state.isEnglish ? "Enter API Key (optional for local models)" : "输入 API Key（本地模型可留空）"),
                        text: $apiKey
                    )
                    .textFieldStyle(.roundedBorder)
                }

                // Toggles
                VStack(alignment: .leading, spacing: 6) {
                    Toggle(state.isEnglish ? "Allow non-loopback plaintext HTTP (trusted local network only)" : "允许非回环 HTTP 明文传输（仅用于可信本地网络）", isOn: $allowInsecureHttp)
                    Toggle(state.isEnglish ? "Set as default chat model" : "设为当前默认对话模型", isOn: $activateChat)
                    HStack(spacing: 16) {
                        Toggle(state.isEnglish ? "Vision (images/PDF)" : "图文视觉", isOn: $supportsVision)
                        Toggle(state.isEnglish ? "Audio" : "音频", isOn: $supportsAudio)
                        Toggle(state.isEnglish ? "Video" : "视频", isOn: $supportsVideo)
                    }
                }
                .font(.callout)

                // Advanced Options
                DisclosureGroup(state.isEnglish ? "Advanced Options" : "高级选项", isExpanded: $showAdvanced) {
                    VStack(alignment: .leading, spacing: 10) {
                        Toggle(state.isEnglish ? "Show model thinking process" : "显示模型的思考过程（如果模型提供）", isOn: $showThinking)

                        Picker(state.isEnglish ? "Reasoning Effort" : "推理强度", selection: $effort) {
                            Text(state.isEnglish ? "Default" : "默认").tag("")
                            Text("low").tag("low")
                            Text("medium").tag("medium")
                            Text("high").tag("high")
                        }

                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(state.isEnglish ? "Context Window" : "上下文窗口 (tokens)")
                                    .font(.caption).foregroundColor(palette.textSecondary)
                                TextField("32000", text: $contextWindowText)
                                    .textFieldStyle(.roundedBorder)
                            }
                            VStack(alignment: .leading, spacing: 4) {
                                Text(state.isEnglish ? "Max Output Tokens" : "单次最大输出")
                                    .font(.caption).foregroundColor(palette.textSecondary)
                                TextField("8000", text: $maxOutputTokensText)
                                    .textFieldStyle(.roundedBorder)
                            }
                        }

                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(state.isEnglish ? "Input price ($/1M)" : "输入单价 ($/百万)")
                                    .font(.caption).foregroundColor(palette.textSecondary)
                                TextField(state.isEnglish ? "Optional" : "可留空", text: $priceInText)
                                    .textFieldStyle(.roundedBorder)
                            }
                            VStack(alignment: .leading, spacing: 4) {
                                Text(state.isEnglish ? "Output price ($/1M)" : "输出单价 ($/百万)")
                                    .font(.caption).foregroundColor(palette.textSecondary)
                                TextField(state.isEnglish ? "Optional" : "可留空", text: $priceOutText)
                                    .textFieldStyle(.roundedBorder)
                            }
                        }

                        // Extra headers
                        HStack {
                            Text(state.isEnglish ? "Extra Headers" : "额外请求头")
                                .font(.caption).foregroundColor(palette.textSecondary)
                            Spacer()
                            Button(state.isEnglish ? "+ Add Header" : "+ 添加请求头") {
                                extraHeaders.append((id: UUID(), key: "", value: ""))
                            }
                            .font(.caption)
                            .buttonStyle(.bordered)
                        }
                        ForEach($extraHeaders, id: \.id) { $hdr in
                            HStack(spacing: 6) {
                                TextField(state.isEnglish ? "Header name" : "请求头名称", text: $hdr.key)
                                    .textFieldStyle(.roundedBorder)
                                TextField(state.isEnglish ? "Value" : "请求头值", text: $hdr.value)
                                    .textFieldStyle(.roundedBorder)
                                Button {
                                    extraHeaders.removeAll { $0.id == hdr.id }
                                } label: {
                                    Image(systemName: "trash").foregroundColor(.red)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                    .padding(.top, 8)
                }

                // Media Processing & Routing Summary
                VStack(alignment: .leading, spacing: 8) {
                    Toggle(
                        state.isEnglish
                            ? "Enable native audio/video processing"
                            : "允许处理音视频文件（直接交给原生支持音视频的 AI 处理）",
                        isOn: $audioVideoEnabled
                    )
                    .font(.callout)
                    .onChange(of: audioVideoEnabled) { _, _ in
                        Task { await saveMediaProcessingSettings() }
                    }

                    Divider().opacity(0.35)

                    Text(state.isEnglish ? "Per-Modality Model Assignment" : "分模态模型指定（图文 / 音频 / 视频可使用不同模型）")
                        .font(.caption.weight(.semibold))
                        .foregroundColor(palette.textPrimary)

                    let visionProfiles = profiles.filter { $0.supportsVision }
                    Picker(state.isEnglish ? "Vision (Image/PDF)" : "图文处理模型", selection: $visionProfileId) {
                        Text(state.isEnglish ? "Auto (prefer default chat)" : "自动（优先默认对话模型）").tag("")
                        ForEach(visionProfiles) { p in
                            Text(p.displayName).tag(p.id)
                        }
                    }
                    .onChange(of: visionProfileId) { _, _ in
                        Task { await saveMediaProcessingSettings() }
                    }

                    let audioProfiles = profiles.filter { $0.supportsAudio }
                    Picker(state.isEnglish ? "Audio" : "音频处理模型", selection: $audioProfileId) {
                        Text(state.isEnglish ? "Auto (prefer default chat)" : "自动（优先默认对话模型）").tag("")
                        ForEach(audioProfiles) { p in
                            Text(p.displayName).tag(p.id)
                        }
                    }
                    .disabled(!audioVideoEnabled)
                    .onChange(of: audioProfileId) { _, _ in
                        Task { await saveMediaProcessingSettings() }
                    }

                    let videoProfiles = profiles.filter { $0.supportsVideo }
                    Picker(state.isEnglish ? "Video" : "视频处理模型", selection: $videoProfileId) {
                        Text(state.isEnglish ? "Auto (prefer default chat)" : "自动（优先默认对话模型）").tag("")
                        ForEach(videoProfiles) { p in
                            Text(p.displayName).tag(p.id)
                        }
                    }
                    .disabled(!audioVideoEnabled)
                    .onChange(of: videoProfileId) { _, _ in
                        Task { await saveMediaProcessingSettings() }
                    }

                    Text(mediaRoutingSummaryText)
                        .font(.caption)
                        .foregroundColor(palette.textSecondary)
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(palette.elevatedBg)
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(palette.subtleBorder, lineWidth: 1))
                .cornerRadius(10)

                // Actions: Save / Test Connection / Clear Key
                HStack(spacing: 10) {
                    Button(state.t("settings.save")) {
                        Task { await saveCurrentProfile() }
                    }
                    .buttonStyle(.borderedProminent)

                    Button(isTesting ? (state.isEnglish ? "Testing..." : "测试中…") : (state.isEnglish ? "Test Connection" : "测试连接")) {
                        Task { await testCurrentProfile() }
                    }
                    .buttonStyle(.bordered)
                    .disabled(isTesting || profileId == nil)

                    Button(state.isEnglish ? "Clear Key" : "清除密钥") {
                        Task { await clearCurrentKey() }
                    }
                    .buttonStyle(.bordered)
                    .disabled(profileId == nil || keychainRef.isEmpty)

                    Spacer()
                }

                if !statusMsg.isEmpty {
                    Text(statusMsg)
                        .font(.caption)
                        .foregroundColor(palette.textSecondary)
                        .textSelection(.enabled)
                }
            }
            .padding(14)
        }
    }

    // MARK: - Tab 4: Semantic Search (Embeddings)

    private var embeddingTab: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(state.isEnglish
                 ? "Semantic search finds relevant passages across different phrasings or languages using a local multilingual embedding model. Nothing is uploaded."
                 : "语义检索让提问和来源语言不同、或措辞不同时也能找到相关内容。它使用本地多语言嵌入模型，不会上传你的内容。")
                .font(.callout)
                .foregroundColor(palette.textSecondary)

            Text(state.embeddingDownloadRunning && !state.embeddingDownloadStatus.isEmpty ? state.embeddingDownloadStatus : embStatusText)
                .font(.callout)
                .foregroundColor(palette.textPrimary)

            if embRunning || state.embeddingDownloadRunning {
                ProgressView(value: state.embeddingDownloadProgress, total: 100)
                    .progressViewStyle(.linear)
            }

            HStack(spacing: 10) {
                Button(embInstalled
                       ? (state.isEnglish ? "Enable Downloaded Model" : "启用已下载的模型")
                       : (state.isEnglish ? "Download & Enable (~\(embSizeMb) MB)" : "下载并启用（约 \(embSizeMb) MB）")) {
                    Task {
                        _ = try? await state.core.call(method: "embedding.download")
                        state.embeddingDownloadRunning = true
                        state.embeddingDownloadProgress = 0
                        await refreshEmbeddingStatus()
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(embRunning || state.embeddingDownloadRunning)

                if embRunning || state.embeddingDownloadRunning {
                    Button(state.isEnglish ? "Cancel" : "取消") {
                        Task {
                            _ = try? await state.core.call(method: "embedding.download_cancel")
                            await refreshEmbeddingStatus()
                        }
                    }
                    .buttonStyle(.bordered)
                }

                Button(state.isEnglish ? "Rebuild Index" : "重建索引") {
                    Task {
                        if let r = try? await state.core.call(method: "embedding.rebuild", params: ["force": true]) {
                            let q = r["sources_queued"] as? Int ?? 0
                            embStatusText = state.isEnglish
                                ? "Queued \(q) sources for rebuild."
                                : "已排队重建 \(q) 个来源。"
                        }
                    }
                }
                .buttonStyle(.bordered)
                .disabled(!embActive)
            }

            Spacer()
        }
        .padding(16)
    }

    // MARK: - AI Models Logic

    private func profileDisplayLabel(_ p: ModelProfileItem) -> String {
        var mods: [String] = []
        if p.supportsVision { mods.append(state.isEnglish ? "Vision" : "图文") }
        if p.supportsAudio { mods.append(state.isEnglish ? "Audio" : "音频") }
        if p.supportsVideo { mods.append(state.isEnglish ? "Video" : "视频") }
        let modTag = mods.isEmpty ? "" : " [\(mods.joined(separator: "/"))]"
        var defTag = p.isActive ? (state.isEnglish ? " (Default)" : "（默认对话）") : ""
        if !p.eligible { defTag += state.isEnglish ? " (unsupported, disabled)" : "（不支持，已停用）" }
        return "\(p.displayName)\(defTag)\(modTag)"
    }

    private var mediaRoutingSummaryText: String {
        let chatModel = profiles.first(where: { $0.isActive })
        func pickMod(explicitId: String, _ check: (ModelProfileItem) -> Bool, empty: String) -> String {
            if !explicitId.isEmpty, let exp = profiles.first(where: { $0.id == explicitId && check($0) }) {
                return exp.displayName
            }
            if let cm = chatModel, check(cm) { return cm.displayName }
            if let m = profiles.first(where: check) { return m.displayName }
            return empty
        }
        let chatName = chatModel?.displayName ?? (state.isEnglish ? "Not configured" : "未配置")
        if state.isEnglish {
            let v = pickMod(explicitId: visionProfileId, { $0.supportsVision }, empty: "None (local OCR fallback)")
            let a = audioVideoEnabled ? pickMod(explicitId: audioProfileId, { $0.supportsAudio }, empty: "Not assigned") : "Disabled"
            let vid = audioVideoEnabled ? pickMod(explicitId: videoProfileId, { $0.supportsVideo }, empty: "Not assigned") : "Disabled"
            return "Current model routing:\n• Chat: \(chatName)\n• Vision: \(v)\n• Audio: \(a)\n• Video: \(vid)"
        } else {
            let v = pickMod(explicitId: visionProfileId, { $0.supportsVision }, empty: "未指定（将尝试本地 OCR）")
            let a = audioVideoEnabled ? pickMod(explicitId: audioProfileId, { $0.supportsAudio }, empty: "未指定") : "已关闭"
            let vid = audioVideoEnabled ? pickMod(explicitId: videoProfileId, { $0.supportsVideo }, empty: "未指定") : "已关闭"
            return "当前多模型分配：\n• 默认对话：\(chatName)\n• 图文处理：\(v)\n• 音频处理：\(a)\n• 视频处理：\(vid)"
        }
    }

    private func saveMediaProcessingSettings() async {
        var val: [String: Any] = ["audio_video_enabled": audioVideoEnabled]
        if !visionProfileId.isEmpty { val["vision_profile_id"] = visionProfileId }
        if !audioProfileId.isEmpty { val["audio_profile_id"] = audioProfileId }
        if !videoProfileId.isEmpty { val["video_profile_id"] = videoProfileId }
        _ = try? await state.core.call(method: "settings.set", params: [
            "key": "media_processing",
            "value": val
        ])
    }

    private func loadModelsData(selectId: String? = nil) async {
        if let listRes = try? await state.core.call(method: "profile.list"),
           let rawProfiles = listRes["profiles"] as? [[String: Any]] {
            profiles = rawProfiles.map(parseProfile)
        }
        if let presetRes = try? await state.core.call(method: "profile.presets"),
           let rawPresets = presetRes["presets"] as? [[String: Any]] {
            presets = rawPresets.map(parsePreset)
        }
        if let mediaRes = try? await state.core.call(method: "settings.get", params: ["key": "media_processing"]),
           let mv = mediaRes["value"] as? [String: Any] {
            if let av = mv["audio_video_enabled"] as? Bool {
                audioVideoEnabled = av
            }
            let vp = mv["vision_profile_id"] as? String ?? ""
            let ap = mv["audio_profile_id"] as? String ?? ""
            let vidp = mv["video_profile_id"] as? String ?? ""
            visionProfileId = profiles.contains(where: { $0.id == vp && $0.supportsVision }) ? vp : ""
            audioProfileId = profiles.contains(where: { $0.id == ap && $0.supportsAudio }) ? ap : ""
            videoProfileId = profiles.contains(where: { $0.id == vidp && $0.supportsVideo }) ? vidp : ""
        } else {
            visionProfileId = ""
            audioProfileId = ""
            videoProfileId = ""
        }
        let target: ModelProfileItem?
        if let sid = selectId, let found = profiles.first(where: { $0.id == sid }) {
            target = found
        } else if let act = profiles.first(where: { $0.isActive }) {
            target = act
        } else {
            target = profiles.first
        }
        selectedProfileId = target?.id ?? ""
        fillProfile(target)
    }

    private func parseProfile(_ d: [String: Any]) -> ModelProfileItem {
        let caps = d["capabilities"] as? [String: Any] ?? [:]
        var hdrs: [String: String] = [:]
        if let rawH = d["extra_headers"] as? [String: Any] {
            for (k, v) in rawH { hdrs[k] = "\(v)" }
        }
        return ModelProfileItem(
            id: d["id"] as? String ?? "",
            displayName: d["display_name"] as? String ?? "",
            protocolName: d["protocol"] as? String ?? "chat_completion",
            baseUrl: d["base_url"] as? String ?? "",
            modelId: d["model_id"] as? String ?? "",
            keychainRef: d["keychain_ref"] as? String ?? "",
            hasSecret: d["has_secret"] as? Bool ?? false,
            isActive: d["is_active"] as? Bool ?? false,
            allowInsecureHttp: d["allow_insecure_http"] as? Bool ?? false,
            showThinking: (d["show_thinking"] as? Bool) ?? true,
            effort: d["effort"] as? String ?? "",
            contextWindow: d["context_window"] as? Int ?? 32000,
            maxOutputTokens: d["max_output_tokens"] as? Int ?? 8000,
            inputPricePerM: d["input_price_per_m"] as? Double,
            outputPricePerM: d["output_price_per_m"] as? Double,
            supportsVision: caps["supports_vision"] as? Bool ?? false,
            supportsAudio: caps["supports_audio"] as? Bool ?? false,
            supportsVideo: caps["supports_video"] as? Bool ?? false,
            extraHeaders: hdrs,
            eligible: d["eligible"] as? Bool ?? true,
            unsupportedReason: d["unsupported_reason"] as? String
        )
    }

    private func parsePreset(_ d: [String: Any]) -> ProviderPresetItem {
        var hdrs: [String: String] = [:]
        if let rawH = d["extra_headers"] as? [String: Any] {
            for (k, v) in rawH { hdrs[k] = "\(v)" }
        }
        return ProviderPresetItem(
            id: d["id"] as? String ?? "",
            name: d["name"] as? String ?? "",
            nameEn: d["name_en"] as? String ?? (d["name"] as? String ?? ""),
            protocolName: d["protocol"] as? String ?? "chat_completion",
            baseUrl: d["base_url"] as? String ?? "",
            defaultModel: d["default_model"] as? String ?? "",
            suggestedModels: d["suggested_models"] as? [String] ?? [],
            contextWindow: d["context_window"] as? Int ?? 32000,
            maxOutputTokens: d["max_output_tokens"] as? Int ?? 8000,
            supportsVision: d["supports_vision"] as? Bool ?? false,
            supportsAudio: d["supports_audio"] as? Bool ?? false,
            supportsVideo: d["supports_video"] as? Bool ?? false,
            extraHeaders: hdrs
        )
    }

    private func fillProfile(_ p: ModelProfileItem?) {
        guard let p else {
            profileId = nil
            keychainRef = ""
            hasSecret = false
            proto = "chat_completion"
            baseUrl = ""
            modelId = ""
            modelCandidates = []
            apiKey = ""
            allowInsecureHttp = false
            activateChat = !profiles.contains(where: { $0.isActive })
            supportsVision = false
            supportsAudio = false
            supportsVideo = false
            showThinking = true
            effort = ""
            contextWindowText = "32000"
            maxOutputTokensText = "8000"
            priceInText = ""
            priceOutText = ""
            extraHeaders = []
            statusMsg = state.isEnglish ? "Editing new profile. Choose a preset or fill in details and save." : "正在编辑新配置，可选择预设或手动填写后保存。"
            return
        }
        profileId = p.id
        keychainRef = p.keychainRef
        hasSecret = p.hasSecret
        proto = p.protocolName
        baseUrl = p.baseUrl
        modelId = p.modelId
        modelCandidates = p.modelId.isEmpty ? [] : [p.modelId]
        apiKey = ""
        allowInsecureHttp = p.allowInsecureHttp
        activateChat = p.isActive
        supportsVision = p.supportsVision
        supportsAudio = p.supportsAudio
        supportsVideo = p.supportsVideo
        showThinking = p.showThinking
        effort = p.effort
        contextWindowText = String(p.contextWindow)
        maxOutputTokensText = String(p.maxOutputTokens)
        priceInText = p.inputPricePerM.map { String($0) } ?? ""
        priceOutText = p.outputPricePerM.map { String($0) } ?? ""
        extraHeaders = p.extraHeaders.sorted(by: { $0.key < $1.key }).map { (UUID(), $0.key, $0.value) }
        statusMsg = p.hasSecret
            ? (state.isEnglish ? "Key status: saved in macOS Keychain." : "密钥状态：已保存在 macOS Keychain。")
            : (state.isEnglish ? "Key status: not saved." : "密钥状态：未保存。")
    }

    private func applyPreset(_ pr: ProviderPresetItem) {
        proto = pr.protocolName
        baseUrl = pr.baseUrl
        modelCandidates = pr.suggestedModels
        if !pr.defaultModel.isEmpty {
            modelId = pr.defaultModel
        } else if let first = pr.suggestedModels.first {
            modelId = first
        }
        contextWindowText = String(pr.contextWindow)
        maxOutputTokensText = String(pr.maxOutputTokens)
        supportsVision = pr.supportsVision
        supportsAudio = pr.supportsAudio
        supportsVideo = pr.supportsVideo
        extraHeaders = pr.extraHeaders.sorted(by: { $0.key < $1.key }).map { (UUID(), $0.key, $0.value) }
        let pname = state.isEnglish ? pr.nameEn : pr.name
        statusMsg = state.isEnglish
            ? "Applied preset: \(pname). Enter API Key and save."
            : "已应用预设：\(pname)，请填写 API Key 后保存。"
    }

    private func discoverModels() async {
        let url = baseUrl.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !url.isEmpty else {
            statusMsg = state.isEnglish ? "Please enter Base URL first." : "请先填写接口地址 (Base URL)。"
            return
        }
        isDiscovering = true
        defer { isDiscovering = false }
        do {
            let res = try await state.core.call(method: "profile.discover_models", params: [
                "profile_id": profileId ?? "",
                "base_url": url,
                "protocol": proto,
                "api_key": apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
            ])
            let rawModels = res["models"] as? [[String: Any]] ?? []
            modelCandidates = rawModels.compactMap { $0["id"] as? String }
            statusMsg = state.isEnglish
                ? "Found \(modelCandidates.count) models. Click the dropdown next to Model ID to choose."
                : "找到 \(modelCandidates.count) 个模型，可点击模型 ID 右侧下拉选择。"
        } catch {
            statusMsg = error.localizedDescription
        }
    }

    private func saveCurrentProfile() async {
        let url = baseUrl.trimmingCharacters(in: .whitespacesAndNewlines)
        let mid = modelId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !url.isEmpty, !mid.isEmpty else {
            statusMsg = state.isEnglish ? "Please fill in Base URL and Model ID." : "请填写接口地址和模型 ID。"
            return
        }
        var hdrDict: [String: String] = [:]
        for h in extraHeaders {
            let k = h.key.trimmingCharacters(in: .whitespacesAndNewlines)
            if !k.isEmpty { hdrDict[k] = h.value.trimmingCharacters(in: .whitespacesAndNewlines) }
        }
        var params: [String: Any] = [
            "display_name": "\(proto) / \(mid)",
            "protocol": proto,
            "base_url": url,
            "model_id": mid,
            "activate": activateChat,
            "allow_insecure_http": allowInsecureHttp,
            "show_thinking": showThinking,
            "context_window": Int(contextWindowText) ?? 32000,
            "max_output_tokens": Int(maxOutputTokensText) ?? 8000,
            "extra_headers": hdrDict,
            "supports_vision": supportsVision,
            "supports_audio": supportsAudio,
            "supports_video": supportsVideo,
        ]
        if !effort.isEmpty { params["effort"] = effort }
        if let pi = Double(priceInText) { params["input_price_per_m"] = pi }
        if let po = Double(priceOutText) { params["output_price_per_m"] = po }
        if let pid = profileId { params["id"] = pid }
        // the core probes tool calling and structured output with this key before saving
        if !apiKey.isEmpty { params["api_key"] = apiKey }
        statusMsg = state.isEnglish
            ? "Checking that the model supports tool calling and structured output…"
            : "正在检测模型是否支持工具调用和结构化输出…"

        do {
            let sv = try await state.core.call(method: "profile.upsert", params: params)
            let newId = sv["id"] as? String ?? profileId
            let newKref = sv["keychain_ref"] as? String ?? keychainRef
            profileId = newId
            keychainRef = newKref
            apiKey = ""
            if activateChat, let nid = newId {
                _ = try? await state.core.call(method: "profile.activate", params: ["profile_id": nid])
            }
            await saveMediaProcessingSettings()
            await loadModelsData(selectId: newId)
            statusMsg = state.isEnglish ? "Capabilities verified; profile saved." : "已通过能力检测并保存。"
        } catch {
            statusMsg = state.isEnglish
                ? "Not saved: \(error.localizedDescription) This app only works with models that support tool calling and structured output."
                : "未保存：\(error.localizedDescription) 本应用只支持具备工具调用和结构化输出的模型。"
        }
    }

    private func testCurrentProfile() async {
        guard let pid = profileId else { return }
        isTesting = true
        statusMsg = state.isEnglish ? "Testing connection..." : "正在测试连接…"
        defer { isTesting = false }
        do {
            let res = try await state.core.call(method: "profile.test", params: ["profile_id": pid])
            let reply = res["reply"] as? String ?? "OK"
            statusMsg = state.isEnglish ? "Connected: \(reply)" : "连接成功：\(reply)"
        } catch {
            statusMsg = state.isEnglish ? "Connection test failed: \(error.localizedDescription)" : "连接测试失败：\(error.localizedDescription)"
        }
    }

    private func clearCurrentKey() async {
        guard !keychainRef.isEmpty else { return }
        do {
            _ = try await state.core.call(method: "secret.clear", params: ["keychain_ref": keychainRef])
            hasSecret = false
            statusMsg = state.isEnglish ? "API key cleared." : "密钥已清除。"
        } catch {
            statusMsg = error.localizedDescription
        }
    }

    private func deleteCurrentProfile() async {
        guard let pid = profileId else { return }
        do {
            _ = try await state.core.call(method: "profile.delete", params: ["profile_id": pid])
            await loadModelsData()
            statusMsg = state.isEnglish ? "Profile deleted." : "配置已删除。"
        } catch {
            statusMsg = error.localizedDescription
        }
    }

    private func refreshEmbeddingStatus() async {
        guard let es = try? await state.core.call(method: "embedding.status"),
              let dm = try? await state.core.call(method: "embedding.default_model") else { return }
        let dl = try? await state.core.call(method: "embedding.download_status")
        let running = (dl?["running"] as? Bool) ?? false
        let installed = (dm["installed"] as? Bool) ?? false
        let totalBytes = (dm["total_bytes"] as? Int) ?? 0
        let mb = totalBytes / 1_000_000
        let totalChunks = (es["total_chunks"] as? Int) ?? 0
        let doneChunks = (es["embedded_chunks"] as? Int) ?? 0
        let activeModel = es["active_model"] as? [String: Any]

        embRunning = running
        embInstalled = installed
        embSizeMb = mb
        embActive = activeModel != nil

        if let am = activeModel, let name = am["model_name"] as? String {
            embStatusText = state.isEnglish
                ? "Active model: \(name). Indexed chunks: \(doneChunks)/\(totalChunks)."
                : "当前模型：\(name)。已建立索引的片段：\(doneChunks)/\(totalChunks)。"
        } else if installed {
            embStatusText = state.isEnglish ? "Model downloaded but not enabled." : "模型文件已下载，尚未启用。"
        } else {
            embStatusText = state.isEnglish
                ? "Semantic search not enabled. Default model is ~\(mb) MB."
                : "尚未启用语义检索。默认模型约 \(mb) MB。"
        }
    }
}

// MARK: - Privacy & Data Sheet (synced with Windows ShowPrivacyAsync)

struct PrivacyInfoSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.dismiss) var dismiss
    @Environment(\.colorScheme) private var systemColorScheme

    private var themeId: AppThemeId { AppThemeId.resolve(state.prefs.theme) }
    private var palette: AppThemePalette { themeId.palette(systemScheme: systemColorScheme) }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                AppLogoView(size: 24, accent: palette.accent)
                Text(state.t("nb.privacy"))
                    .font(.headline)
                    .foregroundColor(palette.textPrimary)
                Spacer()
            }

            VStack(alignment: .leading, spacing: 8) {
                if state.isEnglish {
                    Text("• Your sources, notes, and conversations are stored locally on this Mac.")
                    Text("• When asking questions or generating Studio artifacts, selected excerpts and prompts are sent to your configured model provider.")
                    Text("• API keys are stored securely in macOS Keychain and never written to plain files.")
                    Text("• Diagnostic exports contain only counts, statuses, and error codes—never document contents or keys.")
                } else {
                    Text("• 你的来源、笔记和对话均保存在这台 Mac 本地。")
                    Text("• 提问或生成产物时，被选中的来源片段、问题和相关对话会发送给你配置的模型服务商（接口地址不在本机时）。")
                    Text("• API Key 仅保存在 macOS Keychain 中，不会写入明文文件或日志。")
                    Text("• 诊断包只含数量、状态和错误码，不含文档内容、提示词、回答或密钥。")
                }
            }
            .font(.callout)
            .foregroundColor(palette.textSecondary)

            HStack {
                Spacer()
                Button(state.t("settings.close")) { dismiss() }
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .frame(width: 480)
        .background(palette.panelBg)
        .preferredColorScheme(themeId.preferredColorScheme)
    }
}

// MARK: - Theme Swatch Card

private struct ThemeSwatchCard: View {
    let option: AppThemeId
    let isSelected: Bool
    let en: Bool
    let systemScheme: ColorScheme
    let activePalette: AppThemePalette
    let onSelect: () -> Void

    @State private var isHovered = false

    private var preview: AppThemePalette { option.palette(systemScheme: systemScheme) }

    var body: some View {
        Button(action: onSelect) {
            HStack(spacing: 10) {
                ZStack {
                    RoundedRectangle(cornerRadius: 8)
                        .fill(preview.canvasBg)
                        .overlay(
                            RoundedRectangle(cornerRadius: 8)
                                .stroke(preview.strongBorder.opacity(0.7), lineWidth: 1)
                        )
                    HStack(spacing: 2.5) {
                        RoundedRectangle(cornerRadius: 3)
                            .fill(preview.panelBg)
                            .frame(width: 10)
                        VStack(spacing: 2.5) {
                            RoundedRectangle(cornerRadius: 2.5)
                                .fill(preview.elevatedBg)
                                .frame(height: 8)
                            Spacer(minLength: 0)
                            HStack(spacing: 2) {
                                RoundedRectangle(cornerRadius: 2)
                                    .fill(preview.elevatedBg)
                                    .frame(height: 5)
                                Circle()
                                    .fill(preview.accent)
                                    .frame(width: 6, height: 6)
                            }
                        }
                        .padding(2.5)
                        .background(RoundedRectangle(cornerRadius: 3).fill(preview.panelBg))
                    }
                    .padding(3.5)
                }
                .frame(width: 46, height: 34)

                VStack(alignment: .leading, spacing: 2) {
                    Text(option.displayName(en: en))
                        .font(.callout.weight(.medium))
                        .foregroundColor(activePalette.textPrimary)
                        .lineLimit(1)
                    Text(option.subtitle(en: en))
                        .font(.caption2)
                        .foregroundColor(activePalette.textMuted)
                        .lineLimit(1)
                }

                Spacer(minLength: 4)

                if isSelected {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 15))
                        .foregroundColor(activePalette.accent)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(
                RoundedRectangle(cornerRadius: 12)
                    .fill(isSelected ? activePalette.hoverBg : (isHovered ? activePalette.hoverBg.opacity(0.65) : activePalette.elevatedBg))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .stroke(isSelected ? activePalette.accent : activePalette.subtleBorder, lineWidth: isSelected ? 1.5 : 1)
            )
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .accessibilityLabel(option.displayName(en: en))
    }
}
