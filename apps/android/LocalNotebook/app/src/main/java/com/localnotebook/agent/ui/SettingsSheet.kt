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
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.localnotebook.agent.core.L10n
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
        supportsVision: Boolean,
        supportsAudio: Boolean,
        supportsVideo: Boolean,
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
    var selectedProfileId by remember(profiles) { mutableStateOf(activeOrFirst?.id ?: "") }
    var selectedPresetId by remember { mutableStateOf("") }
    var protocol by remember(selectedProfileId) { mutableStateOf(activeOrFirst?.protocol ?: "chat_completion") }
    var baseUrl by remember(selectedProfileId) { mutableStateOf(activeOrFirst?.baseUrl ?: "") }
    var modelId by remember(selectedProfileId) { mutableStateOf(activeOrFirst?.modelId ?: "") }
    var apiKey by remember { mutableStateOf("") }
    var contextWindow by remember(selectedProfileId) { mutableIntStateOf(activeOrFirst?.contextWindow ?: 32000) }
    var maxOutputTokens by remember(selectedProfileId) { mutableIntStateOf(activeOrFirst?.maxOutputTokens ?: 8000) }
    var activateChat by remember(selectedProfileId) { mutableStateOf(activeOrFirst?.isActive ?: true) }
    var allowInsecureHttp by remember(selectedProfileId) { mutableStateOf(activeOrFirst?.allowInsecureHttp ?: false) }
    var supportsVision by remember(selectedProfileId) { mutableStateOf(activeOrFirst?.supportsVision ?: false) }
    var supportsAudio by remember(selectedProfileId) { mutableStateOf(activeOrFirst?.supportsAudio ?: false) }
    var supportsVideo by remember(selectedProfileId) { mutableStateOf(activeOrFirst?.supportsVideo ?: false) }
    var extraHeaders by remember(selectedProfileId) {
        mutableStateOf<List<Pair<String, String>>>(activeOrFirst?.extraHeaders?.entries?.map { it.key to it.value } ?: emptyList())
    }

    fun selectProfile(p: ModelProfileItem?) {
        selectedProfileId = p?.id ?: ""
        protocol = p?.protocol ?: "chat_completion"
        baseUrl = p?.baseUrl ?: ""
        modelId = p?.modelId ?: ""
        apiKey = ""
        contextWindow = p?.contextWindow ?: 32000
        maxOutputTokens = p?.maxOutputTokens ?: 8000
        activateChat = p?.isActive ?: !profiles.any { it.isActive }
        allowInsecureHttp = p?.allowInsecureHttp ?: false
        supportsVision = p?.supportsVision ?: false
        supportsAudio = p?.supportsAudio ?: false
        supportsVideo = p?.supportsVideo ?: false
        extraHeaders = p?.extraHeaders?.entries?.map { it.key to it.value } ?: emptyList<Pair<String, String>>()
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
                        Button(onClick = { onSavePrefs(draftPrefs) }, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("settings.save", en))
                        }
                    }

                    2 -> {
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
                                            maxOutputTokens = preset.maxOutputTokens
                                            supportsVision = preset.supportsVision
                                            supportsAudio = preset.supportsAudio
                                            supportsVideo = preset.supportsVideo
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
                            value = apiKey,
                            onValueChange = { apiKey = it },
                            label = { Text("API Key") },
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
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            FilterChip(
                                selected = supportsVision,
                                onClick = { supportsVision = !supportsVision },
                                label = { Text(L10n.t("settings.vision", en)) },
                            )
                            FilterChip(
                                selected = supportsAudio,
                                onClick = { supportsAudio = !supportsAudio },
                                label = { Text(L10n.t("model.audio", en)) },
                            )
                            FilterChip(
                                selected = supportsVideo,
                                onClick = { supportsVideo = !supportsVideo },
                                label = { Text(L10n.t("model.video", en)) },
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
                        Button(
                            onClick = {
                                val hdrMap = extraHeaders
                                    .mapNotNull { (k, v) -> k.trim().takeIf { it.isNotEmpty() }?.let { it to v.trim() } }
                                    .toMap()
                                onSaveModel(
                                    selectedProfileId.ifEmpty { null },
                                    protocol,
                                    baseUrl,
                                    modelId,
                                    apiKey,
                                    hdrMap,
                                    contextWindow,
                                    maxOutputTokens,
                                    activateChat,
                                    allowInsecureHttp,
                                    supportsVision,
                                    supportsAudio,
                                    supportsVideo,
                                )
                            },
                            enabled = baseUrl.isNotBlank() && modelId.isNotBlank(),
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(L10n.t("settings.save", en))
                        }
                        if (selectedProfileId.isNotEmpty()) {
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                                OutlinedButton(
                                    onClick = { onTestProfile(selectedProfileId) },
                                    modifier = Modifier.weight(1f),
                                ) {
                                    Text(L10n.t("settings.test", en))
                                }
                                val curKref = profiles.firstOrNull { it.id == selectedProfileId }?.keychainRef.orEmpty()
                                OutlinedButton(
                                    onClick = { if (curKref.isNotEmpty()) onClearKey(curKref) },
                                    enabled = curKref.isNotEmpty(),
                                    modifier = Modifier.weight(1f),
                                ) {
                                    Text(L10n.t("model.clear_key", en))
                                }
                                OutlinedButton(
                                    onClick = { onDeleteProfile(selectedProfileId) },
                                    modifier = Modifier.weight(1f),
                                ) {
                                    Text(L10n.t("studio.delete", en))
                                }
                            }
                        }
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
        }
    }
}
