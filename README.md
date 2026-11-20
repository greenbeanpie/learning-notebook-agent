# Local Notebook Agent

基于 Rust Core (`crates/lna-*`) + Windows WinUI 3 (.NET 10 NativeAOT) + SQLite WAL / FTS5 / `sqlite-vec` 构建的本地自定义模型笔记本 Agent，支持 Chat Completions、Responses、Anthropic Messages、Google Gemini 四大模型协议。

## 工程结构
- `crates/`：14 个 Rust workspace crates（存储、密钥、任务、四协议 LLM、嵌入、摄取、检索、Agent、Studio 7 模态、TTS、API、C ABI、DevTools）。
- `apps/windows/LocalNotebook/`：.NET 10 NativeAOT + WinUI 3 三栏桌面客户端（含 `app.manifest`）：对话与引用、来源管理、Studio 全部模态的查看窗口、设置（模型协议、嵌入模型下载）、隐私与引导对话框；公式由共享的 WebView2 + 本地 KaTeX 排版（`WebContentHost`）。
- `migrations/`、`schemas/`、`prompts/`：SQLite 初始化脚本、Studio 7 模态 JSON Schema、版本化提示词模板。
- `frontend-demo/`：独立演示与 DevTools 联调页（通过 `api.js` 在 `?ws=...&token=...` 时连接真实 Core；视频概览与搜索/Research 入口已按范围移除）。
- `tests/e2e/`：端到端冒烟测试套件（`mock_llm.py` + `smoke.mjs` + `extended.mjs` + `run-smoke.ps1`）；`crates/lna-api/tests/` 是进程内集成测试（模型模拟服务器、流水线、Studio、归档、生命周期、检索评测）。

> 范围说明：按 2026-11-20 范围调整，已取消 Studio **视频概览**，并暂缓网页/本地搜索（`research.*` 返回 `CAPABILITY_MISSING`）。

## 构建与验收运行
1. **构建与单测（Rust）**（`ort` 已锁定 `=2.0.0-rc.10` 并附带 `Cargo.lock`；离线环境可通过设置 `ORT_LIB_LOCATION` 指向本地 ONNX Runtime 动态库目录跳过构建期下载）：
   ```bash
   cargo build --workspace --all-targets
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
2. **运行端到端冒烟测试**（需 Python 3.10+ 与 Node 22+；`-Pdf` 会自动获取固定版本的 PDFium 并校验 SHA-256）：
   ```powershell
   pwsh tests/e2e/run-smoke.ps1 -Pdf
   ```
   PDF 导入依赖 PDFium 动态库（不入库）。运行 PDF 相关单测前执行 `pwsh scripts/fetch-pdfium.ps1 -Dest target/debug`；设置 `LNA_REQUIRE_PDFIUM=1` 可让缺库的单测失败而不是跳过。
3. **启动联调页**：
   ```bash
   cargo run -p lna-devtools -- serve
   ```
   使用终端输出的带 `token` URL 打开 `frontend-demo/index.html` 即可联调。
4. **构建可运行的 Windows 客户端**（需 .NET 10 SDK 与 Windows App SDK）。一条命令完成：核心库、NativeAOT 客户端、`lna_ffi.dll`、PDFium、`configure-model.ps1`，输出到 `dist/LocalNotebook-test`：
   ```powershell
   pwsh scripts/package-windows-client.ps1
   ```
   运行 `dist/LocalNotebook-test/LocalNotebook.exe`，在“模型协议配置”里填写协议、地址、模型和 API Key（Key 保存在 Windows 凭据管理器）。

## 附加测试与评测
- **检索评测**（Recall@8 / MRR / 拒答率，对照 `eval/baseline.json`）：随 `cargo test --workspace` 运行，报告写入 `target/eval-report.json`。
- **性能基线**（`#[ignore]`）：`cargo test -p lna-api --test bench --release -- --ignored --nocapture`，结果见 [docs/benchmarks.md](docs/benchmarks.md)。
- **真实嵌入模型**（默认跳过，需联网下载约 490 MB，下载经 SHA-256 校验并支持断点续传）：
  ```powershell
  $env:LNA_REAL_EMBEDDING=1; $env:LNA_MODEL_CACHE="$env:TEMP\lna-model-cache"
  cargo test -p lna-api --test embedding real_multilingual -- --nocapture
  ```
- 冒烟之前请先 `cargo build --workspace --all-targets`，否则可能使用过期的 `lna-devtools`。
