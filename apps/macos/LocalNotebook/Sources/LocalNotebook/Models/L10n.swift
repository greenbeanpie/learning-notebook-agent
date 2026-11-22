import Foundation

struct UiPreferences {
    var language: String = "auto"           // "auto" | "zh-CN" | "en-US"
    var theme: String = "dark"              // "dark" | "light" | "system"
    var fontSize: Double = 15               // 14 | 15 | 17
    var fullscreenAnimation: Bool = true
    var expandThoughtsByDefault: Bool = false
    var showSuggestedQuestions: Bool = true
    var defaultStudioLang: String = ""      // "" | "zh-CN" | "en"

    var isEnglish: Bool {
        if language == "en-US" { return true }
        if language == "zh-CN" { return false }
        let preferred = Locale.preferredLanguages.first ?? "en"
        return !preferred.lowercased().hasPrefix("zh")
    }

    static func from(_ dict: [String: Any]?) -> UiPreferences {
        var p = UiPreferences()
        guard let d = dict else { return p }
        if let v = d["language"] as? String, !v.isEmpty { p.language = v }
        if let v = d["theme"] as? String, !v.isEmpty { p.theme = v }
        if let v = d["font_size"] as? Double { p.fontSize = v }
        else if let v = d["font_size"] as? Int { p.fontSize = Double(v) }
        if let v = d["fullscreen_animation"] as? Bool { p.fullscreenAnimation = v }
        if let v = d["expand_thoughts_by_default"] as? Bool { p.expandThoughtsByDefault = v }
        if let v = d["show_suggested_questions"] as? Bool { p.showSuggestedQuestions = v }
        if let v = d["default_studio_lang"] as? String { p.defaultStudioLang = v }
        return p
    }

    func toDict() -> [String: Any] {
        [
            "language": language,
            "theme": theme,
            "font_size": Int(fontSize),
            "fullscreen_animation": fullscreenAnimation,
            "expand_thoughts_by_default": expandThoughtsByDefault,
            "show_suggested_questions": showSuggestedQuestions,
            "default_studio_lang": defaultStudioLang,
        ]
    }
}

enum L10n {
    static func t(_ key: String, en: Bool) -> String {
        if en {
            return enMap[key] ?? zhMap[key] ?? key
        } else {
            return zhMap[key] ?? key
        }
    }

    static func advice(_ code: String, en: Bool) -> String? {
        switch (en, code) {
        case (true, "MODEL_AUTH_FAILED"): return "Invalid API key or permissions. Check Settings → AI Models."
        case (false, "MODEL_AUTH_FAILED"): return "API Key 无效或没有权限，请在“设置 → AI 模型”中检查。"
        case (true, "MODEL_RATE_LIMITED"): return "Model service rate limit reached. Please try again shortly."
        case (false, "MODEL_RATE_LIMITED"): return "模型服务请求过多，请稍后再试。"
        case (true, "MODEL_OVERLOADED"): return "Model service is temporarily unavailable. Please try again shortly."
        case (false, "MODEL_OVERLOADED"): return "模型服务暂时不可用，请稍后再试。"
        case (true, "MODEL_NOT_CONFIGURED"): return "Please configure a model in Settings → AI Models first."
        case (false, "MODEL_NOT_CONFIGURED"): return "请先在“设置 → AI 模型”中配置模型。"
        case (true, "NO_SOURCE_SELECTED"): return "Please select at least one ready source."
        case (false, "NO_SOURCE_SELECTED"): return "请在左侧勾选至少一个已就绪的来源。"
        case (true, "OUTPUT_INVALID"): return "Unexpected model output. Please try again."
        case (false, "OUTPUT_INVALID"): return "生成结果格式异常，请重新尝试。"
        default: return nil
        }
    }

    private static let zhMap: [String: String] = [
        "nb.new": "新建笔记本",
        "nb.untitled": "未命名笔记本",
        "sources.title": "来源",
        "sources.add": "+ 添加来源",
        "sources.add_file": "本地文件（文档 / 图片 / 音视频等）",
        "sources.add_paste": "粘贴文本",
        "sources.create_note": "+ 创建笔记",
        "chat.empty_summary": "请添加来源或直接提问。",
        "chat.placeholder": "提问或创作内容...",
        "chat.you": "你",
        "chat.assistant": "助手",
        "studio.audio": "音频概览",
        "studio.slides": "演示文稿",
        "studio.mindmap": "思维导图",
        "studio.report": "报告",
        "studio.flashcards": "闪卡",
        "studio.quiz": "测验",
        "studio.infographic": "信息图",
        "studio.knowledge_graph": "知识点图谱",
        "kg.build_btn": "构建知识点图谱",
        "kg.recommended": "AI 推荐：适合构建知识点图谱",
        "kg.action_quiz": "针对此知识点生成测验",
        "kg.action_mindmap": "展开为详细思维导图",
        "kg.action_chat": "在对话中深入讲解",
        "studio.prompt_header": "提示",
        "studio.prompt_empty": "基于所选来源自动生成",
        "studio.sources_header": "来源",
        "studio.sources_empty": "无关联来源",
        "studio.copy_prompt": "复制提示",
        "studio.regenerate": "重新生成",
        "studio.fullscreen": "全屏显示 / 还原",
        "studio.close": "关闭详情",
        "studio.good": "优质内容",
        "studio.bad": "劣质内容",
        "studio.failed": "生成失败，请重试。",
        "studio.retry": "重试",
        "studio.diag_log": "诊断日志",
        "studio.copy_log": "复制日志",
        "studio.toc": "目录",
        "studio.preview_draft": "预览提纲",
        "studio.edit_draft": "编辑提纲",
        "studio.approve_draft": "批准并生成演示文稿",
        "studio.submitted": "已提交生成任务。",
        "settings.title": "设置",
        "settings.tab_appearance": "外观与界面",
        "settings.tab_general": "通用与功能",
        "settings.tab_models": "AI 模型",
        "settings.tab_embedding": "语义检索",
        "settings.language": "界面语言",
        "settings.theme": "主题外观",
        "settings.font_size": "字体大小",
        "settings.fullscreen_anim": "切换全屏时启用缩放动画",
        "settings.expand_thoughts": "默认展开 AI 思考过程",
        "settings.show_suggestions": "显示后续提问建议",
        "settings.default_studio_lang": "Studio 默认输出语言",
        "settings.save": "保存",
        "settings.close": "关闭",
        "settings.saved": "设置已保存。",
    ]

    private static let enMap: [String: String] = [
        "nb.new": "New Notebook",
        "nb.untitled": "Untitled Notebook",
        "sources.title": "Sources",
        "sources.add": "+ Add source",
        "sources.add_file": "Local file (Docs / Images / Audio / Video)",
        "sources.add_paste": "Paste text",
        "sources.create_note": "+ New note",
        "chat.empty_summary": "Add sources or start asking questions.",
        "chat.placeholder": "Ask a question or create content...",
        "chat.you": "You",
        "chat.assistant": "Assistant",
        "studio.audio": "Audio Overview",
        "studio.slides": "Slides",
        "studio.mindmap": "Mind Map",
        "studio.report": "Report",
        "studio.flashcards": "Flashcards",
        "studio.quiz": "Quiz",
        "studio.infographic": "Infographic",
        "studio.knowledge_graph": "Knowledge Graph",
        "kg.build_btn": "Build Knowledge Graph",
        "kg.recommended": "AI Recommended: Build Knowledge Graph",
        "kg.action_quiz": "Quiz on this concept",
        "kg.action_mindmap": "Expand as Mind Map",
        "kg.action_chat": "Explain in Chat",
        "studio.prompt_header": "Prompt",
        "studio.prompt_empty": "Generated from selected sources",
        "studio.sources_header": "Sources",
        "studio.sources_empty": "No linked sources",
        "studio.copy_prompt": "Copy prompt",
        "studio.regenerate": "Regenerate",
        "studio.fullscreen": "Toggle fullscreen",
        "studio.close": "Close",
        "studio.good": "Good content",
        "studio.bad": "Bad content",
        "studio.failed": "Generation failed. Please try again.",
        "studio.retry": "Retry",
        "studio.diag_log": "Diagnostic Log",
        "studio.copy_log": "Copy Log",
        "studio.toc": "Contents",
        "studio.preview_draft": "Preview Outline",
        "studio.edit_draft": "Edit Outline",
        "studio.approve_draft": "Approve & Generate Slides",
        "studio.submitted": "Studio task submitted.",
        "settings.title": "Settings",
        "settings.tab_appearance": "Appearance",
        "settings.tab_general": "General",
        "settings.tab_models": "AI Models",
        "settings.tab_embedding": "Semantic Search",
        "settings.language": "Language",
        "settings.theme": "Theme",
        "settings.font_size": "Font Size",
        "settings.fullscreen_anim": "Animate fullscreen transitions",
        "settings.expand_thoughts": "Expand AI thoughts by default",
        "settings.show_suggestions": "Show suggested follow-up questions",
        "settings.default_studio_lang": "Default Studio language",
        "settings.save": "Save",
        "settings.close": "Close",
        "settings.saved": "Settings saved.",
    ]
}
