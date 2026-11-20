# 端到端冒烟测试

用本地模拟模型服务驱动 `lna-devtools serve`，经 WebSocket 走完主流程：导入来源 → 对话 → Studio 生成 → 删除来源。不需要真实 API Key，也不访问外网。

## 前置条件

- 已完成 `cargo build -p lna-devtools`
- Python 3.10+、Node 22+（使用内置 `WebSocket`）

## 运行

```powershell
pwsh tests/e2e/run-smoke.ps1        # 主流程 22 项检查
pwsh tests/e2e/run-smoke.ps1 -Pdf   # 额外检查 PDF 导入
```

## 文件

| 文件 | 作用 |
| :--- | :--- |
| `mock_llm.py` | OpenAI Chat Completions 兼容的流式模拟服务，按系统提示词返回对话文本或测验、思维导图、摘要 JSON；请求记录到日志 |
| `smoke.mjs` | 主流程检查，输出 `PASS/FAIL` 与汇总 |
| `make_pdf.py`、`pdf.mjs` | 生成最小文本 PDF 并检查导入结果 |

## 基线结果（2026-11-20，v2.2）

主流程 22/22 通过（v2.1 为 20/21；v2.2 新增 B-14 检查，B-22 修复后“failed generation”通过）。预期失败项对应计划中的缺陷：

| 检查 | 当前结果 | 缺陷 |
| :--- | :--- | :--- |
| PDF 导入（`-Pdf`） | 已通过：脚本自动获取 PDFium 到 `target/debug`，来源进入 `READY` 且 `page_count=1`；不再是预期失败项 | B-06（已关闭） |

修复对应缺陷后，这些检查应转为通过，并保持为回归测试。通过项数量不得低于上一版（见计划 1.4 节）。
