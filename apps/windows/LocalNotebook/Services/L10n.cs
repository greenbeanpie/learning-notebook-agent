using System.Globalization;
using System.Text.Json;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Services;

public sealed class UiPreferences
{
    public string Language { get; set; } = "auto";           // "auto" | "zh-CN" | "en-US"
    public string Theme { get; set; } = "dark";              // "dark" | "light" | "system"
    public int FontSize { get; set; } = 15;                  // 14 | 15 | 17
    public bool FullscreenAnimation { get; set; } = true;
    public bool ExpandThoughtsByDefault { get; set; } = false;
    public bool ShowSuggestedQuestions { get; set; } = true;
    public string DefaultStudioLang { get; set; } = "";      // "" | "zh-CN" | "en"

    public static UiPreferences FromJson(JsonElement el)
    {
        var p = new UiPreferences();
        if (el.ValueKind != JsonValueKind.Object) return p;
        var lang = Str(el, "language");
        if (lang.Length > 0) p.Language = lang;
        var theme = Str(el, "theme");
        if (theme.Length > 0) p.Theme = theme;
        var fs = (int)Int(el, "font_size", 15);
        if (fs is 13 or 14 or 15 or 16 or 17) p.FontSize = fs;
        if (el.TryGetProperty("fullscreen_animation", out var fa) && fa.ValueKind is JsonValueKind.True or JsonValueKind.False)
            p.FullscreenAnimation = fa.GetBoolean();
        if (el.TryGetProperty("expand_thoughts_by_default", out var et) && et.ValueKind is JsonValueKind.True or JsonValueKind.False)
            p.ExpandThoughtsByDefault = et.GetBoolean();
        if (el.TryGetProperty("show_suggested_questions", out var sq) && sq.ValueKind is JsonValueKind.True or JsonValueKind.False)
            p.ShowSuggestedQuestions = sq.GetBoolean();
        if (el.TryGetProperty("default_studio_lang", out var dsl) && dsl.ValueKind == JsonValueKind.String)
            p.DefaultStudioLang = dsl.GetString() ?? "";
        return p;
    }

    public JsonElement ToJson() => Args(
        ("language", Language),
        ("theme", Theme),
        ("font_size", FontSize),
        ("fullscreen_animation", FullscreenAnimation),
        ("expand_thoughts_by_default", ExpandThoughtsByDefault),
        ("show_suggested_questions", ShowSuggestedQuestions),
        ("default_studio_lang", DefaultStudioLang)
    );
}

public static class L10n
{
    public static UiPreferences Prefs { get; set; } = new();

    public static bool IsEnglish
    {
        get
        {
            if (Prefs.Language == "en-US") return true;
            if (Prefs.Language == "zh-CN") return false;
            return !CultureInfo.CurrentUICulture.Name.StartsWith("zh", StringComparison.OrdinalIgnoreCase);
        }
    }

    public static string T(string key)
    {
        var dict = IsEnglish ? En : Zh;
        if (dict.TryGetValue(key, out var val)) return val;
        if (Zh.TryGetValue(key, out var fb)) return fb;
        return key;
    }

    public static string Fmt(string key, params object[] args) =>
        string.Format(CultureInfo.CurrentCulture, T(key), args);

    public static string Advice(string code) => code switch
    {
        "MODEL_AUTH_FAILED" => T("err.model_auth_failed"),
        "MODEL_RATE_LIMITED" => T("err.model_rate_limited"),
        "MODEL_OVERLOADED" => T("err.model_overloaded"),
        "MODEL_NOT_FOUND" => T("err.model_not_found"),
        "MODEL_REFUSED" => T("err.model_refused"),
        "MODEL_STREAM_INTERRUPTED" => T("err.model_stream_interrupted"),
        "MODEL_CONTEXT_EXCEEDED" => T("err.model_context_exceeded"),
        "MODEL_NOT_CONFIGURED" => T("err.model_not_configured"),
        "NO_SOURCE_SELECTED" => T("err.no_source_selected"),
        "FETCH_FAILED" => T("err.fetch_failed"),
        "SOURCE_NEEDS_OCR" => T("err.source_needs_ocr"),
        "SOURCE_ENCRYPTED" => T("err.source_encrypted"),
        "SOURCE_UNSUPPORTED" => T("err.source_unsupported"),
        "SOURCE_TOO_LARGE" => T("err.source_too_large"),
        "SOURCE_LIMIT_REACHED" => T("err.source_limit_reached"),
        "URL_BLOCKED" => T("err.url_blocked"),
        "CAPABILITY_MISSING" => T("err.capability_missing"),
        "OUTPUT_INVALID" => T("err.output_invalid"),
        _ => "",
    };

    private static readonly Dictionary<string, string> Zh = new()
    {
        ["app.title"] = "Local Notebook Agent",
        ["nb.untitled"] = "未命名笔记本",
        ["nb.new"] = "新建笔记本",
        ["nb.new_title"] = "新笔记本",
        ["nb.switch"] = "切换笔记本",
        ["nb.menu"] = "笔记本菜单",
        ["nb.rename"] = "重命名笔记本",
        ["nb.export_lna"] = "导出为 .lna",
        ["nb.export_md"] = "导出为 Markdown",
        ["nb.import_lna"] = "导入 .lna",
        ["nb.clear_chat"] = "清空对话",
        ["nb.delete"] = "删除笔记本",
        ["nb.export_diag"] = "导出诊断包",
        ["nb.privacy"] = "隐私说明",

        ["sources.title"] = "来源",
        ["sources.add"] = "+ 添加来源",
        ["sources.add_file"] = "本地文件（文档 / 图片 / 音视频等）",
        ["sources.add_url"] = "网页地址",
        ["sources.add_paste"] = "粘贴文本",
        ["sources.create_note"] = "+ 创建笔记",

        ["chat.empty_summary"] = "请添加来源或直接提问。",
        ["chat.placeholder"] = "提问或创作内容...",
        ["chat.send"] = "发送",
        ["chat.stop"] = "停止生成",

        ["studio.title"] = "Studio",
        ["studio.audio"] = "音频概览",
        ["studio.slides"] = "演示文稿",
        ["studio.mindmap"] = "思维导图",
        ["studio.report"] = "报告",
        ["studio.flashcards"] = "闪卡",
        ["studio.quiz"] = "测验",
        ["studio.infographic"] = "信息图",
        ["studio.empty_hint"] = "选择上方的类型，把已勾选的来源生成为测验、闪卡、导图、报告等。",
        ["studio.view_prompt_sources"] = "查看提示和 {0} 个来源",
        ["studio.view_prompt"] = "查看提示",
        ["studio.prompt_header"] = "提示",
        ["studio.prompt_empty"] = "基于所选来源自动生成",
        ["studio.sources_header"] = "来源",
        ["studio.sources_empty"] = "无关联来源",
        ["studio.copy_prompt"] = "复制提示",
        ["studio.prompt_copied"] = "已复制提示",
        ["studio.regenerate"] = "重新生成",
        ["studio.export"] = "导出",
        ["studio.export_as"] = "导出为 {0}",
        ["studio.fullscreen_toggle"] = "全屏显示 / 还原",
        ["studio.close_detail"] = "关闭详情",
        ["studio.thoughts"] = "思考过程",
        ["studio.back_to_artifact"] = "返回成品",
        ["studio.good_content"] = "优质内容",
        ["studio.bad_content"] = "劣质内容",
        ["studio.feedback_received"] = "已收到反馈",
        ["studio.gen_failed"] = "生成失败",
        ["studio.gen_failed_msg"] = "生成失败，请重试。",
        ["studio.retry"] = "重试",
        ["studio.retrying"] = "重试中...",
        ["studio.view_process"] = "查看思考过程",
        ["studio.copy_log"] = "复制日志",
        ["studio.log_copied"] = "已复制日志",
        ["studio.log_copied_msg"] = "诊断日志已复制到剪贴板。",
        ["studio.diag_log"] = "诊断日志（点击展开）",
        ["studio.process_records"] = "生成过程记录",
        ["studio.cancelled"] = "已停止生成。",
        ["studio.status_pending"] = "排队中",
        ["studio.status_collect"] = "收集资料",
        ["studio.status_plan"] = "规划大纲",
        ["studio.status_generate"] = "生成中",
        ["studio.status_render"] = "渲染中",
        ["studio.status_processing"] = "处理中",
        ["studio.status_draft"] = "等待审核大纲",
        ["studio.status_ready"] = "已完成",
        ["studio.status_failed"] = "失败",
        ["studio.status_cancelled"] = "已取消",

        ["mindmap.expand_collapse_all"] = "全部展开 / 收起",
        ["mindmap.zoom_in"] = "放大",
        ["mindmap.zoom_out"] = "缩小",
        ["mindmap.download"] = "下载思维导图 (SVG)",
        ["mindmap.empty"] = "没有导图节点。",

        ["kg.title"] = "知识点图谱",
        ["kg.build_btn"] = "构建知识点图谱",
        ["kg.recommended_banner"] = "AI 推荐：这份材料概念体系清晰，适合构建知识点图谱",
        ["kg.badge_hint"] = "知识点图谱 · 点击节点可测验 / 展开导图",
        ["kg.prereq"] = "前置知识",
        ["kg.action_quiz"] = "针对此知识点生成测验",
        ["kg.action_mindmap"] = "展开为详细思维导图",
        ["kg.action_chat"] = "在对话中深入讲解",

        ["settings.title"] = "设置",
        ["settings.tab_appearance"] = "外观与界面",
        ["settings.tab_general"] = "通用与功能",
        ["settings.tab_models"] = "AI 模型",
        ["settings.tab_embedding"] = "语义检索",
        ["settings.language"] = "界面语言",
        ["settings.lang_auto"] = "跟随系统",
        ["settings.lang_zh"] = "简体中文",
        ["settings.lang_en"] = "English",
        ["settings.theme"] = "主题外观",
        ["settings.theme_dark"] = "深色",
        ["settings.theme_light"] = "浅色",
        ["settings.theme_system"] = "跟随系统",
        ["settings.font_size"] = "界面字体大小",
        ["settings.font_compact"] = "紧凑 (13px)",
        ["settings.font_normal"] = "标准 (14px)",
        ["settings.font_large"] = "宽松 (16px)",
        ["settings.fullscreen_anim"] = "切换全屏时启用缩放动画",
        ["settings.expand_thoughts"] = "默认展开 AI 思考过程",
        ["settings.show_suggestions"] = "在摘要卡片中显示后续提问建议",
        ["settings.default_studio_lang"] = "Studio 默认输出语言",
        ["settings.studio_lang_source"] = "跟随来源",
        ["settings.saved"] = "设置已保存。",
        ["settings.no_model_title"] = "尚未配置模型",
        ["settings.no_model_msg"] = "请点击右上角“设置”，在“AI 模型”中填写协议、地址、模型与 API Key。",
        ["settings.privacy_btn"] = "查看隐私说明",
        ["settings.diag_btn"] = "导出诊断包",

        ["common.ok"] = "确定",
        ["common.cancel"] = "取消",
        ["common.close"] = "关闭",
        ["common.save"] = "保存",
        ["common.delete"] = "删除",
        ["common.failed"] = "{0}失败",
        ["common.exported"] = "已导出",

        ["err.model_auth_failed"] = "API Key 无效或没有权限，请在“设置 → AI 模型”中检查密钥。",
        ["err.model_rate_limited"] = "模型服务请求过多，请稍后再试。",
        ["err.model_overloaded"] = "模型服务暂时不可用，请稍后再试。",
        ["err.model_not_found"] = "找不到该模型或接口地址，请检查模型 ID 与 Base URL。",
        ["err.model_refused"] = "模型拒绝回答这个问题，可以换一种问法再试。",
        ["err.model_stream_interrupted"] = "回答中途中断，已保留已生成的内容；可点击“重新生成”。",
        ["err.model_context_exceeded"] = "内容超出模型上下文，请减少勾选的来源或缩短问题。",
        ["err.model_not_configured"] = "请先在“设置 → AI 模型”中配置模型。",
        ["err.no_source_selected"] = "请在左侧勾选至少一个已就绪的来源。",
        ["err.fetch_failed"] = "无法连接服务，请检查网络或接口地址。",
        ["err.source_needs_ocr"] = "这个 PDF 为图片或扫描件，可在“设置 → AI 模型”中启用图文视觉模型后再试。",
        ["err.source_encrypted"] = "这个 PDF 有密码保护，请先解除保护再导入。",
        ["err.source_unsupported"] = "无法解析这个文件。",
        ["err.source_too_large"] = "文件或文本超出大小限制。",
        ["err.source_limit_reached"] = "单个笔记本最多支持 300 个来源。",
        ["err.url_blocked"] = "出于安全原因，不能导入本机或内网地址。",
        ["err.capability_missing"] = "缺少所需组件，请检查相关设置。",
        ["err.output_invalid"] = "生成结果格式异常，请重新尝试。",
    };

    private static readonly Dictionary<string, string> En = new()
    {
        ["app.title"] = "Local Notebook Agent",
        ["nb.untitled"] = "Untitled Notebook",
        ["nb.new"] = "New Notebook",
        ["nb.new_title"] = "New Notebook",
        ["nb.switch"] = "Switch notebook",
        ["nb.menu"] = "Notebook menu",
        ["nb.rename"] = "Rename notebook",
        ["nb.export_lna"] = "Export as .lna",
        ["nb.export_md"] = "Export as Markdown",
        ["nb.import_lna"] = "Import .lna",
        ["nb.clear_chat"] = "Clear chat",
        ["nb.delete"] = "Delete notebook",
        ["nb.export_diag"] = "Export diagnostics",
        ["nb.privacy"] = "Privacy notice",

        ["sources.title"] = "Sources",
        ["sources.add"] = "+ Add source",
        ["sources.add_file"] = "Local file (Docs / Images / Audio / Video)",
        ["sources.add_url"] = "Web URL",
        ["sources.add_paste"] = "Paste text",
        ["sources.create_note"] = "+ New note",

        ["chat.empty_summary"] = "Add sources or start asking questions.",
        ["chat.placeholder"] = "Ask a question or create content...",
        ["chat.send"] = "Send",
        ["chat.stop"] = "Stop generating",

        ["studio.title"] = "Studio",
        ["studio.audio"] = "Audio Overview",
        ["studio.slides"] = "Slides",
        ["studio.mindmap"] = "Mind Map",
        ["studio.report"] = "Report",
        ["studio.flashcards"] = "Flashcards",
        ["studio.quiz"] = "Quiz",
        ["studio.infographic"] = "Infographic",
        ["studio.empty_hint"] = "Pick a format above to turn selected sources into quizzes, flashcards, mind maps, or reports.",
        ["studio.view_prompt_sources"] = "View prompt and {0} sources",
        ["studio.view_prompt"] = "View prompt",
        ["studio.prompt_header"] = "Prompt",
        ["studio.prompt_empty"] = "Generated from selected sources",
        ["studio.sources_header"] = "Sources",
        ["studio.sources_empty"] = "No linked sources",
        ["studio.copy_prompt"] = "Copy prompt",
        ["studio.prompt_copied"] = "Prompt copied",
        ["studio.regenerate"] = "Regenerate",
        ["studio.export"] = "Export",
        ["studio.export_as"] = "Export as {0}",
        ["studio.fullscreen_toggle"] = "Toggle fullscreen",
        ["studio.close_detail"] = "Close",
        ["studio.thoughts"] = "Thoughts",
        ["studio.back_to_artifact"] = "Back to view",
        ["studio.good_content"] = "Good content",
        ["studio.bad_content"] = "Bad content",
        ["studio.feedback_received"] = "Feedback recorded",
        ["studio.gen_failed"] = "Generation failed",
        ["studio.gen_failed_msg"] = "Generation failed. Please try again.",
        ["studio.retry"] = "Retry",
        ["studio.retrying"] = "Retrying...",
        ["studio.view_process"] = "View thoughts",
        ["studio.copy_log"] = "Copy log",
        ["studio.log_copied"] = "Log copied",
        ["studio.log_copied_msg"] = "Diagnostic log copied to clipboard.",
        ["studio.diag_log"] = "Diagnostic Log (click to expand)",
        ["studio.process_records"] = "Generation Process",
        ["studio.cancelled"] = "Generation stopped.",
        ["studio.status_pending"] = "Queued",
        ["studio.status_collect"] = "Collecting sources",
        ["studio.status_plan"] = "Planning outline",
        ["studio.status_generate"] = "Generating",
        ["studio.status_render"] = "Rendering",
        ["studio.status_processing"] = "Processing",
        ["studio.status_draft"] = "Review outline",
        ["studio.status_ready"] = "Ready",
        ["studio.status_failed"] = "Failed",
        ["studio.status_cancelled"] = "Cancelled",

        ["mindmap.expand_collapse_all"] = "Expand / Collapse all",
        ["mindmap.zoom_in"] = "Zoom in",
        ["mindmap.zoom_out"] = "Zoom out",
        ["mindmap.download"] = "Download Mind Map (SVG)",
        ["mindmap.empty"] = "No mind map nodes.",

        ["kg.title"] = "Knowledge Graph",
        ["kg.build_btn"] = "Build Knowledge Graph",
        ["kg.recommended_banner"] = "AI Recommended: Structured concepts ideal for a Knowledge Graph",
        ["kg.badge_hint"] = "Knowledge Graph · Click any node to Quiz / Expand",
        ["kg.prereq"] = "Prerequisites",
        ["kg.action_quiz"] = "Quiz on this concept",
        ["kg.action_mindmap"] = "Expand as Mind Map",
        ["kg.action_chat"] = "Explain in Chat",

        ["settings.title"] = "Settings",
        ["settings.tab_appearance"] = "Appearance",
        ["settings.tab_general"] = "General",
        ["settings.tab_models"] = "AI Models",
        ["settings.tab_embedding"] = "Semantic Search",
        ["settings.language"] = "Language",
        ["settings.lang_auto"] = "Follow system",
        ["settings.lang_zh"] = "简体中文",
        ["settings.lang_en"] = "English",
        ["settings.theme"] = "Theme",
        ["settings.theme_dark"] = "Dark",
        ["settings.theme_light"] = "Light",
        ["settings.theme_system"] = "System",
        ["settings.font_size"] = "Interface Font Size",
        ["settings.font_compact"] = "Compact (13px)",
        ["settings.font_normal"] = "Standard (14px)",
        ["settings.font_large"] = "Large (16px)",
        ["settings.fullscreen_anim"] = "Animate fullscreen transitions",
        ["settings.expand_thoughts"] = "Expand AI thoughts by default",
        ["settings.show_suggestions"] = "Show follow-up question suggestions",
        ["settings.default_studio_lang"] = "Default Studio output language",
        ["settings.studio_lang_source"] = "Follow sources",
        ["settings.saved"] = "Settings saved.",
        ["settings.no_model_title"] = "No model configured",
        ["settings.no_model_msg"] = "Open Settings → AI Models to configure protocol, endpoint, model ID, and API key.",
        ["settings.privacy_btn"] = "Privacy Notice",
        ["settings.diag_btn"] = "Export Diagnostics",

        ["common.ok"] = "OK",
        ["common.cancel"] = "Cancel",
        ["common.close"] = "Close",
        ["common.save"] = "Save",
        ["common.delete"] = "Delete",
        ["common.failed"] = "{0} failed",
        ["common.exported"] = "Exported",

        ["err.model_auth_failed"] = "Invalid API key or insufficient permissions. Check Settings → AI Models.",
        ["err.model_rate_limited"] = "Model service rate limit reached. Please try again shortly.",
        ["err.model_overloaded"] = "Model service is temporarily unavailable. Please try again shortly.",
        ["err.model_not_found"] = "Model or endpoint not found. Check your Model ID and Base URL.",
        ["err.model_refused"] = "The model declined to answer this request. Try rephrasing.",
        ["err.model_stream_interrupted"] = "Response was interrupted. Partial output has been kept; you can regenerate.",
        ["err.model_context_exceeded"] = "Content exceeds the model's context window. Deselect some sources or shorten your prompt.",
        ["err.model_not_configured"] = "Please configure a model in Settings → AI Models first.",
        ["err.no_source_selected"] = "Please select at least one ready source on the left.",
        ["err.fetch_failed"] = "Unable to connect to the service. Check your network or endpoint URL.",
        ["err.source_needs_ocr"] = "This PDF appears to be scanned. Enable a vision-capable model in Settings → AI Models and try again.",
        ["err.source_encrypted"] = "This PDF is password-protected. Remove the password before importing.",
        ["err.source_unsupported"] = "Unable to parse this file format.",
        ["err.source_too_large"] = "File or text exceeds the size limit.",
        ["err.source_limit_reached"] = "A notebook can hold up to 300 sources.",
        ["err.url_blocked"] = "Local or private network URLs cannot be imported for security reasons.",
        ["err.capability_missing"] = "A required component is missing.",
        ["err.output_invalid"] = "The model returned an unexpected format. Please try again.",
    };
}
