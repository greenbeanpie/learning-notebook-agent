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
    /// Most model requests of this profile in flight at once (parallel generation).
    var maxParallel: Int = 4
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
    @State private var maxParallelText = "4"
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
                        Text(L10n.t("settings.lang_auto", en: state.isEnglish)).tag("auto")
                        Text("简体中文").tag("zh-CN")
                        Text("English").tag("en-US")
                    }
                    .onChange(of: state.prefs.language) { _, _ in
                        Task { await state.savePreferences() }
                    }

                    Picker(state.t("settings.font_size"), selection: $state.prefs.fontSize) {
                        Text(L10n.t("appearance.compact_14px", en: state.isEnglish)).tag(14.0)
                        Text(L10n.t("appearance.normal_15px", en: state.isEnglish)).tag(15.0)
                        Text(L10n.t("appearance.large_17px", en: state.isEnglish)).tag(17.0)
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
                        Text(L10n.t("settings.studio_lang_source", en: state.isEnglish)).tag("")
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
                    Picker(L10n.t("model.saved_profiles", en: state.isEnglish), selection: $selectedProfileId) {
                        Text(L10n.t("model.new_profile", en: state.isEnglish)).tag("")
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

                    Button(L10n.t("model.new", en: state.isEnglish)) {
                        selectedProfileId = ""
                        selectedPresetId = ""
                        fillProfile(nil)
                    }
                    .buttonStyle(.bordered)

                    Button(L10n.t("common.delete", en: state.isEnglish), role: .destructive) {
                        Task { await deleteCurrentProfile() }
                    }
                    .buttonStyle(.bordered)
                    .disabled(profileId == nil)
                }

                // Provider Preset (Quick Fill)
                if !presets.isEmpty {
                    Picker(L10n.t("model.provider_preset", en: state.isEnglish), selection: $selectedPresetId) {
                        Text(L10n.t("model.choose_a_preset_to_auto", en: state.isEnglish)).tag("")
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
                Picker(L10n.t("model.protocol", en: state.isEnglish), selection: $proto) {
                    Text("chat_completion (OpenAI Compatible)").tag("chat_completion")
                    Text("responses (OpenAI Responses)").tag("responses")
                    Text("anthropic (Anthropic Messages)").tag("anthropic")
                    Text("gemini (Gemini GenerateContent)").tag("gemini")
                }

                VStack(alignment: .leading, spacing: 4) {
                    Text(L10n.t("model.base_url", en: state.isEnglish))
                        .font(.caption).foregroundColor(palette.textSecondary)
                    TextField("https://api.example.com/v1", text: $baseUrl)
                        .textFieldStyle(.roundedBorder)
                }

                // Model ID + Candidates menu + Fetch Models
                VStack(alignment: .leading, spacing: 4) {
                    Text(L10n.t("model.model_id", en: state.isEnglish))
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
                            .help(L10n.t("model.choose_model", en: state.isEnglish))
                        }

                        Button(isDiscovering ? (L10n.t("model.fetching", en: state.isEnglish)) : (L10n.t("model.fetch_models", en: state.isEnglish))) {
                            Task { await discoverModels() }
                        }
                        .buttonStyle(.bordered)
                        .disabled(isDiscovering)
                    }
                }

                // API Key
                VStack(alignment: .leading, spacing: 4) {
                    Text(L10n.t("model.api_key", en: state.isEnglish))
                        .font(.caption).foregroundColor(palette.textSecondary)
                    SecureField(
                        hasSecret
                            ? (L10n.t("model.saved_in_keychain_leave_blank", en: state.isEnglish))
                            : (L10n.t("model.enter_api_key_optional_for", en: state.isEnglish)),
                        text: $apiKey
                    )
                    .textFieldStyle(.roundedBorder)
                }

                // Toggles
                VStack(alignment: .leading, spacing: 6) {
                    Toggle(L10n.t("model.allow_non_loopback_plaintext_http", en: state.isEnglish), isOn: $allowInsecureHttp)
                    Toggle(L10n.t("model.set_as_default_chat_model", en: state.isEnglish), isOn: $activateChat)
                    HStack(spacing: 16) {
                        Toggle(L10n.t("model.vision_images_pdf", en: state.isEnglish), isOn: $supportsVision)
                        Toggle(L10n.t("model.audio", en: state.isEnglish), isOn: $supportsAudio)
                        Toggle(L10n.t("model.video", en: state.isEnglish), isOn: $supportsVideo)
                    }
                }
                .font(.callout)

                // Advanced Options
                DisclosureGroup(L10n.t("model.advanced_options", en: state.isEnglish), isExpanded: $showAdvanced) {
                    VStack(alignment: .leading, spacing: 10) {
                        Toggle(L10n.t("model.show_model_thinking_process", en: state.isEnglish), isOn: $showThinking)

                        Picker(L10n.t("model.reasoning_effort", en: state.isEnglish), selection: $effort) {
                            Text(L10n.t("model.effort_default", en: state.isEnglish)).tag("")
                            Text(L10n.t("model.effort_low", en: state.isEnglish)).tag("low")
                            Text(L10n.t("model.effort_medium", en: state.isEnglish)).tag("medium")
                            Text(L10n.t("model.effort_high", en: state.isEnglish)).tag("high")
                        }

                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(L10n.t("model.context_window", en: state.isEnglish))
                                    .font(.caption).foregroundColor(palette.textSecondary)
                                TextField("32000", text: $contextWindowText)
                                    .textFieldStyle(.roundedBorder)
                            }
                            VStack(alignment: .leading, spacing: 4) {
                                Text(L10n.t("model.max_output_tokens", en: state.isEnglish))
                                    .font(.caption).foregroundColor(palette.textSecondary)
                                TextField("8000", text: $maxOutputTokensText)
                                    .textFieldStyle(.roundedBorder)
                            }
                        }

                        VStack(alignment: .leading, spacing: 4) {
                            Text(L10n.t("model.max_parallel", en: state.isEnglish))
                                .font(.caption).foregroundColor(palette.textSecondary)
                            TextField("4", text: $maxParallelText)
                                .textFieldStyle(.roundedBorder)
                                .frame(maxWidth: 120, alignment: .leading)
                            Text(L10n.t("model.max_parallel_tip", en: state.isEnglish))
                                .font(.caption2).foregroundColor(palette.textMuted)
                        }

                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(L10n.t("model.input_price_1m", en: state.isEnglish))
                                    .font(.caption).foregroundColor(palette.textSecondary)
                                TextField(L10n.t("model.optional", en: state.isEnglish), text: $priceInText)
                                    .textFieldStyle(.roundedBorder)
                            }
                            VStack(alignment: .leading, spacing: 4) {
                                Text(L10n.t("model.output_price_1m", en: state.isEnglish))
                                    .font(.caption).foregroundColor(palette.textSecondary)
                                TextField(L10n.t("model.optional", en: state.isEnglish), text: $priceOutText)
                                    .textFieldStyle(.roundedBorder)
                            }
                        }

                        // Extra headers
                        HStack {
                            Text(L10n.t("model.extra_headers", en: state.isEnglish))
                                .font(.caption).foregroundColor(palette.textSecondary)
                            Spacer()
                            Button(L10n.t("model.add_header", en: state.isEnglish)) {
                                extraHeaders.append((id: UUID(), key: "", value: ""))
                            }
                            .font(.caption)
                            .buttonStyle(.bordered)
                        }
                        ForEach($extraHeaders, id: \.id) { $hdr in
                            HStack(spacing: 6) {
                                TextField(L10n.t("model.header_name", en: state.isEnglish), text: $hdr.key)
                                    .textFieldStyle(.roundedBorder)
                                TextField(L10n.t("model.value", en: state.isEnglish), text: $hdr.value)
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
                        L10n.t("route.enable_native_audio_video_processing", en: state.isEnglish),
                        isOn: $audioVideoEnabled
                    )
                    .font(.callout)
                    .onChange(of: audioVideoEnabled) { _, _ in
                        Task { await saveMediaProcessingSettings() }
                    }

                    Divider().opacity(0.35)

                    Text(L10n.t("route.per_modality_model_assignment", en: state.isEnglish))
                        .font(.caption.weight(.semibold))
                        .foregroundColor(palette.textPrimary)

                    let visionProfiles = profiles.filter { $0.supportsVision }
                    Picker(L10n.t("route.vision_image_pdf", en: state.isEnglish), selection: $visionProfileId) {
                        Text(L10n.t("route.auto_prefer_default_chat", en: state.isEnglish)).tag("")
                        ForEach(visionProfiles) { p in
                            Text(p.displayName).tag(p.id)
                        }
                    }
                    .onChange(of: visionProfileId) { _, _ in
                        Task { await saveMediaProcessingSettings() }
                    }

                    let audioProfiles = profiles.filter { $0.supportsAudio }
                    Picker(L10n.t("route.audio_2", en: state.isEnglish), selection: $audioProfileId) {
                        Text(L10n.t("route.auto_prefer_default_chat", en: state.isEnglish)).tag("")
                        ForEach(audioProfiles) { p in
                            Text(p.displayName).tag(p.id)
                        }
                    }
                    .disabled(!audioVideoEnabled)
                    .onChange(of: audioProfileId) { _, _ in
                        Task { await saveMediaProcessingSettings() }
                    }

                    let videoProfiles = profiles.filter { $0.supportsVideo }
                    Picker(L10n.t("route.video_2", en: state.isEnglish), selection: $videoProfileId) {
                        Text(L10n.t("route.auto_prefer_default_chat", en: state.isEnglish)).tag("")
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

                    Button(isTesting ? (L10n.t("model.testing", en: state.isEnglish)) : (L10n.t("model.test_connection", en: state.isEnglish))) {
                        Task { await testCurrentProfile() }
                    }
                    .buttonStyle(.bordered)
                    .disabled(isTesting || profileId == nil)

                    Button(L10n.t("model.clear_key", en: state.isEnglish)) {
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
            Text(L10n.t("emb.semantic_search_finds_relevant_passages", en: state.isEnglish))
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
                       ? (L10n.t("emb.enable_downloaded_model", en: state.isEnglish))
                       : (L10n.fmt("emb.download_size", en: state.isEnglish, embSizeMb))) {
                    Task {
                        _ = await state.run("embedding.download")
                        state.embeddingDownloadRunning = true
                        state.embeddingDownloadProgress = 0
                        await refreshEmbeddingStatus()
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(embRunning || state.embeddingDownloadRunning)

                if embRunning || state.embeddingDownloadRunning {
                    Button(L10n.t("common.cancel", en: state.isEnglish)) {
                        Task {
                            _ = await state.run("embedding.download_cancel")
                            await refreshEmbeddingStatus()
                        }
                    }
                    .buttonStyle(.bordered)
                }

                Button(L10n.t("emb.rebuild_index", en: state.isEnglish)) {
                    Task {
                        if let r = await state.run("embedding.rebuild", ["force": true]) {
                            let q = r["sources_queued"] as? Int ?? 0
                            embStatusText = L10n.fmt("emb.queued_rebuild", en: state.isEnglish, q)
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
        if p.supportsVision { mods.append(L10n.t("model.vision", en: state.isEnglish)) }
        if p.supportsAudio { mods.append(L10n.t("model.audio", en: state.isEnglish)) }
        if p.supportsVideo { mods.append(L10n.t("model.video", en: state.isEnglish)) }
        let modTag = mods.isEmpty ? "" : " [\(mods.joined(separator: "/"))]"
        var defTag = p.isActive ? (L10n.t("model.default", en: state.isEnglish)) : ""
        if !p.eligible { defTag += L10n.t("model.disabled_tag", en: state.isEnglish) }
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
        let chatName = chatModel?.displayName ?? (L10n.t("route.not_configured", en: state.isEnglish))
        let en = state.isEnglish
        let v = pickMod(explicitId: visionProfileId, { $0.supportsVision }, empty: L10n.t("route.vision_none", en: en))
        let a = audioVideoEnabled ? pickMod(explicitId: audioProfileId, { $0.supportsAudio }, empty: L10n.t("route.unassigned", en: en)) : L10n.t("route.disabled", en: en)
        let vid = audioVideoEnabled ? pickMod(explicitId: videoProfileId, { $0.supportsVideo }, empty: L10n.t("route.unassigned", en: en)) : L10n.t("route.disabled", en: en)
        return L10n.fmt("route.summary", en: en, chatName, v, a, vid)
    }

    private func saveMediaProcessingSettings() async {
        var val: [String: Any] = ["audio_video_enabled": audioVideoEnabled]
        if !visionProfileId.isEmpty { val["vision_profile_id"] = visionProfileId }
        if !audioProfileId.isEmpty { val["audio_profile_id"] = audioProfileId }
        if !videoProfileId.isEmpty { val["video_profile_id"] = videoProfileId }
        _ = await state.run("settings.set", [
            "key": "media_processing",
            "value": val
        ])
    }

    private func loadModelsData(selectId: String? = nil) async {
        if let listRes = await state.run("profile.list"),
           let rawProfiles = listRes["profiles"] as? [[String: Any]] {
            profiles = rawProfiles.map(parseProfile)
        }
        if let presetRes = await state.run("profile.presets"),
           let rawPresets = presetRes["presets"] as? [[String: Any]] {
            presets = rawPresets.map(parsePreset)
        }
        if let mediaRes = await state.run("settings.get", ["key": "media_processing"]),
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
            unsupportedReason: d["unsupported_reason"] as? String,
            maxParallel: d["max_parallel"] as? Int ?? 4
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
            maxParallelText = "4"
            priceInText = ""
            priceOutText = ""
            extraHeaders = []
            statusMsg = L10n.t("model.editing_new_profile_choose_a", en: state.isEnglish)
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
        maxParallelText = String(p.maxParallel)
        priceInText = p.inputPricePerM.map { String($0) } ?? ""
        priceOutText = p.outputPricePerM.map { String($0) } ?? ""
        extraHeaders = p.extraHeaders.sorted(by: { $0.key < $1.key }).map { (UUID(), $0.key, $0.value) }
        statusMsg = p.hasSecret
            ? (L10n.t("model.key_status_saved_in_macos", en: state.isEnglish))
            : (L10n.t("model.key_status_not_saved", en: state.isEnglish))
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
        statusMsg = L10n.fmt("model.applied_preset_enter_api_key", en: state.isEnglish, pname)
    }

    private func discoverModels() async {
        let url = baseUrl.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !url.isEmpty else {
            statusMsg = L10n.t("model.please_enter_base_url_first", en: state.isEnglish)
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
            statusMsg = L10n.fmt("model.found_models_click_the_dropdown", en: state.isEnglish, modelCandidates.count)
        } catch {
            statusMsg = error.localizedDescription
        }
    }

    private func saveCurrentProfile() async {
        let url = baseUrl.trimmingCharacters(in: .whitespacesAndNewlines)
        let mid = modelId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !url.isEmpty, !mid.isEmpty else {
            statusMsg = L10n.t("model.please_fill_in_base_url", en: state.isEnglish)
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
            "max_parallel": min(64, max(1, Int(maxParallelText) ?? 4)),
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
        statusMsg = L10n.t("model.probing", en: state.isEnglish)

        do {
            let sv = try await state.core.call(method: "profile.upsert", params: params)
            let newId = sv["id"] as? String ?? profileId
            let newKref = sv["keychain_ref"] as? String ?? keychainRef
            profileId = newId
            keychainRef = newKref
            apiKey = ""
            if activateChat, let nid = newId {
                _ = await state.run("profile.activate", ["profile_id": nid])
            }
            await saveMediaProcessingSettings()
            await loadModelsData(selectId: newId)
            statusMsg = L10n.t("model.saved_checked", en: state.isEnglish)
        } catch {
            if let ce = error as? CoreError, ce.code == "CAPABILITY_MISSING" {
                statusMsg = L10n.fmt("model.refused", en: state.isEnglish, ce.message)
            } else {
                statusMsg = L10n.fmt("model.save_failed", en: state.isEnglish, state.describe(error))
            }
        }
    }

    private func testCurrentProfile() async {
        guard let pid = profileId else { return }
        isTesting = true
        statusMsg = L10n.t("model.testing_connection", en: state.isEnglish)
        defer { isTesting = false }
        do {
            let res = try await state.core.call(method: "profile.test", params: ["profile_id": pid])
            let reply = res["reply"] as? String ?? L10n.t("model.reply_ok", en: state.isEnglish)
            statusMsg = L10n.fmt("model.connected", en: state.isEnglish, reply)
        } catch {
            statusMsg = L10n.fmt("model.connection_test_failed", en: state.isEnglish, error.localizedDescription)
        }
    }

    private func clearCurrentKey() async {
        guard !keychainRef.isEmpty else { return }
        do {
            _ = try await state.core.call(method: "secret.clear", params: ["keychain_ref": keychainRef])
            hasSecret = false
            statusMsg = L10n.t("model.api_key_cleared", en: state.isEnglish)
        } catch {
            statusMsg = error.localizedDescription
        }
    }

    private func deleteCurrentProfile() async {
        guard let pid = profileId else { return }
        do {
            _ = try await state.core.call(method: "profile.delete", params: ["profile_id": pid])
            await loadModelsData()
            statusMsg = L10n.t("model.profile_deleted", en: state.isEnglish)
        } catch {
            statusMsg = error.localizedDescription
        }
    }

    private func refreshEmbeddingStatus() async {
        guard let es = await state.run("embedding.status"),
              let dm = await state.run("embedding.default_model") else { return }
        let dl = await state.run("embedding.download_status")
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
            embStatusText = L10n.fmt("emb.active_model_indexed_chunks", en: state.isEnglish, name, doneChunks, totalChunks)
        } else if installed {
            embStatusText = L10n.t("emb.model_downloaded_but_not_enabled", en: state.isEnglish)
        } else {
            embStatusText = L10n.fmt("emb.status_not_enabled", en: state.isEnglish, mb)
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
                ForEach(["privacy.line_local", "privacy.line_send", "privacy.line_keys", "privacy.line_diag"], id: \.self) { k in
                    Text(L10n.t(k, en: state.isEnglish))
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
