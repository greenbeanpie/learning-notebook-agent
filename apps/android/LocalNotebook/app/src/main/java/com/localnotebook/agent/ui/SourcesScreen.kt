package com.localnotebook.agent.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AutoAwesome
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.EditNote
import androidx.compose.material.icons.filled.PictureAsPdf
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.localnotebook.agent.core.AppUiState
import com.localnotebook.agent.core.L10n
import com.localnotebook.agent.core.SourceItem

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun SourcesScreen(
    state: AppUiState,
    onImportFile: (Uri) -> Unit,
    onAddUrl: (String) -> Unit,
    onAddText: (String, String) -> Unit,
    onCreateNote: (String, String) -> Unit,
    onToggleSource: (String, Boolean) -> Unit,
    onOpenSource: (SourceItem) -> Unit,
    onOpenPdf: (SourceItem, Int) -> Unit,
    onBuildKnowledgeGraph: (SourceItem) -> Unit,
    onAskTopic: (SourceItem, String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val en = state.prefs.isEnglish
    var showAddMenu by remember { mutableStateOf(false) }
    var showUrlDialog by remember { mutableStateOf(false) }
    var showPasteDialog by remember { mutableStateOf(false) }
    var showNoteDialog by remember { mutableStateOf(false) }

    val filePicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) onImportFile(uri)
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
            Box(modifier = Modifier.weight(1f)) {
                FilledTonalButton(onClick = { showAddMenu = true }, modifier = Modifier.fillMaxWidth()) {
                    Icon(Icons.Default.Add, contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(4.dp))
                    Text(L10n.t("sources.add", en), maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                DropdownMenu(expanded = showAddMenu, onDismissRequest = { showAddMenu = false }) {
                    DropdownMenuItem(
                        text = { Text(L10n.t("sources.add_file", en)) },
                        onClick = {
                            showAddMenu = false
                            filePicker.launch(arrayOf("*/*"))
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(L10n.t("sources.add_url", en)) },
                        onClick = { showAddMenu = false; showUrlDialog = true },
                    )
                    DropdownMenuItem(
                        text = { Text(L10n.t("sources.add_paste", en)) },
                        onClick = { showAddMenu = false; showPasteDialog = true },
                    )
                }
            }
            OutlinedButton(onClick = { showNoteDialog = true }, modifier = Modifier.weight(1f)) {
                Icon(Icons.Default.EditNote, contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(4.dp))
                Text(L10n.t("sources.create_note", en), maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }

        // Source detail / Guide view if one is opened
        val openSrc = state.activeSourcePreview
        if (openSrc != null) {
            SourceGuideDetailCard(
                source = openSrc,
                indexing = openSrc.id in state.indexingSourceIds,
                en = en,
                onClose = { onOpenSource(openSrc) },
                onOpenPdf = { onOpenPdf(openSrc, 1) },
                onBuildKnowledgeGraph = { onBuildKnowledgeGraph(openSrc) },
                onAskTopic = { t -> onAskTopic(openSrc, t) },
            )
        }

        LazyColumn(
            modifier = Modifier.weight(1f),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (state.sources.isEmpty()) {
                item {
                    Text(
                        L10n.t("sources.empty", en),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(8.dp),
                    )
                }
            } else {
                items(state.sources, key = { it.id }) { src ->
                    Surface(
                        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.55f),
                        shape = RoundedCornerShape(10.dp),
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(10.dp))
                            .clickable { onOpenSource(src) },
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
                        ) {
                            Icon(
                                if (src.title.endsWith(".pdf", ignoreCase = true)) Icons.Default.PictureAsPdf else Icons.Default.Description,
                                contentDescription = null,
                                tint = if (src.title.endsWith(".pdf", ignoreCase = true)) Color(0xFFF28B82) else Color(0xFF8AB4F8),
                                modifier = Modifier.size(20.dp),
                            )
                            Spacer(Modifier.width(10.dp))
                            Column(modifier = Modifier.weight(1f)) {
                                Text(src.title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium)
                                if (src.id in state.indexingSourceIds) {
                                    CircularProgressIndicator(modifier = Modifier.size(14.dp), strokeWidth = 2.dp)
                                } else if (src.status != "READY") {
                                    Text(src.status, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                }
                            }
                            Checkbox(
                                checked = src.isSelected,
                                onCheckedChange = { onToggleSource(src.id, it) },
                            )
                        }
                    }
                }
            }

            if (state.notes.isNotEmpty()) {
                item {
                    Text(
                        L10n.t("sources.notes", en),
                        style = MaterialTheme.typography.titleMedium,
                        modifier = Modifier.padding(top = 12.dp, bottom = 4.dp),
                    )
                }
                items(state.notes, key = { it.id }) { note ->
                    Surface(
                        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.55f),
                        shape = RoundedCornerShape(10.dp),
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.padding(horizontal = 10.dp, vertical = 10.dp),
                        ) {
                            Icon(Icons.Default.EditNote, contentDescription = null, tint = Color(0xFFFDD663), modifier = Modifier.size(20.dp))
                            Spacer(Modifier.width(10.dp))
                            Text(note.title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium)
                        }
                    }
                }
            }
        }
    }

    if (showUrlDialog) {
        var url by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { showUrlDialog = false },
            title = { Text(L10n.t("sources.add_url", en)) },
            text = { OutlinedTextField(value = url, onValueChange = { url = it }, placeholder = { Text("https://...") }, singleLine = true) },
            confirmButton = {
                TextButton(onClick = { onAddUrl(url); showUrlDialog = false }) { Text(L10n.t("common.ok", en)) }
            },
            dismissButton = {
                TextButton(onClick = { showUrlDialog = false }) { Text(L10n.t("common.cancel", en)) }
            },
        )
    }

    if (showPasteDialog) {
        var title by remember { mutableStateOf("") }
        var body by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { showPasteDialog = false },
            title = { Text(L10n.t("sources.add_paste", en)) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(value = title, onValueChange = { title = it }, label = { Text(L10n.t("sources.field_title", en)) }, singleLine = true)
                    OutlinedTextField(value = body, onValueChange = { body = it }, label = { Text(L10n.t("sources.field_text", en)) }, minLines = 4)
                }
            },
            confirmButton = {
                TextButton(onClick = { onAddText(title, body); showPasteDialog = false }) { Text(L10n.t("common.ok", en)) }
            },
            dismissButton = {
                TextButton(onClick = { showPasteDialog = false }) { Text(L10n.t("common.cancel", en)) }
            },
        )
    }

    if (showNoteDialog) {
        var title by remember { mutableStateOf("") }
        var body by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { showNoteDialog = false },
            title = { Text(L10n.t("sources.create_note", en)) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(value = title, onValueChange = { title = it }, label = { Text(L10n.t("sources.field_title", en)) }, singleLine = true)
                    OutlinedTextField(value = body, onValueChange = { body = it }, label = { Text("Markdown") }, minLines = 4)
                }
            },
            confirmButton = {
                TextButton(onClick = { onCreateNote(title, body); showNoteDialog = false }) { Text(L10n.t("common.ok", en)) }
            },
            dismissButton = {
                TextButton(onClick = { showNoteDialog = false }) { Text(L10n.t("common.cancel", en)) }
            },
        )
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun SourceGuideDetailCard(
    source: SourceItem,
    indexing: Boolean,
    en: Boolean,
    onClose: () -> Unit,
    onOpenPdf: () -> Unit,
    onBuildKnowledgeGraph: () -> Unit,
    onAskTopic: (String) -> Unit,
) {
    Surface(
        color = Color(0xFF242838),
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(1.dp, Color(0xFF383E56)),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Default.AutoAwesome, contentDescription = null, tint = Color(0xFFE0E8FF), modifier = Modifier.size(16.dp))
                Spacer(Modifier.width(6.dp))
                Text(L10n.t("sources.guide", en), fontWeight = FontWeight.SemiBold, color = Color(0xFFF1F3F4), modifier = Modifier.weight(1f))
                if (source.title.endsWith(".pdf", ignoreCase = true) && source.blobPath.isNotEmpty()) {
                    TextButton(onClick = onOpenPdf) { Text("PDF") }
                }
                TextButton(onClick = onClose) { Text(L10n.t("common.close", en)) }
            }

            if (source.guideSummaryMd.isEmpty() && indexing) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp)
                    Text(L10n.t("sources.guide_generating", en), style = MaterialTheme.typography.bodyMedium, color = Color(0xFFCED4E4))
                }
            }

            if (source.guideSummaryMd.isNotEmpty()) {
                MarkdownText(
                    markdown = source.guideSummaryMd,
                    style = MaterialTheme.typography.bodyMedium,
                    color = Color(0xFFCED4E4),
                )
            }

            // Horizontal flow-wrap layout for suggested topics (packs row-by-row, wraps only when needed)
            if (source.guideTopics.isNotEmpty()) {
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    for (topic in source.guideTopics) {
                        SuggestionChip(
                            onClick = { onAskTopic(topic) },
                            label = { Text(topic, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        )
                    }
                }
            }

            // AI Recommended Knowledge Graph learning tool bar
            Surface(
                color = if (source.recommendKnowledgeGraph) Color(0xFF223444) else Color(0xFF282C3C),
                shape = RoundedCornerShape(10.dp),
                border = BorderStroke(1.dp, if (source.recommendKnowledgeGraph) Color(0xFF5286BA) else Color(0xFF3E445C)),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.padding(10.dp),
                ) {
                    Text(
                        if (source.recommendKnowledgeGraph) L10n.t("kg.recommended", en) else L10n.t("studio.knowledge_graph", en),
                        style = MaterialTheme.typography.labelMedium,
                        color = Color(0xFFD4E4F8),
                        modifier = Modifier.weight(1f),
                    )
                    Button(onClick = onBuildKnowledgeGraph, contentPadding = PaddingValues(horizontal = 12.dp, vertical = 4.dp)) {
                        Text(L10n.t("kg.build_btn", en), style = MaterialTheme.typography.labelMedium)
                    }
                }
            }
        }
    }
}
