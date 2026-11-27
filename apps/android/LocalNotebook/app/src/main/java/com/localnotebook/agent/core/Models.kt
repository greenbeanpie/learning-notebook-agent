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

data class ChatAttachment(val name: String, val path: String, val kind: String = "image", val url: String? = null, val bytes: Long = 0, val id: String = "")

data class ChatMessageItem(
    val id: String,
    val role: String,
    val content: String,
    val thoughts: List<ThoughtStepItem>,
    val citations: List<CitationRef>,
    val attachments: List<ChatAttachment> = emptyList(),
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
    /** The user's own pictures (local paths) and file names given with the request. */
    val imagePaths: List<String> = emptyList(),
    val fileNames: List<String> = emptyList(),
    val draftMarkdown: String,
    val payload: JSONObject,
    val config: JSONObject,
    val errorCode: String,
    val errorDetail: String,
    val errorLog: String,
    val outputDir: String = "",
    val progressState: org.json.JSONObject = org.json.JSONObject(),
    /** Unix ms when the artifact was created (the Studio list shows its age). */
    val createdAt: Long = 0L,
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
    /** 0 = unlimited. */
    val maxOutputTokens: Int,
    val extraHeaders: Map<String, String>,
)

data class ModelProfileItem(
    val id: String,
    val displayName: String,
    val protocol: String,
    val baseUrl: String,
    val modelId: String,
    val keychainRef: String,
    val hasSecret: Boolean,
    val isActive: Boolean,
    val allowInsecureHttp: Boolean,
    val contextWindow: Int,
    /** Max output tokens; 0 = unlimited. */
    val maxOutputTokens: Int,
    /** Most model requests of this profile in flight at once; 0 = unlimited. */
    val maxParallel: Int = 0,
    /** Auto-detected by the core (read-only here); meaningful only when [probed]. */
    val supportsVision: Boolean,
    val supportsAudio: Boolean,
    val supportsVideo: Boolean,
    /** The core has detected inputs for this profile (capabilities.probed_at is set). */
    val probed: Boolean = false,
    val extraHeaders: Map<String, String>,
    /** Provider preset id the profile was created from (e.g. "mimo"), or null. */
    val preset: String? = null,
    /** Re-encode oversized audio/video before import (MiMo): Media3 on Android, ffmpeg on desktop. */
    val shrinkMedia: Boolean = false,
    /** Supports tool calling and structured output (or is an embedding endpoint). */
    val eligible: Boolean = true,
    val unsupportedReason: String? = null,
    val probePending: Boolean = false,
    val capsSummary: String = "",
)

data class MediaRoutingConfig(
    val audioVideoEnabled: Boolean = true,
    val visionProfileId: String = "",
    val audioProfileId: String = "",
    val videoProfileId: String = "",
)
