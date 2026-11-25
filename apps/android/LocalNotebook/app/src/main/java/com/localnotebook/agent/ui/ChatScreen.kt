package com.localnotebook.agent.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.localnotebook.agent.core.AppUiState
import com.localnotebook.agent.core.ChatMessageItem
import com.localnotebook.agent.core.CitationDetail
import com.localnotebook.agent.core.L10n

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ChatScreen(
    state: AppUiState,
    onSendChat: (String, org.json.JSONArray?) -> Unit,
    onClickCitation: (messageId: String, citationNo: Int) -> Unit,
    onDismissCitation: () -> Unit,
    onOpenCitationPdf: (sourceId: String, page: Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val en = state.prefs.isEnglish
    // the unsent text and attachments belong to the notebook they were typed in
    val draft = remember(state.activeNotebookId) { ChatDrafts.of(state.activeNotebookId) }
    var input by draft.text
    val attached = draft.attached

    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        // Summary card + horizontal FlowRow suggested questions
        Surface(
            color = MaterialTheme.colorScheme.surfaceVariant,
            shape = RoundedCornerShape(14.dp),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Column(modifier = Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                MarkdownText(
                    markdown = state.summaryText.ifEmpty { L10n.t("chat.empty_summary", en) },
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (state.prefs.showSuggestedQuestions && state.suggestedQuestions.isNotEmpty()) {
                    FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        for (q in state.suggestedQuestions.take(6)) {
                            SuggestionChip(
                                onClick = { onSendChat(q, null) },
                                label = { Text(q, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            )
                        }
                    }
                }
            }
        }

        // Chat message list
        LazyColumn(
            modifier = Modifier.weight(1f),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            items(state.messages, key = { it.id }) { msg ->
                ChatMessageCard(
                    message = msg,
                    en = en,
                    defaultExpandThoughts = state.prefs.expandThoughtsByDefault,
                    onClickCitation = { no -> onClickCitation(msg.id, no) },
                )
            }
        }

        // Input pill
        Surface(
            color = MaterialTheme.colorScheme.surfaceVariant,
            shape = RoundedCornerShape(26.dp),
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
            modifier = Modifier.fillMaxWidth(),
        ) {
          Column {
            AttachStrip(attached, en, Modifier.padding(start = 14.dp, end = 14.dp, top = 10.dp))
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.padding(start = 4.dp, end = 6.dp, top = 4.dp, bottom = 4.dp),
            ) {
                AttachImageButton(attached, en, enabled = !state.isSendingChat)
                TextField(
                    value = input,
                    onValueChange = { input = it },
                    placeholder = { Text(L10n.t("chat.placeholder", en)) },
                    colors = TextFieldDefaults.colors(
                        focusedContainerColor = Color.Transparent,
                        unfocusedContainerColor = Color.Transparent,
                        focusedIndicatorColor = Color.Transparent,
                        unfocusedIndicatorColor = Color.Transparent,
                    ),
                    modifier = Modifier.weight(1f),
                )
                FilledIconButton(
                    onClick = {
                        if (input.isNotBlank() || attached.hasItems) {
                            onSendChat(input, attached.toJson())
                            input = ""
                            attached.clear()
                        }
                    },
                    enabled = (input.isNotBlank() || attached.hasItems) && !state.isSendingChat,
                    shape = CircleShape,
                ) {
                    Icon(Icons.AutoMirrored.Filled.Send, contentDescription = null, modifier = Modifier.size(18.dp))
                }
            }
          }
        }
    }

    // Reference Popover Card (matches Wikipedia-style card & jumps to PDF for unclear/figure pages)
    state.activeCitation?.let { cit ->
        CitationReferenceDialog(
            detail = cit,
            en = en,
            onDismiss = onDismissCitation,
            onJumpToSource = {
                onDismissCitation()
                if (cit.sourceId.isNotEmpty()) {
                    onOpenCitationPdf(cit.sourceId, cit.pageStart.coerceAtLeast(1))
                }
            },
        )
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ChatMessageCard(
    message: ChatMessageItem,
    en: Boolean,
    defaultExpandThoughts: Boolean,
    onClickCitation: (Int) -> Unit,
) {
    var thoughtsExpanded by remember(message.id) { mutableStateOf(defaultExpandThoughts) }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.fillMaxWidth()) {
        Text(
            text = if (message.role == "user") L10n.t("chat.you", en) else L10n.t("chat.assistant", en),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        if (message.role == "assistant" && message.thoughts.isNotEmpty()) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .clickable { thoughtsExpanded = !thoughtsExpanded }
                    .padding(vertical = 2.dp),
            ) {
                Text("Thoughts", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Icon(
                    if (thoughtsExpanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(16.dp),
                )
            }
            if (thoughtsExpanded) {
                Surface(
                    color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f),
                    shape = RoundedCornerShape(10.dp),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Column(modifier = Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        for (th in message.thoughts) {
                            Text(th.title, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.SemiBold)
                            if (!th.isTool && th.detail.isNotEmpty()) {
                                Text(th.detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                }
            }
        }

        if (message.role == "user") {
            val pics = message.attachments.filter { it.kind == "image" }
            val others = message.attachments.filter { it.kind != "image" }
            if (pics.isNotEmpty()) AttachmentGallery(pics.map { it.path })
            if (others.isNotEmpty()) {
                val ctx = androidx.compose.ui.platform.LocalContext.current
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    for (a in others) {
                        AttachmentChip(
                            a.kind,
                            a.name,
                            if (a.kind == "link") a.url.orEmpty() else if (a.bytes > 0) "${maxOf(1, a.bytes / 1024)} KB" else "",
                            onClick = { openAttachment(ctx, a.kind, a.path, a.url) },
                        )
                    }
                }
            }
            if (message.content.isNotBlank()) Text(
                text = message.content,
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurface,
            )
        } else {
            MarkdownText(
                markdown = message.content,
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurface,
            )
        }

        if (message.citations.isNotEmpty()) {
            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                for (c in message.citations) {
                    Surface(
                        color = Color(0xFF2A2E36),
                        shape = RoundedCornerShape(10.dp),
                        border = BorderStroke(1.dp, Color(0xFF444A56)),
                        modifier = Modifier.clickable { onClickCitation(c.no) },
                    ) {
                        Text(
                            text = if (c.pageStart > 0) "${c.no} · p.${c.pageStart}" else "${c.no}",
                            color = Color(0xFFC6CEDC),
                            fontSize = 11.5.sp,
                            modifier = Modifier.padding(horizontal = 8.dp, vertical = 3.dp),
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun CitationReferenceDialog(
    detail: CitationDetail,
    en: Boolean,
    onDismiss: () -> Unit,
    onJumpToSource: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Icon(Icons.Default.Description, contentDescription = null, modifier = Modifier.size(18.dp))
                Text(L10n.t("cite.header", en), style = MaterialTheme.typography.titleMedium)
            }
        },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    text = if (detail.sectionPath.isNotEmpty()) "${detail.sourceTitle} · ${detail.sectionPath}" else detail.sourceTitle,
                    color = MaterialTheme.colorScheme.primary,
                    fontWeight = FontWeight.SemiBold,
                    style = MaterialTheme.typography.bodyMedium,
                )
                val body = if (detail.isUnclear && detail.pageStart > 0 && detail.sourceTitle.endsWith(".pdf", ignoreCase = true)) {
                    String.format(L10n.t("cite.unclear_pdf", en), detail.pageStart)
                } else {
                    detail.content.take(280)
                }
                if (body.isNotEmpty()) {
                    Text(body, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        },
        confirmButton = {
            if (!detail.removed && detail.sourceId.isNotEmpty()) {
                Button(onClick = onJumpToSource) {
                    Text(
                        if (detail.pageStart > 0) String.format(L10n.t("cite.open_page", en), detail.pageStart)
                        else L10n.t("cite.open_source", en),
                    )
                }
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(L10n.t("common.close", en)) }
        },
    )
}

/** Unsent chat input per notebook, kept while the app runs (switching notebooks or tabs keeps it). */
private class ChatDraft {
    val text = mutableStateOf("")
    val attached = ImageAttachState()
}

private object ChatDrafts {
    private val drafts = mutableMapOf<String, ChatDraft>()
    fun of(notebookId: String): ChatDraft = drafts.getOrPut(notebookId) { ChatDraft() }
}
