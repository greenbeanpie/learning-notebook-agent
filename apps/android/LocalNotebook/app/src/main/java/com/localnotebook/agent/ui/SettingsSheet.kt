package com.localnotebook.agent.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.localnotebook.agent.core.L10n
import com.localnotebook.agent.core.MaxTokenLimitsConfig
import com.localnotebook.agent.core.MediaRoutingConfig
import com.localnotebook.agent.core.ModelProfileItem
import com.localnotebook.agent.core.ProviderPresetItem
import com.localnotebook.agent.core.UiPreferences

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun UnifiedSettingsSheet(
    prefs: UiPreferences,
    presets: List<ProviderPresetItem> = emptyList(),
    profiles: List<ModelProfileItem> = emptyList(),
    mediaRouting: MediaRoutingConfig = MediaRoutingConfig(),
    lectureRepairRounds: Int = 2,
    onSaveLectureRepairRounds: (Int) -> Unit = {},
    tokenLimits: MaxTokenLimitsConfig = MaxTokenLimitsConfig(),
    onSaveTokenLimits: (MaxTokenLimitsConfig) -> Unit = {},
    onResetTokenLimits: () -> Unit = {},
    debugMode: Boolean = false,
    onToggleDebugMode: (Boolean) -> Unit = {},
    onExportLogs: () -> Unit = {},
    onSavePrefs: (UiPreferences) -> Unit,
    onSaveModel: (
        profileId: String?,
        protocol: String,
        baseUrl: String,
        modelId: String,
        apiKey: String,
        extraHeaders: Map<String, String>,
        contextWindow: Int,
        maxOutputTokens: Int,
        activateChat: Boolean,
        allowInsecureHttp: Boolean,
        maxParallel: Int,
        preset: String,
        shrinkMedia: Boolean,
        onFailed: () -> Unit,
    ) -> Unit,
    onDeleteProfile: (String) -> Unit = {},
    onTestProfile: (String) -> Unit = {},
    onClearKey: (String) -> Unit = {},
    onSaveMediaRouting: (MediaRoutingConfig) -> Unit = {},
    onDownloadEmbedding: () -> Unit,
    onDismiss: () -> Unit,
) {
    val en = prefs.isEnglish
    var tabIndex by remember { mutableIntStateOf(0) }
    var draftPrefs by remember(prefs) { mutableStateOf(prefs) }

    val activeOrFirst = profiles.firstOrNull { it.isActive } ?: profiles.firstOrNull()
    // The form is filled by selectProfile() only; it is not keyed on `profiles`, so a reload
    // after Apply / Test does not throw the form back to the default model.
    var selectedProfileId by remember { mutableStateOf("") }
    var selectedPresetId by remember { mutableStateOf("") }
    var protocol by remember { mutableStateOf("chat_completion") }
    var baseUrl by remember { mutableStateOf("") }
    var modelId by remember { mutableStateOf("") }
    var apiKey by remember { mutableStateOf("") }
    var contextWindow by remember { mutableIntStateOf(32000) }
    // text, so that empty can mean "unlimited"; empty or 0 is sent as 0
    var maxOutputText by remember { mutableStateOf("") }
    var activateChat by remember { mutableStateOf(true) }
    var allowInsecureHttp by remember { mutableStateOf(false) }
    var shrinkMedia by remember { mutableStateOf(false) }
    var maxParallelText by remember { mutableStateOf("") }
    var extraHeaders by remember { mutableStateOf<List<Pair<String, String>>>(emptyList()) }
    // what was last loaded or applied; Apply is enabled only when the form differs from it
    var savedForm by remember { mutableStateOf("") }
    var formLoaded by remember { mutableStateOf(false) }
    var reloadAfterApply by remember { mutableStateOf(false) }
    var knownIds by remember { mutableStateOf(emptySet<String>()) }
    var confirmDelete by remember { mutableStateOf(false) }

    fun formSnapshot(): String = listOf(
        protocol, baseUrl.trim(), modelId.trim(), if (apiKey.isEmpty()) "" else "key",
        contextWindow.toString(), maxOutputText.trim(), activateChat.toString(),
        allowInsecureHttp.toString(), maxParallelText.trim(),
        selectedPresetId, (selectedPresetId == "mimo" && shrinkMedia).toString(),
        extraHeaders.mapNotNull { (k, v) -> k.trim().takeIf { it.isNotEmpty() }?.let { "$it=${v.trim()}" } }
            .sorted().joinToString("\n"),
    ).joinToString("\u001f")

    fun isPlaintextRemote(url: String): Boolean {
        val u = runCatching { java.net.URI(url.trim()) }.getOrNull() ?: return false
        val host = u.host?.lowercase() ?: return false
        return u.scheme.equals("http", ignoreCase = true) &&
            !(host == "localhost" || host == "::1" || host == "[::1]" || host.startsWith("127."))
    }

    fun selectProfile(p: ModelProfileItem?) {
        selectedProfileId = p?.id ?: ""
        protocol = p?.protocol ?: "chat_completion"
        baseUrl = p?.baseUrl ?: ""
        modelId = p?.modelId ?: ""
        apiKey = ""
        contextWindow = p?.contextWindow ?: 32000
        // 0 (unlimited) is shown as an empty field
        maxOutputText = p?.maxOutputTokens?.takeIf { it > 0 }?.toString().orEmpty()
        activateChat = p?.isActive ?: !profiles.any { it.isActive }
        // the core does not store this flag; a saved plaintext remote URL was confirmed back then
        allowInsecureHttp = p != null && (p.allowInsecureHttp || isPlaintextRemote(p.baseUrl))
        // the preset is stored with the profile; a new profile (p == null) has none
        selectedPresetId = p?.preset.orEmpty()
        shrinkMedia = p?.shrinkMedia ?: false
        maxParallelText = p?.maxParallel?.takeIf { it > 0 }?.toString().orEmpty()
        extraHeaders = p?.extraHeaders?.entries?.map { it.key to it.value } ?: emptyList<Pair<String, String>>()
        savedForm = formSnapshot()
    }

    val formDirty = selectedProfileId.isEmpty() || formSnapshot() != savedForm

    // A reload (after Apply, Test, Clear Key or Delete) shows the stored state of the profile
    // being edited: the newly created one after applying a new profile, the default one when
    // the edited profile was deleted. Unapplied edits are kept on any other reload.
    LaunchedEffect(profiles) {
        val ids = profiles.map { it.id }.toSet()
        val added = ids - knownIds
        knownIds = ids
        val current = profiles.firstOrNull { it.id == selectedProfileId }
        when {
            !formLoaded -> { selectProfile(activeOrFirst); formLoaded = profiles.isNotEmpty() }
            selectedProfileId.isEmpty() && reloadAfterApply && added.size == 1 ->
                selectProfile(profiles.first { it.id in added })
            selectedProfileId.isNotEmpty() && current == null -> selectProfile(activeOrFirst)
            current != null && (reloadAfterApply || !formDirty) -> selectProfile(current)
        }
        reloadAfterApply = false
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp)
                .padding(bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(L10n.t("settings.title", en), style = MaterialTheme.typography.titleLarge)

            ScrollableTabRow(selectedTabIndex = tabIndex, edgePadding = 0.dp) {
                listOf(
                    L10n.t("settings.tab_appearance", en),
                    L10n.t("settings.tab_general", en),
                    L10n.t("settings.tab_models", en),
                    L10n.t("settings.tab_embedding", en),
                ).forEachIndexed { idx, title ->
                    Tab(selected = tabIndex == idx, onClick = { tabIndex = idx }, text = { Text(title) })
                }
            }

            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 260.dp, max = 420.dp)
                    .verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                when (tabIndex) {
                    0 -> {
                        Text(L10n.t("settings.language", en), style = MaterialTheme.typography.labelMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            listOf("auto" to (L10n.t("settings.system", en)), "zh-CN" to "简体中文", "en-US" to "English").forEach { (code, lbl) ->
                                FilterChip(
                                    selected = draftPrefs.language == code,
                                    onClick = { draftPrefs = draftPrefs.copy(language = code) },
                                    label = { Text(lbl) },
                                )
                            }
                        }

                        Text(L10n.t("settings.theme", en), style = MaterialTheme.typography.labelMedium)
                        LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            items(
                                listOf(
                                    "dark" to (L10n.t("settings.obsidian_dark", en)),
                                    "light" to (L10n.t("settings.theme_light", en)),
                                    "system" to (L10n.t("settings.system", en)),
                                    "midnight" to (L10n.t("settings.theme_midnight", en)),
                                    "forest" to (L10n.t("settings.theme_forest", en)),
                                    "warm_paper" to (L10n.t("settings.theme_warm_paper", en)),
                                    "rose_dusk" to (L10n.t("settings.theme_rose_dusk", en)),
                                ),
                                key = { it.first },
                            ) { (code, lbl) ->
                                FilterChip(
                                    selected = draftPrefs.theme == code,
                                    onClick = { draftPrefs = draftPrefs.copy(theme = code) },
                                    label = { Text(lbl) },
                                )
                            }
                        }

                        Text(L10n.t("settings.font_size", en), style = MaterialTheme.typography.labelMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            listOf(14, 15, 17).forEach { sz ->
                                FilterChip(
                                    selected = draftPrefs.fontSize == sz,
                                    onClick = { draftPrefs = draftPrefs.copy(fontSize = sz) },
                                    label = { Text("${sz}sp") },
                                )
                            }
                        }

                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.fullscreen_anim", en), modifier = Modifier.weight(1f))
                            Switch(checked = draftPrefs.fullscreenAnimation, onCheckedChange = { draftPrefs = draftPrefs.copy(fullscreenAnimation = it) })
                        }

                        Button(onClick = { onSavePrefs(draftPrefs) }, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.save", en))
                        }

                        HorizontalDivider()
                        Text(L10n.t("settings.token_limits_header", en), style = MaterialTheme.typography.titleSmall)
                        var tLecSlides by remember(tokenLimits) { mutableStateOf(tokenLimits.lectureSlides.toString()) }
                        var tLecOutline by remember(tokenLimits) { mutableStateOf(tokenLimits.lectureOutline.toString()) }
                        var tStudioDef by remember(tokenLimits) { mutableStateOf(tokenLimits.studioDefault.toString()) }
                        var tStudioAux by remember(tokenLimits) { mutableStateOf(tokenLimits.studioAux.toString()) }
                        var tAgentAux by remember(tokenLimits) { mutableStateOf(tokenLimits.agentAux.toString()) }
                        var tIngestIdx by remember(tokenLimits) { mutableStateOf(tokenLimits.ingestIndex.toString()) }
                        OutlinedTextField(value = tLecSlides, onValueChange = { tLecSlides = it }, label = { Text(L10n.t("settings.token_limit_lecture_slides", en)) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), singleLine = true, modifier = Modifier.fillMaxWidth())
                        OutlinedTextField(value = tLecOutline, onValueChange = { tLecOutline = it }, label = { Text(L10n.t("settings.token_limit_lecture_outline", en)) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), singleLine = true, modifier = Modifier.fillMaxWidth())
                        OutlinedTextField(value = tStudioDef, onValueChange = { tStudioDef = it }, label = { Text(L10n.t("settings.token_limit_studio_default", en)) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), singleLine = true, modifier = Modifier.fillMaxWidth())
                        OutlinedTextField(value = tStudioAux, onValueChange = { tStudioAux = it }, label = { Text(L10n.t("settings.token_limit_studio_aux", en)) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), singleLine = true, modifier = Modifier.fillMaxWidth())
                        OutlinedTextField(value = tAgentAux, onValueChange = { tAgentAux = it }, label = { Text(L10n.t("settings.token_limit_agent_aux", en)) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), singleLine = true, modifier = Modifier.fillMaxWidth())
                        OutlinedTextField(value = tIngestIdx, onValueChange = { tIngestIdx = it }, label = { Text(L10n.t("settings.token_limit_ingest_index", en)) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), singleLine = true, modifier = Modifier.fillMaxWidth())
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                            Button(
                                onClick = {
                                    onSaveTokenLimits(
                                        MaxTokenLimitsConfig(
                                            lectureSlides = tLecSlides.trim().toIntOrNull()?.coerceAtLeast(0) ?: 32768,
                                            lectureOutline = tLecOutline.trim().toIntOrNull()?.coerceAtLeast(0) ?: 16384,
                                            studioDefault = tStudioDef.trim().toIntOrNull()?.coerceAtLeast(0) ?: 0,
                                            studioAux = tStudioAux.trim().toIntOrNull()?.coerceAtLeast(0) ?: 8192,
                                            agentAux = tAgentAux.trim().toIntOrNull()?.coerceAtLeast(0) ?: 4096,
                                            ingestIndex = tIngestIdx.trim().toIntOrNull()?.coerceAtLeast(0) ?: 8192,
                                        )
                                    )
                                },
                                modifier = Modifier.weight(1f),
                            ) { Text(L10n.t("settings.save", en)) }
                            OutlinedButton(onClick = onResetTokenLimits, modifier = Modifier.weight(1f)) {
                                Text(L10n.t("settings.token_limits_reset", en))
                            }
                        }

                        HorizontalDivider()
                        Text(L10n.t("settings.debug_section", en), style = MaterialTheme.typography.titleSmall)
                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.debug_mode", en), modifier = Modifier.weight(1f))
                            Switch(checked = debugMode, onCheckedChange = onToggleDebugMode)
                        }
                        Text(
                            L10n.t("settings.debug_mode_hint", en),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        OutlinedButton(onClick = onExportLogs, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.export_logs", en))
                        }
                    }

                    1 -> {
                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.expand_thoughts", en), modifier = Modifier.weight(1f))
                            Switch(checked = draftPrefs.expandThoughtsByDefault, onCheckedChange = { draftPrefs = draftPrefs.copy(expandThoughtsByDefault = it) })
                        }
                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.show_suggestions", en), modifier = Modifier.weight(1f))
                            Switch(checked = draftPrefs.showSuggestedQuestions, onCheckedChange = { draftPrefs = draftPrefs.copy(showSuggestedQuestions = it) })
                        }
                        Text(L10n.t("settings.lecture_repair_rounds", en), style = MaterialTheme.typography.labelMedium)
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            OutlinedButton(onClick = { onSaveLectureRepairRounds(lectureRepairRounds - 1) }, enabled = lectureRepairRounds > 0) { Text("−") }
                            Text("$lectureRepairRounds", style = MaterialTheme.typography.titleMedium)
                            OutlinedButton(onClick = { onSaveLectureRepairRounds(lectureRepairRounds + 1) }, enabled = lectureRepairRounds < 20) { Text("+") }
                        }
                        Text(
                            L10n.t("settings.lecture_repair_rounds_hint", en),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Button(onClick = { onSavePrefs(draftPrefs) }, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.save", en))
                        }
                    }

                    2 -> {
                        if (selectedProfileId.isNotEmpty()) {
                            val cur = profiles.firstOrNull { it.id == selectedProfileId }
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                                // the test runs against what is stored, so it waits for Apply
                                OutlinedButton(
                                    onClick = { onTestProfile(selectedProfileId) },
                                    enabled = !formDirty,
                                    modifier = Modifier.weight(1f),
                                ) {
                                    Text(L10n.t("settings.test", en))
                                }
                                val curKref = cur?.keychainRef.orEmpty()
                                OutlinedButton(
                                    onClick = { if (curKref.isNotEmpty()) onClearKey(curKref) },
                                    enabled = curKref.isNotEmpty() && cur?.hasSecret != false,
                                    modifier = Modifier.weight(1f),
                                ) {
                                    Text(L10n.t("model.clear_key", en))
                                }
                                OutlinedButton(
                                    onClick = { confirmDelete = true },
                                    modifier = Modifier.weight(1f),
                                ) {
                                    Text(L10n.t("studio.delete", en))
                                }
                            }
                            if (formDirty) {
                                Text(
                                    L10n.t("model.apply_before_test", en),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                            if (confirmDelete) {
                                AlertDialog(
                                    onDismissRequest = { confirmDelete = false },
                                    title = { Text(cur?.displayName.orEmpty()) },
                                    text = { Text(L10n.t("model.confirm_delete", en)) },
                                    confirmButton = {
                                        TextButton(onClick = {
                                            confirmDelete = false
                                            onDeleteProfile(selectedProfileId)
                                        }) { Text(L10n.t("studio.delete", en)) }
                                    },
                                    dismissButton = {
                                        TextButton(onClick = { confirmDelete = false }) { Text(L10n.t("common.cancel", en)) }
                                    },
                                )
                            }
                        }
                        Text(
                            L10n.t("model.saved_profiles", en),
                            style = MaterialTheme.typography.labelMedium,
                        )
                        LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            item {
                                FilterChip(
                                    selected = selectedProfileId.isEmpty(),
                                    onClick = { selectProfile(null) },
                                    label = { Text(L10n.t("settings.new", en)) },
                                )
                            }
                            items(profiles, key = { it.id }) { prof ->
                                val tag = (if (prof.isActive) (L10n.t("settings.default", en)) else "") +
                                    (if (!prof.eligible) L10n.t("model.disabled_tag", en) else "")
                                FilterChip(
                                    selected = selectedProfileId == prof.id,
                                    onClick = { selectProfile(prof) },
                                    label = { Text("${prof.displayName}$tag") },
                                )
                            }
                        }
                        if (presets.isNotEmpty()) {
                            Text(
                                L10n.t("settings.provider_presets", en),
                                style = MaterialTheme.typography.labelMedium,
                            )
                            LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                items(presets, key = { it.id }) { preset ->
                                    FilterChip(
                                        selected = selectedPresetId == preset.id,
                                        onClick = {
                                            selectedPresetId = preset.id
                                            protocol = preset.protocol
                                            baseUrl = preset.baseUrl
                                            modelId = preset.defaultModel
                                            contextWindow = preset.contextWindow
                                            maxOutputText = preset.maxOutputTokens.takeIf { it > 0 }?.toString().orEmpty()
                                            extraHeaders = preset.extraHeaders.entries.map { it.key to it.value }
                                        },
                                        label = { Text(if (en) preset.nameEn else preset.name) },
                                    )
                                }
                            }
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf("chat_completion", "responses", "anthropic", "gemini").forEach { p ->
                                FilterChip(
                                    selected = protocol == p,
                                    onClick = { protocol = p },
                                    label = { Text(p.take(10)) },
                                )
                            }
                        }
                        OutlinedTextField(
                            value = baseUrl,
                            onValueChange = { baseUrl = it },
                            label = { Text(L10n.t("model.base_url", en)) },
                            placeholder = { Text("https://api.openai.com/v1") },
                            singleLine = true,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        OutlinedTextField(
                            value = modelId,
                            onValueChange = { modelId = it },
                            label = { Text(L10n.t("model.model_id", en)) },
                            singleLine = true,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        OutlinedTextField(
                            value = maxOutputText,
                            onValueChange = { v -> maxOutputText = v.filter { it.isDigit() }.take(7) },
                            label = { Text(L10n.t("model.max_output_tokens", en)) },
                            supportingText = { Text(L10n.t("model.unlimited_hint", en)) },
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                            singleLine = true,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        OutlinedTextField(
                            value = maxParallelText,
                            onValueChange = { v -> maxParallelText = v.filter { it.isDigit() }.take(2) },
                            label = { Text(L10n.t("model.max_parallel", en)) },
                            supportingText = { Text(L10n.t("model.max_parallel_tip", en)) },
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                            singleLine = true,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        OutlinedTextField(
                            value = apiKey,
                            onValueChange = { apiKey = it },
                            label = { Text(L10n.t("tts.api_key", en)) },
                            visualTransformation = PasswordVisualTransformation(),
                            singleLine = true,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(L10n.t("model.extra_headers", en), style = MaterialTheme.typography.labelLarge)
                            TextButton(onClick = { extraHeaders = extraHeaders + ("" to "") }) {
                                Text(L10n.t("model.add_header", en))
                            }
                        }
                        extraHeaders.forEachIndexed { idx, (hk, hv) ->
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.spacedBy(6.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                OutlinedTextField(
                                    value = hk,
                                    onValueChange = { v ->
                                        extraHeaders = extraHeaders.toMutableList().also { it[idx] = v to hv }
                                    },
                                    label = { Text(L10n.t("common.name", en)) },
                                    singleLine = true,
                                    modifier = Modifier.weight(1f),
                                )
                                OutlinedTextField(
                                    value = hv,
                                    onValueChange = { v ->
                                        extraHeaders = extraHeaders.toMutableList().also { it[idx] = hk to v }
                                    },
                                    label = { Text(L10n.t("settings.value", en)) },
                                    singleLine = true,
                                    modifier = Modifier.weight(1.2f),
                                )
                                IconButton(onClick = {
                                    extraHeaders = extraHeaders.toMutableList().also { it.removeAt(idx) }
                                }) {
                                    Icon(Icons.Default.Delete, contentDescription = null)
                                }
                            }
                        }
                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("model.set_as_default_chat_model", en), modifier = Modifier.weight(1f))
                            Switch(checked = activateChat, onCheckedChange = { activateChat = it })
                        }
                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.allow_non_loopback_http", en), modifier = Modifier.weight(1f))
                            Switch(checked = allowInsecureHttp, onCheckedChange = { allowInsecureHttp = it })
                        }
                        // inputs are detected by the core; read-only here
                        val curProfile = profiles.firstOrNull { it.id == selectedProfileId }
                        val detectedText = curProfile?.takeIf { it.probed }?.let { p ->
                            L10n.t("model.detected_inputs", en)
                                .replace("{0}", if (p.supportsVision) "✓" else "✗")
                                .replace("{1}", if (p.supportsAudio) "✓" else "✗")
                                .replace("{2}", if (p.supportsVideo) "✓" else "✗")
                        } ?: L10n.t("model.detect_pending", en)
                        Text(
                            detectedText,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (selectedPresetId == "mimo") {
                            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                                Text(L10n.t("model.shrink_media", en), modifier = Modifier.weight(1f))
                                Switch(checked = shrinkMedia, onCheckedChange = { shrinkMedia = it })
                            }
                            Text(
                                L10n.t("model.shrink_media_hint_android", en),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }

                        HorizontalDivider()
                        Text(
                            L10n.t("settings.per_modality_model_assignment", en),
                            style = MaterialTheme.typography.labelMedium,
                        )
                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                            Text(
                                L10n.t("settings.enable_native_audio_video_processing", en),
                                modifier = Modifier.weight(1f),
                            )
                            Switch(
                                checked = mediaRouting.audioVideoEnabled,
                                onCheckedChange = { onSaveMediaRouting(mediaRouting.copy(audioVideoEnabled = it)) },
                            )
                        }
                        Text(L10n.t("route.vision_image_pdf", en), style = MaterialTheme.typography.labelSmall)
                        LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            item {
                                FilterChip(
                                    selected = mediaRouting.visionProfileId.isEmpty(),
                                    onClick = { onSaveMediaRouting(mediaRouting.copy(visionProfileId = "")) },
                                    label = { Text(L10n.t("studio.cfg.level_auto", en)) },
                                )
                            }
                            items(profiles.filter { it.supportsVision }, key = { "v_${it.id}" }) { p ->
                                FilterChip(
                                    selected = mediaRouting.visionProfileId == p.id,
                                    onClick = { onSaveMediaRouting(mediaRouting.copy(visionProfileId = p.id)) },
                                    label = { Text(p.displayName) },
                                )
                            }
                        }
                        if (mediaRouting.audioVideoEnabled) {
                            Text(L10n.t("route.audio_2", en), style = MaterialTheme.typography.labelSmall)
                            LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                item {
                                    FilterChip(
                                        selected = mediaRouting.audioProfileId.isEmpty(),
                                        onClick = { onSaveMediaRouting(mediaRouting.copy(audioProfileId = "")) },
                                        label = { Text(L10n.t("studio.cfg.level_auto", en)) },
                                    )
                                }
                                items(profiles.filter { it.supportsAudio }, key = { "a_${it.id}" }) { p ->
                                    FilterChip(
                                        selected = mediaRouting.audioProfileId == p.id,
                                        onClick = { onSaveMediaRouting(mediaRouting.copy(audioProfileId = p.id)) },
                                        label = { Text(p.displayName) },
                                    )
                                }
                            }
                            Text(L10n.t("route.video_2", en), style = MaterialTheme.typography.labelSmall)
                            LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                item {
                                    FilterChip(
                                        selected = mediaRouting.videoProfileId.isEmpty(),
                                        onClick = { onSaveMediaRouting(mediaRouting.copy(videoProfileId = "")) },
                                        label = { Text(L10n.t("studio.cfg.level_auto", en)) },
                                    )
                                }
                                items(profiles.filter { it.supportsVideo }, key = { "vid_${it.id}" }) { p ->
                                    FilterChip(
                                        selected = mediaRouting.videoProfileId == p.id,
                                        onClick = { onSaveMediaRouting(mediaRouting.copy(videoProfileId = p.id)) },
                                        label = { Text(p.displayName) },
                                    )
                                }
                            }
                        }

                        profiles.firstOrNull { it.id == selectedProfileId }?.let { cur ->
                            val line = cur.unsupportedReason?.let { "⚠ $it" } ?: cur.capsSummary
                            if (line.isNotEmpty()) Text(
                                line,
                                style = MaterialTheme.typography.bodySmall,
                                color = if (cur.eligible) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.error,
                            )
                        }
                        Text(
                            L10n.t("model.requirement", en),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }

                    3 -> {
                        Text(
                            L10n.t("settings.semantic_search_uses_a_local", en),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Button(onClick = onDownloadEmbedding, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.download_enable_model", en))
                        }
                    }
                }
            }
            if (tabIndex == 2) {
                Button(
                    onClick = {
                        val hdrMap = extraHeaders
                            .mapNotNull { (k, v) -> k.trim().takeIf { it.isNotEmpty() }?.let { it to v.trim() } }
                            .toMap()
                        reloadAfterApply = true
                        onSaveModel(
                            selectedProfileId.ifEmpty { null },
                            protocol,
                            baseUrl,
                            modelId,
                            apiKey,
                            hdrMap,
                            contextWindow,
                            // empty or 0 = unlimited
                            (maxOutputText.toIntOrNull() ?: 0).coerceAtLeast(0),
                            activateChat,
                            allowInsecureHttp,
                            // empty or 0 = unlimited; the core is sent 0 either way
                            (maxParallelText.toIntOrNull() ?: 0).coerceIn(0, 64),
                            selectedPresetId,
                            selectedPresetId == "mimo" && shrinkMedia,
                            // a failed Apply must not leave the reload flag set: a later list
                            // refresh would then throw the user's edits away
                            { reloadAfterApply = false },
                        )
                    },
                    enabled = baseUrl.isNotBlank() && modelId.isNotBlank() && formDirty,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(L10n.t("settings.apply", en))
                }
            }
        }
    }
}
