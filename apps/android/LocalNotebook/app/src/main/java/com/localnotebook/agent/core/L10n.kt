package com.localnotebook.agent.core

import org.json.JSONObject
import java.util.Locale

data class UiPreferences(
    val language: String = "auto",           // "auto" | "zh-CN" | "en-US"
    val theme: String = "dark",              // "dark" | "light" | "system"
    val fontSize: Int = 15,                  // 14 | 15 | 17
    val fullscreenAnimation: Boolean = true,
    val expandThoughtsByDefault: Boolean = false,
    val showSuggestedQuestions: Boolean = true,
    val defaultStudioLang: String = "",
) {
    val isEnglish: Boolean
        get() = when (language) {
            "en-US" -> true
            "zh-CN" -> false
            else -> !Locale.getDefault().language.startsWith("zh", ignoreCase = true)
        }

    val targetLangCode: String
        get() = if (isEnglish) "en-US" else "zh-CN"

    fun toJson(): JSONObject = JSONObject()
        .put("language", language)
        .put("theme", theme)
        .put("font_size", fontSize)
        .put("fullscreen_animation", fullscreenAnimation)
        .put("expand_thoughts_by_default", expandThoughtsByDefault)
        .put("show_suggested_questions", showSuggestedQuestions)
        .put("default_studio_lang", defaultStudioLang)

    companion object {
        fun fromJson(obj: JSONObject?): UiPreferences {
            if (obj == null) return UiPreferences()
            val fs = obj.optInt("font_size", 15).let { if (it in listOf(14, 15, 17)) it else 15 }
            return UiPreferences(
                language = obj.optString("language", "auto").ifEmpty { "auto" },
                theme = obj.optString("theme", "dark").ifEmpty { "dark" },
                fontSize = fs,
                fullscreenAnimation = obj.optBoolean("fullscreen_animation", true),
                expandThoughtsByDefault = obj.optBoolean("expand_thoughts_by_default", false),
                showSuggestedQuestions = obj.optBoolean("show_suggested_questions", true),
                defaultStudioLang = obj.optString("default_studio_lang", ""),
            )
        }
    }
}

object L10n {
    fun t(key: String, en: Boolean): String =
        if (en) enMap[key] ?: zhMap[key] ?: key else zhMap[key] ?: key

    fun localizeDefaultArtifactTitle(title: String, en: Boolean): String = when (title) {
        "音频概览", "Audio Overview" -> t("studio.audio", en)
        "演示文稿", "Slides" -> t("studio.slides", en)
        "思维导图", "Mind Map" -> t("studio.mindmap", en)
        "知识点图谱", "Knowledge Graph" -> t("studio.knowledge_graph", en)
        "研究报告", "报告", "Report" -> t("studio.report", en)
        "学习闪卡", "闪卡", "Flashcards" -> t("studio.flashcards", en)
        "知识测验", "测验", "Quiz" -> t("studio.quiz", en)
        "信息图", "Infographic" -> t("studio.infographic", en)
        else -> title
    }

    fun advice(code: String, en: Boolean): String? = when (code) {
        "MODEL_AUTH_FAILED" -> if (en) "Invalid API key or permissions. Check Settings → AI Models." else "API Key 无效或没有权限，请在“设置 → AI 模型”中检查。"
        "MODEL_RATE_LIMITED" -> if (en) "Model service rate limit reached. Please try again shortly." else "模型服务请求过多，请稍后再试。"
        "MODEL_OVERLOADED" -> if (en) "Model service is temporarily unavailable. Please try again shortly." else "模型服务暂时不可用，请稍后再试。"
        "MODEL_NOT_CONFIGURED" -> if (en) "Please configure a model in Settings → AI Models first." else "请先在“设置 → AI 模型”中配置模型。"
        "NO_SOURCE_SELECTED" -> if (en) "Please select at least one ready source." else "请先勾选至少一个已就绪的来源。"
        "OUTPUT_INVALID" -> if (en) "Unexpected model output. Please try again." else "生成结果格式异常，请重新尝试。"
        else -> null
    }

    private val zhMap = mapOf(
        "app.title" to "Local Notebook Agent",
        "nb.new" to "新建笔记本",
        "nb.untitled" to "未命名笔记本",
        "tab.sources" to "来源",
        "tab.chat" to "对话",
        "tab.studio" to "Studio",
        "sources.add" to "添加来源",
        "sources.add_file" to "本地文件 (PDF / 文档 / 图片)",
        "sources.add_url" to "网页地址",
        "sources.add_paste" to "粘贴文本",
        "sources.create_note" to "新建笔记",
        "sources.select_all" to "全选来源",
        "sources.notes" to "笔记",
        "sources.guide" to "来源指南",
        "sources.empty" to "还没有来源，点击上方按钮导入文件、网页或粘贴文本。",
        "chat.empty_summary" to "请添加来源或直接提问。",
        "chat.placeholder" to "提问或创作内容...",
        "chat.you" to "你",
        "chat.assistant" to "助手",
        "cite.header" to "来源引用",
        "cite.open_page" to "跳转到第 %d 页",
        "cite.open_source" to "在来源中查看",
        "cite.unclear_pdf" to "该引用页包含图表、公式或排版内容，已为您打开 PDF 并跳转到第 %d 页。",
        "studio.audio" to "音频概览",
        "studio.slides" to "演示文稿",
        "studio.mindmap" to "思维导图",
        "studio.report" to "报告",
        "studio.flashcards" to "闪卡",
        "studio.quiz" to "测验",
        "studio.infographic" to "信息图",
        "studio.knowledge_graph" to "知识点图谱",
        "studio.prompt_header" to "提示",
        "studio.prompt_empty" to "基于所选来源自动生成",
        "studio.sources_header" to "来源",
        "studio.copy_prompt" to "复制提示",
        "studio.regenerate" to "重新生成",
        "studio.good" to "优质内容",
        "studio.bad" to "劣质内容",
        "studio.failed" to "生成失败，请重试。",
        "studio.retry" to "重试",
        "studio.diag_log" to "诊断日志",
        "studio.copy_log" to "复制日志",
        "studio.approve_draft" to "批准并生成演示文稿",
        "kg.build_btn" to "构建知识点图谱",
        "kg.recommended" to "AI 推荐：该材料结构清晰，适合构建知识点图谱",
        "kg.action_quiz" to "针对此知识点生成测验",
        "kg.action_mindmap" to "展开为详细思维导图",
        "kg.action_chat" to "在对话中深入讲解",
        "settings.title" to "设置",
        "settings.tab_appearance" to "外观",
        "settings.tab_general" to "通用",
        "settings.tab_models" to "AI 模型",
        "settings.tab_embedding" to "语义检索",
        "settings.language" to "界面语言",
        "settings.theme" to "主题外观",
        "settings.font_size" to "字体大小",
        "settings.fullscreen_anim" to "全屏切换缩放动画",
        "settings.expand_thoughts" to "默认展开思考过程",
        "settings.show_suggestions" to "显示建议问题",
        "settings.save" to "保存",
        "settings.saved" to "设置已保存",
        "common.close" to "关闭",
        "common.cancel" to "取消",
        "common.ok" to "确定",
    )

    private val enMap = mapOf(
        "app.title" to "Local Notebook Agent",
        "nb.new" to "New Notebook",
        "nb.untitled" to "Untitled Notebook",
        "tab.sources" to "Sources",
        "tab.chat" to "Chat",
        "tab.studio" to "Studio",
        "sources.add" to "Add source",
        "sources.add_file" to "Local file (PDF / Docs / Images)",
        "sources.add_url" to "Web URL",
        "sources.add_paste" to "Paste text",
        "sources.create_note" to "New note",
        "sources.select_all" to "Select all sources",
        "sources.notes" to "Notes",
        "sources.guide" to "Source Guide",
        "sources.empty" to "No sources yet. Tap Add source to import files, URLs, or text.",
        "chat.empty_summary" to "Add sources or start asking questions.",
        "chat.placeholder" to "Ask a question or create content...",
        "chat.you" to "You",
        "chat.assistant" to "Assistant",
        "cite.header" to "Source Reference",
        "cite.open_page" to "Jump to Page %d",
        "cite.open_source" to "View in Source",
        "cite.unclear_pdf" to "This cited page contains figures, equations, or layout content. Opened PDF at page %d.",
        "studio.audio" to "Audio Overview",
        "studio.slides" to "Slides",
        "studio.mindmap" to "Mind Map",
        "studio.report" to "Report",
        "studio.flashcards" to "Flashcards",
        "studio.quiz" to "Quiz",
        "studio.infographic" to "Infographic",
        "studio.knowledge_graph" to "Knowledge Graph",
        "studio.prompt_header" to "Prompt",
        "studio.prompt_empty" to "Generated from selected sources",
        "studio.sources_header" to "Sources",
        "studio.copy_prompt" to "Copy prompt",
        "studio.regenerate" to "Regenerate",
        "studio.good" to "Good content",
        "studio.bad" to "Bad content",
        "studio.failed" to "Generation failed. Please try again.",
        "studio.retry" to "Retry",
        "studio.diag_log" to "Diagnostic Log",
        "studio.copy_log" to "Copy Log",
        "studio.approve_draft" to "Approve & Generate Slides",
        "kg.build_btn" to "Build Knowledge Graph",
        "kg.recommended" to "AI Recommended: Great fit for a Knowledge Graph",
        "kg.action_quiz" to "Quiz on this concept",
        "kg.action_mindmap" to "Expand as Mind Map",
        "kg.action_chat" to "Explain in Chat",
        "settings.title" to "Settings",
        "settings.tab_appearance" to "Appearance",
        "settings.tab_general" to "General",
        "settings.tab_models" to "AI Models",
        "settings.tab_embedding" to "Semantic Search",
        "settings.language" to "Language",
        "settings.theme" to "Theme",
        "settings.font_size" to "Font Size",
        "settings.fullscreen_anim" to "Animate fullscreen transitions",
        "settings.expand_thoughts" to "Expand thoughts by default",
        "settings.show_suggestions" to "Show suggested questions",
        "settings.save" to "Save",
        "settings.saved" to "Settings saved",
        "common.close" to "Close",
        "common.cancel" to "Cancel",
        "common.ok" to "OK",
    )
}
