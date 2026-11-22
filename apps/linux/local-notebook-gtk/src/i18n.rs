use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct UiPreferences {
    pub language: String, // "auto" | "zh-CN" | "en-US"
    pub theme: String,    // "dark" | "light"
    pub font_size: i32,   // 13 | 14 | 16
    pub fullscreen_animation: bool,
    pub expand_thoughts_by_default: bool,
    pub show_suggested_questions: bool,
    pub default_studio_lang: String,
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme: "dark".into(),
            font_size: 15,
            fullscreen_animation: true,
            expand_thoughts_by_default: false,
            show_suggested_questions: true,
            default_studio_lang: String::new(),
        }
    }
}

impl UiPreferences {
    pub fn from_json(v: &Value) -> Self {
        let mut p = Self::default();
        if let Some(s) = v.get("language").and_then(Value::as_str) {
            if !s.is_empty() {
                p.language = s.to_string();
            }
        }
        if let Some(s) = v.get("theme").and_then(Value::as_str) {
            if !s.is_empty() {
                p.theme = s.to_string();
            }
        }
        if let Some(n) = v.get("font_size").and_then(Value::as_i64) {
            if matches!(n, 13 | 14 | 16) {
                p.font_size = n as i32;
            }
        }
        if let Some(b) = v.get("fullscreen_animation").and_then(Value::as_bool) {
            p.fullscreen_animation = b;
        }
        if let Some(b) = v.get("expand_thoughts_by_default").and_then(Value::as_bool) {
            p.expand_thoughts_by_default = b;
        }
        if let Some(b) = v.get("show_suggested_questions").and_then(Value::as_bool) {
            p.show_suggested_questions = b;
        }
        if let Some(s) = v.get("default_studio_lang").and_then(Value::as_str) {
            p.default_studio_lang = s.to_string();
        }
        p
    }

    pub fn to_json(&self) -> Value {
        json!({
            "language": self.language,
            "theme": self.theme,
            "font_size": self.font_size,
            "fullscreen_animation": self.fullscreen_animation,
            "expand_thoughts_by_default": self.expand_thoughts_by_default,
            "show_suggested_questions": self.show_suggested_questions,
            "default_studio_lang": self.default_studio_lang,
        })
    }

    pub fn is_english(&self) -> bool {
        match self.language.as_str() {
            "en-US" => true,
            "zh-CN" => false,
            _ => {
                let env_lang = std::env::var("LC_ALL")
                    .or_else(|_| std::env::var("LANG"))
                    .unwrap_or_default();
                !env_lang.to_lowercase().starts_with("zh")
            }
        }
    }
}

pub fn tr(en: bool, key: &str) -> &'static str {
    if en {
        match key {
            "nb.new" => "New Notebook",
            "nb.untitled" => "Untitled Notebook",
            "nb.rename" => "Rename Notebook",
            "nb.clear_chat" => "Clear Chat",
            "nb.delete" => "Delete Notebook",
            "sources.title" => "Sources",
            "sources.add" => "+ Add source",
            "sources.add_paste" => "Paste text",
            "sources.add_url" => "Web URL",
            "sources.create_note" => "+ New note",
            "chat.empty_summary" => "Add sources or start asking questions.",
            "chat.placeholder" => "Ask a question or create content...",
            "chat.send" => "Send",
            "chat.you" => "You",
            "chat.assistant" => "Assistant",
            "studio.audio" => "Audio Overview",
            "studio.slides" => "Slides",
            "studio.mindmap" => "Mind Map",
            "studio.report" => "Report",
            "studio.flashcards" => "Flashcards",
            "studio.quiz" => "Quiz",
            "studio.infographic" => "Infographic",
            "studio.knowledge_graph" => "Knowledge Graph",
            "kg.build_btn" => "Build Knowledge Graph",
            "kg.recommended" => "AI Recommended: Build Knowledge Graph",
            "kg.action_quiz" => "Quiz on this concept",
            "kg.action_mindmap" => "Expand as Mind Map",
            "kg.action_chat" => "Explain in Chat",
            "studio.prompt_header" => "Prompt",
            "studio.prompt_empty" => "Generated from selected sources",
            "studio.sources_header" => "Sources",
            "studio.sources_empty" => "No linked sources",
            "studio.copy_prompt" => "Copy prompt",
            "studio.regenerate" => "Regenerate",
            "studio.fullscreen" => "Toggle fullscreen",
            "studio.close" => "Close",
            "studio.good" => "Good content",
            "studio.bad" => "Bad content",
            "studio.failed" => "Generation failed. Please try again.",
            "studio.retry" => "Retry",
            "studio.diag_log" => "Diagnostic Log",
            "studio.copy_log" => "Copy Log",
            "studio.toc" => "Contents",
            "studio.approve_draft" => "Approve & Generate Slides",
            "studio.submitted" => "Studio task submitted.",
            "settings.title" => "Settings",
            "settings.tab_appearance" => "Appearance",
            "settings.tab_general" => "General",
            "settings.tab_models" => "AI Models",
            "settings.tab_embedding" => "Semantic Search",
            "settings.language" => "Language",
            "settings.theme" => "Theme",
            "settings.font_size" => "Font Size",
            "settings.fullscreen_anim" => "Animate fullscreen transitions",
            "settings.expand_thoughts" => "Expand AI thoughts by default",
            "settings.show_suggestions" => "Show suggested follow-up questions",
            "settings.default_studio_lang" => "Default Studio language",
            "settings.save" => "Save",
            "settings.saved" => "Settings saved.",
            "status.ready" => "Ready",
            _ => "",
        }
    } else {
        match key {
            "nb.new" => "新建笔记本",
            "nb.untitled" => "未命名笔记本",
            "nb.rename" => "重命名笔记本",
            "nb.clear_chat" => "清空对话",
            "nb.delete" => "删除笔记本",
            "sources.title" => "来源",
            "sources.add" => "+ 添加来源",
            "sources.add_paste" => "粘贴文本",
            "sources.add_url" => "网页地址",
            "sources.create_note" => "+ 创建笔记",
            "chat.empty_summary" => "请添加来源或直接提问。",
            "chat.placeholder" => "提问或创作内容...",
            "chat.send" => "发送",
            "chat.you" => "你",
            "chat.assistant" => "助手",
            "studio.audio" => "音频概览",
            "studio.slides" => "演示文稿",
            "studio.mindmap" => "思维导图",
            "studio.report" => "报告",
            "studio.flashcards" => "闪卡",
            "studio.quiz" => "测验",
            "studio.infographic" => "信息图",
            "studio.knowledge_graph" => "知识点图谱",
            "kg.build_btn" => "构建知识点图谱",
            "kg.recommended" => "AI 推荐：适合构建知识点图谱",
            "kg.action_quiz" => "针对此知识点生成测验",
            "kg.action_mindmap" => "展开为详细思维导图",
            "kg.action_chat" => "在对话中深入讲解",
            "studio.prompt_header" => "提示",
            "studio.prompt_empty" => "基于所选来源自动生成",
            "studio.sources_header" => "来源",
            "studio.sources_empty" => "无关联来源",
            "studio.copy_prompt" => "复制提示",
            "studio.regenerate" => "重新生成",
            "studio.fullscreen" => "全屏显示 / 还原",
            "studio.close" => "关闭详情",
            "studio.good" => "优质内容",
            "studio.bad" => "劣质内容",
            "studio.failed" => "生成失败，请重试。",
            "studio.retry" => "重试",
            "studio.diag_log" => "诊断日志",
            "studio.copy_log" => "复制日志",
            "studio.toc" => "目录",
            "studio.approve_draft" => "批准并生成演示文稿",
            "studio.submitted" => "已提交 Studio 生成任务。",
            "settings.title" => "设置",
            "settings.tab_appearance" => "外观与界面",
            "settings.tab_general" => "通用与功能",
            "settings.tab_models" => "AI 模型",
            "settings.tab_embedding" => "语义检索",
            "settings.language" => "界面语言",
            "settings.theme" => "主题外观",
            "settings.font_size" => "界面字体大小",
            "settings.fullscreen_anim" => "切换全屏时启用缩放动画",
            "settings.expand_thoughts" => "默认展开 AI 思考过程",
            "settings.show_suggestions" => "显示后续提问建议",
            "settings.default_studio_lang" => "Studio 默认输出语言",
            "settings.save" => "保存",
            "settings.saved" => "设置已保存。",
            "status.ready" => "就绪",
            _ => "",
        }
    }
}

pub fn user_error_advice(en: bool, code: &str) -> Option<&'static str> {
    Some(match (en, code) {
        (true, "MODEL_AUTH_FAILED") => "Invalid API key or permissions. Check Settings → AI Models.",
        (false, "MODEL_AUTH_FAILED") => "API Key 无效或没有权限，请在“设置 → AI 模型”中检查。",
        (true, "MODEL_RATE_LIMITED") => "Model service rate limit reached. Please try again shortly.",
        (false, "MODEL_RATE_LIMITED") => "模型服务请求过多，请稍后再试。",
        (true, "MODEL_OVERLOADED") => "Model service is temporarily unavailable. Please try again shortly.",
        (false, "MODEL_OVERLOADED") => "模型服务暂时不可用，请稍后再试。",
        (true, "MODEL_NOT_CONFIGURED") => "Please configure a model in Settings → AI Models first.",
        (false, "MODEL_NOT_CONFIGURED") => "请先在“设置 → AI 模型”中配置模型。",
        (true, "NO_SOURCE_SELECTED") => "Please select at least one ready source.",
        (false, "NO_SOURCE_SELECTED") => "请在左侧勾选至少一个已就绪的来源。",
        (true, "OUTPUT_INVALID") => "Unexpected model output. Please try again.",
        (false, "OUTPUT_INVALID") => "生成结果格式异常，请重新尝试。",
        _ => return None,
    })
}
