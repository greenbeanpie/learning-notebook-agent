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
import com.localnotebook.agent.core.ProviderPresetItem
import com.localnotebook.agent.core.UiPreferences

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun UnifiedSettingsSheet(
    prefs: UiPreferences,
    presets: List<ProviderPresetItem> = emptyList(),
    onSavePrefs: (UiPreferences) -> Unit,
    onSaveModel: (
        protocol: String,
        baseUrl: String,
        modelId: String,
        apiKey: String,
        extraHeaders: Map<String, String>,
        contextWindow: Int,
        maxOutputTokens: Int,
        supportsVision: Boolean,
        supportsAudio: Boolean,
        supportsVideo: Boolean,
    ) -> Unit,
    onDownloadEmbedding: () -> Unit,
    onDismiss: () -> Unit,
) {
    val en = prefs.isEnglish
    var tabIndex by remember { mutableIntStateOf(0) }
    var draftPrefs by remember(prefs) { mutableStateOf(prefs) }

    var selectedPresetId by remember { mutableStateOf("") }
    var protocol by remember { mutableStateOf("chat_completion") }
    var baseUrl by remember { mutableStateOf("") }
    var modelId by remember { mutableStateOf("") }
    var apiKey by remember { mutableStateOf("") }
    var contextWindow by remember { mutableIntStateOf(32000) }
    var maxOutputTokens by remember { mutableIntStateOf(8000) }
    var supportsVision by remember { mutableStateOf(false) }
    var supportsAudio by remember { mutableStateOf(false) }
    var supportsVideo by remember { mutableStateOf(false) }
    var extraHeaders by remember { mutableStateOf(listOf<Pair<String, String>>()) }

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
                            listOf("auto" to (if (en) "System" else "跟随系统"), "zh-CN" to "简体中文", "en-US" to "English").forEach { (code, lbl) ->
                                FilterChip(
                                    selected = draftPrefs.language == code,
                                    onClick = { draftPrefs = draftPrefs.copy(language = code) },
                                    label = { Text(lbl) },
                                )
                            }
                        }

                        Text(L10n.t("settings.theme", en), style = MaterialTheme.typography.labelMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            listOf("dark" to (if (en) "Dark" else "深色"), "light" to (if (en) "Light" else "浅色"), "system" to (if (en) "System" else "跟随系统")).forEach { (code, lbl) ->
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
                        if (presets.isNotEmpty()) {
                            Text(
                                if (en) "Provider Presets" else "提供商预设（快速填充）",
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
                            label = { Text("Base URL") },
                            placeholder = { Text("https://api.openai.com/v1") },
                            singleLine = true,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        OutlinedTextField(
                            value = modelId,
                            onValueChange = { modelId = it },
                            label = { Text("Model ID") },
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
                            Text(if (en) "Extra Headers" else "额外请求头", style = MaterialTheme.typography.labelLarge)
                            TextButton(onClick = { extraHeaders = extraHeaders + ("" to "") }) {
                                Text(if (en) "+ Add Header" else "+ 添加请求头")
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
                                    label = { Text(if (en) "Name" else "名称") },
                                    singleLine = true,
                                    modifier = Modifier.weight(1f),
                                )
                                OutlinedTextField(
                                    value = hv,
                                    onValueChange = { v ->
                                        extraHeaders = extraHeaders.toMutableList().also { it[idx] = hk to v }
                                    },
                                    label = { Text(if (en) "Value" else "值") },
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
                        Button(
                            onClick = {
                                val hdrMap = extraHeaders
                                    .mapNotNull { (k, v) -> k.trim().takeIf { it.isNotEmpty() }?.let { it to v.trim() } }
                                    .toMap()
                                onSaveModel(
                                    protocol,
                                    baseUrl,
                                    modelId,
                                    apiKey,
                                    hdrMap,
                                    contextWindow,
                                    maxOutputTokens,
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
                    }

                    3 -> {
                        Text(
                            if (en) "Semantic search uses a local multilingual embedding model so queries match across phrasings and languages."
                            else "语义检索使用本地多语言嵌入模型，让不同措辞或跨语言提问都能精准召回相关片段。",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Button(onClick = onDownloadEmbedding, modifier = Modifier.fillMaxWidth()) {
                            Text(if (en) "Download & Enable Model" else "下载并启用嵌入模型")
                        }
                    }
                }
            }
        }
    }
}
