# 性能基线与公式渲染（G6 / G7）

测量日期：2026-11-20。机器：Windows 11 LTSC 24H2，x64，release 构建。数字是这台开发机上的实测值，用于发现回归，不是对其他机器的承诺。

## 1. G7 性能指标

| 指标 | 目标 | 实测 | 方法 |
| :--- | :--- | :--- | :--- |
| 空闲内存（Windows，无 WebView2） | 40–80 MB | **私有工作集 78 MB**（启动后 70 秒、含一个 1 来源的笔记本）；私有字节 99 MB；总工作集 161 MB（含共享的系统库） | 启动客户端，空闲 70 秒后读性能计数器 `Working Set - Private` |
| 冷启动到窗口出现 | < 500 ms | **中位数 140 ms**，最大 244 ms（8 次） | `Start-Process` 到 `MainWindowTitle` 出现；核心在后台初始化，窗口先显示 |
| 混合检索 P95（关键词部分，10 万分块） | < 300 ms | **123 ms**（P50 95 ms）；1 万分块 P95 5.6 ms | `crates/lna-api/tests/bench.rs::retrieval_latency_on_10k_and_100k_chunks` |
| 首个 `thought_step` | < 100 ms | **P95 1.1 ms**（P50 0.9 ms，模拟模型） | `first_thought_step_latency`，从 `chat.send` 到第一个事件 |
| 500 页 PDF 分块入库 | < 30 秒 | **0.8 秒**，819 个分块 | `pdf_500_pages_ingest_time`，纯文本 PDF，含页眉页脚识别 |
| 10 万分块语义查询 P95 | < 150 ms | **未测** | 需要 10 万个向量；`vec0` 精确检索的耗时留待真实嵌入模型下测量 |

复现：

```powershell
cargo test -p lna-api --test bench --release -- --ignored --nocapture   # 结果写入 target/bench-report.json
```

PDF 基准需要 PDFium（`pwsh scripts/fetch-pdfium.ps1 -Dest target/debug`）和 PyMuPDF（用来生成测试 PDF）。客户端的内存与启动时间用上面的 PowerShell 方法手工测量。

## 2. G6 公式渲染

结论：**可用，已在 NativeAOT 客户端中验证。**

- `WebContentHost`（`apps/windows/LocalNotebook/Services/WebContentHost.cs`）按需创建**一个**共享的 WebView2，用完或空闲 60 秒后释放；消息列表里不会给每条消息建一个 WebView。
- 入口：含公式的回答多一个“公式视图”按钮；报告窗口的工具栏有同名按钮。点击后在对话框里排版。
- KaTeX 0.16.11 的文件随客户端发布（`Assets/katex/`，MIT 许可，含许可证文件），不联网。页面通过虚拟主机名 `katex.local` 加载，导航到其他地址一律取消。
- 安全：页面的 CSP 为 `default-src 'none'`（只允许本地样式、脚本和字体）；传给页面的只有文本，由页面先转义再排版，不是 HTML；`IsWebMessageEnabled=false`，禁用开发者工具、右键菜单和新窗口。
- 运行时要求：需要系统的 WebView2 运行时（Windows 11 自带）。缺失时对话框里显示错误，不影响其他功能。
- 踩过的坑：Windows App SDK 1.6 自带的 WebView2 投影在 NativeAOT 下无法创建（`Cannot create an RCW factory`）。改为显式引用 `Microsoft.Web.WebView2` 1.0.4314.35 后正常，且不再有 IL2104/IL3053 告警。
