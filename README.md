# Local Notebook Agent — 设计与架构产出包

本压缩包包含一个对齐 Google Gemini Notebook (NotebookLM) 界面的本地自定义模型笔记本 Agent 的前端 HTML Demo、跨平台低占用后端架构设计文档，以及四大模型协议（Chat Completion / Responses / Anthropic / Gemini）的后端适配器参考骨架代码。

## 目录结构

```text
local-notebook-agent/
├── README.md                                  # 本说明文件
├── frontend-demo/
│   └── index.html                             # 独立可运行的 HTML Demo（直接用浏览器打开即可）
├── docs/
│   ├── ARCHITECTURE.md                        # 跨平台低内存后端架构设计（Rust Core + WinUI 3 / SwiftUI / .NET NativeAOT）
│   └── PROTOCOL_ADAPTERS.md                   # 四大模型协议报文映射与 SSE 流式事件归一化规范
└── backend-skeleton/
    ├── csharp/
    │   └── ProtocolAdapters.cs                # .NET 8 / C# (NativeAOT 兼容) 四协议适配器骨架
    ├── rust/
    │   └── protocol_adapter.rs                # Rust 轻量核心四协议 Trait 与流式事件定义
    └── sql/
        └── schema.sql                         # SQLite + FTS5 + sqlite-vec 本地知识库与笔记 Schema
```

## 快速体验前端 Demo

直接双击打开 `frontend-demo/index.html`（无需 Node.js 或本地服务器）：
- **三栏布局**：左侧 `来源`（支持勾选过滤、添加本地来源）、中间对话区（含可折叠 `Thoughts` 推理链与角标引用 `[1]`）、右侧 `Studio`（8 宫格功能卡片 + 生成列表）。
- **测验 (Quiz) 弹窗**：点击右侧 Studio 的 `测验` 卡片，可体验与原版一致的参数配置弹窗并生成可交互测验题。
- **4 协议模型配置**：点击顶部模型徽章或右上角设置齿轮 `⚙️`，可切换 `Chat Completion`、`Responses API`、`Anthropic`、`Gemini` 四种协议并实时预览请求报文映射。
