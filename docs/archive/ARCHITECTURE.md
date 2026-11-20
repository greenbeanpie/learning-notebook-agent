# Local Notebook Agent 后端与跨平台低占用架构设计（完整业务逻辑链条版）

## 1. 设计目标与跨平台低占用技术选型

1. **极致低内存占用**：相比 Electron（空闲 250–450 MB RAM），目标桌面端空闲内存控制在 **40–80 MB RAM**，冷启动 `< 500 ms`。
2. **跨平台原生 UI + 共享核心**：
   - **Windows**：`.NET 8/9 + C# + WinUI 3 (Windows App SDK)`（NativeAOT + Mica/Acrylic 暗色材质，~45–65 MB RAM）。
   - **macOS**：`Swift + SwiftUI`（原生 Metal/AppKit 渲染，~35–55 MB RAM）。
   - **Linux**：`GTK4 / libadwaita`（`gtk4-rs`）或 `Tauri 2.0`（复用 OS WebKitGTK）。
   - **共享 Core 引擎 (`notebook-core`)**：Rust 编写，通过 **UniFFI** 编译为 `.dll` / `.dylib` / `.so` 或本地 Named Pipe / UDS 守护进程。
   - **单语言备选**：若全栈采用 C#，可使用 **.NET 8/9 NativeAOT** 作为 Core，Windows 用 **WinUI 3**，macOS/Linux 用 **Avalonia UI**。

---

## 2. 总体分层架构

```text
+-----------------------------------------------------------------------+
|                       Native UI Layer (WinUI 3 / SwiftUI / GTK4)      |
|  - 三栏布局 (Sources / Chat+Thoughts / Studio 8-Grid)                  |
|  - Studio 8 大生成选项弹窗状态机 & 进度条                              |
+-----------------------------------+-----------------------------------+
                                    |  UniFFI / Local IPC (JSON-RPC + SSE)
+-----------------------------------v-----------------------------------+
|                         Notebook Core Daemon                          |
|  1. Ingestion Engine   (PDF/MD/TXT 解析 -> 分块 -> FTS5 + Vector)      |
|  2. Hybrid RAG Engine  (BM25 + sqlite-vec -> RRF 重排 -> 引用锚点)     |
|  3. Agent Orchestrator (Thoughts 状态机 + 工具调用 + 上下文预算管理)   |
|  4. Protocol Router    (Chat Completion / Responses / Anthropic / Gemini)
|  5. Studio Worker Pool (异步生成 + 两阶段大纲/备注审核 + 结构化输出)   |
+-----------------------------------+-----------------------------------+
                                    |
+-----------------------------------v-----------------------------------+
|  SQLite WAL (notebooks, sources, source_chunks, chunks_fts, vec0,     |
|              chat_messages, studio_artifacts, model_profiles)         |
|  OS Keychain / Windows DPAPI (API Key 加密存储)                        |
+-----------------------------------------------------------------------+
```

---

## 3. 完整端到端业务逻辑链条

### 链条 A：笔记本初始化与来源摄取链路 (Ingestion Pipeline)
1. **触发**：用户在左栏点击 `+ 添加来源`（本地文件 / 网页 URL / 文本）或在搜索框输入关键词触发 `Web / Local` 检索。
2. **去重与落盘**：计算文件 SHA-256（`content_hash`），若已存在则跳过重复解析；否则写入 `sources` 表（默认 `is_selected = 1`）。
3. **格式解析**：
   - `PDF`：调用本地 `pdfium` / `lopdf` 按页提取文本与页码元数据。
   - `Markdown / TXT`：按标题层级（`#`, `##`）与段落边界切分。
4. **语义分块 (Chunking)**：按 `450 tokens` 目标长度、`60 tokens` 重叠窗口切分为 `source_chunks`。
5. **双路索引构建**：
   - 写入 `chunks_fts`（SQLite FTS5 `unicode61` 分词器）建立 BM25 词法索引。
   - 异步批处理调用本地量化 ONNX 模型（`bge-small-zh-v1.5`）或用户配置的 Embedding Endpoint，将向量写入 `chunks_vec`（`sqlite-vec`）。
6. **自动摘要生成**：当新增来源完成索引后，触发后台轻量任务调用当前激活的模型生成/刷新笔记本顶部 `Linear Algebra` 全局摘要，并通过 `NotebookSummaryUpdated` 事件推送给 UI。

### 链条 B：Agentic 对话、混合检索与四协议推理链路 (Chat & RAG Chain)
1. **输入组装**：用户在底部输入框发送问题，UI 将 `query` + 当前勾选的 `selected_source_ids[]` + `active_model_profile_id` 发往 Core。
2. **混合检索与 RRF 融合**：
   - 推送 `StreamEvent::ThoughtDelta("正在搜索您的来源...")` 到 UI 的 `Thoughts ^` 折叠区。
   - **词法召回**：在 `chunks_fts` 中按 `selected_source_ids` 过滤执行 BM25 Top-20。
   - **语义召回**：在 `chunks_vec` 中执行余弦相似度 Top-20。
   - **RRF 融合**：按 `score = 1/(60 + rank_bm25) + 1/(60 + rank_vec)` 重排取 Top-8 切片，并为每个切片分配顺序引用编号 `[1]`, `[2]`, ...。
3. **上下文预算与 Prompt 构造**：
   - 读取最近 N 轮 `chat_messages`，结合 `[1]..[8]` 切片构建带严格引用指令的 `UnifiedRequest`。
4. **四协议路由与流式归一化 (`ModelProtocolRouter`)**：
   - 从 Windows DPAPI / macOS Keychain 读取明文 API Key（内存驻留最小化，不落盘）。
   - 根据 `ModelProfile.protocol` 路由至对应适配器：
     - `ChatCompletion` -> 解析 `delta.reasoning_content` 与 `delta.content`
     - `ResponsesApi` -> 解析 `response.reasoning_summary_text.delta` 与 `response.output_text.delta`
     - `Anthropic` -> 解析 `thinking_delta` 与 `text_delta`
     - `Gemini` -> 解析 `part.thought == true` 与普通 `part.text`
   - 统一发射 `AgentStreamEvent`（`ThoughtDelta` / `TextDelta` / `UsageReported` / `Completed`）回传 UI。
5. **落库与引用绑定**：流式结束后，将完整回答、`thoughts_json` 与 `citations_json`（角标 `[1]` -> `chunk_id` + `page` 映射）写入 `chat_messages`。

### 链条 C：Studio 8 大模态异步生成与两阶段审核链路 (Studio Worker Chain)
1. **参数收集**：用户在右侧 Studio 点击任意功能卡片，弹出对应选项弹窗并提交配置：
   - **音频概览**：`format`（深入探究/摘要/评论/辩论）、`duration`（短/默认）、`lang`、主持人侧重点。
   - **演示文稿**：`format`（详细/幻灯片）、`duration`、`reviewOutline`（是否先检查大纲）。
   - **视频概览**：`format`（简洁版/说明视频）、侧重主题、`reviewNotes`（是否先检查制作者备注）。
   - **思维导图**：`lang`、主题约束。
   - **报告**：`format`（互动式/文档）、`template`（学习概览/执行简报）。
   - **抽认卡**：`count`、`difficulty`、`includeImages`、`lang`、主题。
   - **测验**：`count`、`difficulty`、`includeImages`、`questionTypes[]`（单选/多选/填空/简答）、`lang`、主题。
   - **信息图**：`lang`、`orientation`（横向/纵向/方形）、`visualStyle`（自动/可爱/泥塑/手绘/动漫等）、`detailLevel`（简短/标准/详细）。
2. **任务入队与 AI 用量预估**：
   - Core 计算选中来源 Token 总量 + 目标模态开销，更新 `AI 用量` 估算值。
   - 写入 `studio_artifacts`（`status = 'PENDING'`），立即返回 Artifact ID，UI 在右侧列表显示生成中占位符。
3. **两阶段生成分支（针对 `检查幻灯片大纲` / `检查制作者备注`）**：
   - 若开关开启：Worker 先生成结构化大纲/备注 JSON，将状态置为 `DRAFT_REVIEW` 并通知 UI；用户编辑确认后，再进入终稿渲染阶段。
   - 若开关关闭：直接通过各协议的结构化输出（JSON Schema / Tool Use）生成完整产物（如测验题库 JSON、思维导图树、双人播客剧本 + 本地 TTS 合成）。
4. **完成通知与交互预览**：
   - 状态更新为 `READY`，触发 OS 原生通知（若用户已开启通知），UI 点击即可在交互式预览器中作答测验、翻看闪卡或展开思维导图。

### 链条 D：桌面端生命周期与安全边界 (Lifecycle & Security)
1. **冷启动**：UI 进程启动 -> 拉起/连接 `notebook-core` -> SQLite `PRAGMA journal_mode=WAL` 恢复未完成事务（将残留 `PENDING` 任务标记为可重试）。
2. **隐私与安全**：所有文档原文、向量索引、聊天记录 100% 存储在本地应用数据目录；API Key 仅通过系统级凭据库（DPAPI / Keychain / Secret Service）加解密。
3. **优雅退出**：取消进行中的 HTTP SSE 流 -> Flush SQLite WAL Checkpoint -> 释放 ONNX Runtime 内存。
