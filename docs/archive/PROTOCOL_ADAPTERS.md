# 四大模型协议适配器规范 (Protocol Adapters)

后端通过统一的 `IModelProtocolAdapter`（C#）或 `ModelProvider`（Rust）抽象，将四种异构协议映射为统一的内部请求 `UnifiedChatRequest` 与流式事件 `AgentStreamEvent`。

---

## 1. 协议端点与鉴权对照表

| 协议类型 | 默认 Endpoint Path | 鉴权 Header | Thinking / 推理字段 |
| :--- | :--- | :--- | :--- |
| **Chat Completion** | `POST {base}/chat/completions` | `Authorization: Bearer {key}` | `delta.reasoning_content` (兼容 DeepSeek/vLLM) |
| **Responses API** | `POST {base}/responses` | `Authorization: Bearer {key}` | `reasoning: { effort, summary: "auto" }` |
| **Anthropic** | `POST {base}/messages` | `x-api-key: {key}` + `anthropic-version: 2023-06-01` | `thinking: { type: "enabled", budget_tokens }` |
| **Gemini** | `POST {base}/models/{model}:streamGenerateContent?alt=sse` | `x-goog-api-key: {key}` | `generationConfig.thinkingConfig.includeThoughts: true` |

---

## 2. SSE 流式事件归一化映射

### 2.1 Chat Completion (`/v1/chat/completions`)
- 请求：`stream: true`, `stream_options: { include_usage: true }`
- SSE 帧：`data: {"choices":[{"delta":{"reasoning_content":"...","content":"..."}}]}`
- 结束标志：`data: [DONE]`

### 2.2 Responses API (`/v1/responses`)
- 请求：`input: [...]`, `instructions: systemPrompt`, `stream: true`, `previous_response_id`（可选多轮状态复用）
- SSE 事件类型 (`event:`):
  - `response.reasoning_summary_text.delta` -> `ThoughtDelta`
  - `response.output_text.delta` -> `TextDelta`
  - `response.function_call_arguments.delta` -> `ToolCallDelta`
  - `response.completed` -> `Usage` + `Done`

### 2.3 Anthropic Messages (`/v1/messages`)
- 请求：`system: systemPrompt`, `messages: [...]`, `max_tokens: 4096`, `stream: true`
- SSE 事件类型 (`event:`):
  - `content_block_delta` (`delta.type == "thinking_delta"`) -> `ThoughtDelta(delta.thinking)`
  - `content_block_delta` (`delta.type == "text_delta"`) -> `TextDelta(delta.text)`
  - `content_block_delta` (`delta.type == "input_json_delta"`) -> `ToolCallDelta`
  - `message_delta` -> `Usage`

### 2.4 Gemini GenerateContent (`:streamGenerateContent?alt=sse`)
- 请求：`systemInstruction: { parts: [{ text }] }`, `contents: [...]`, `generationConfig`
- SSE 帧：`data: {"candidates":[{"content":{"parts":[{"text":"...","thought":true}]}}]}`
  - 当 `part.thought == true` -> `ThoughtDelta(part.text)`
  - 当 `part.text` 且无 `thought` -> `TextDelta(part.text)`
  - 当 `part.functionCall` -> `ToolCall`
  - `usageMetadata` -> `Usage`
