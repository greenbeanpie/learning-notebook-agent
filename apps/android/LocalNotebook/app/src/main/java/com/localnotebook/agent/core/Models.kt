package com.localnotebook.agent.core

import org.json.JSONObject

data class NotebookItem(
    val id: String,
    val title: String,
    val summaryMd: String,
    val suggestedQuestions: List<String>,
)

data class SourceItem(
    val id: String,
    val title: String,
    val kind: String,
    val status: String,
    val isSelected: Boolean,
    val blobPath: String,
    val pageCount: Int,
    val guideSummaryMd: String,
    val guideTopics: List<String>,
    val recommendKnowledgeGraph: Boolean,
)

data class NoteItem(
    val id: String,
    val title: String,
    val bodyMd: String,
)

data class ThoughtStepItem(
    val id: String = "",
    val kind: String,
    val title: String,
    val detail: String = "",
    val seq: Int = 0,
    val body: String = "",
) {
    val isTool: Boolean
        get() = kind in setOf("retrieve", "retrieve_fallback", "tool_search", "tool_read_context", "tool_list_sources", "tool_call")
}

data class CitationRef(
    val no: Int,
    val sourceTitle: String,
    val pageStart: Int,
    val quote: String,
)

data class CitationDetail(
    val no: Int,
    val sourceId: String,
    val sourceTitle: String,
    val sectionPath: String,
    val pageStart: Int,
    val content: String,
    val isUnclear: Boolean,
    val removed: Boolean,
)

data class ChatMessageItem(
    val id: String,
    val role: String,
    val content: String,
    val thoughts: List<ThoughtStepItem>,
    val citations: List<CitationRef>,
)

data class ArtifactSourceRef(
    val id: String,
    val title: String,
    val kind: String,
)

data class StudioArtifactItem(
    val id: String,
    val artifactType: String,
    val title: String,
    val status: String,
    val stage: String,
    val progress: Float,
    val promptTopic: String,
    val isKnowledgeGraph: Boolean,
    val sourceRefs: List<ArtifactSourceRef>,
    val sourceCount: Int,
    val draftMarkdown: String,
    val payload: JSONObject,
    val config: JSONObject,
    val errorCode: String,
    val errorDetail: String,
    val errorLog: String,
)

data class PdfPreviewTarget(
    val sourceId: String,
    val title: String,
    val filePath: String,
    val initialPage: Int = 1,
)

data class ProviderPresetItem(
    val id: String,
    val name: String,
    val nameEn: String,
    val protocol: String,
    val baseUrl: String,
    val defaultModel: String,
    val contextWindow: Int,
    val maxOutputTokens: Int,
    val supportsVision: Boolean,
    val supportsAudio: Boolean,
    val supportsVideo: Boolean,
    val extraHeaders: Map<String, String>,
)
