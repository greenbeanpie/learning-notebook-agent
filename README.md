# Local Notebook Agent

基于 **Rust Core (`crates/lna-*`)** + **SQLite WAL / FTS5 / `sqlite-vec`** 构建的本地知识笔记本与 AI Agent 应用，支持 **Chat Completions**、**Responses**、**Anthropic Messages**、**Google Gemini** 四大模型协议，内置多家官方与中转 AI 提供商预设，并提供 **Windows (WinUI 3)**、**Android (Jetpack Compose)**、**Linux (GTK4)** 与 **macOS (SwiftUI)** 原生客户端。

---

## 目录结构

| 路径 | 说明 |
| --- | --- |
| `crates/` | 14 个 Rust workspace crates（存储、密钥、任务调度、四协议 LLM、本地/远程嵌入、文档摄取、混合检索、Agent、Studio 产物生成、TTS、API、FFI C ABI / JNI、DevTools） |
| `apps/windows/LocalNotebook/` | .NET 10 NativeAOT + WinUI 3 桌面客户端（三栏工作区、WebView2 + KaTeX 公式渲染、Windows 凭据管理器） |
| `apps/android/LocalNotebook/` | Kotlin + Jetpack Compose + Material 3 安卓客户端（通过 JNI 调用 `liblna_ffi.so`，Android Keystore 加密存储密钥） |
| `apps/linux/local-notebook-gtk/` | Rust + GTK4 Linux 桌面客户端（独立 Cargo manifest） |
| `apps/macos/LocalNotebook/` | Swift 5.9+ / SwiftUI macOS 客户端（macOS 14+，链接 `liblna_ffi`） |
| `migrations/`、`schemas/`、`prompts/` | SQLite 数据库迁移脚本、Studio 产物 JSON Schema、版本化提示词模板 |
| `scripts/` | 打包与辅助脚本（如 `package-windows-client.ps1`、`fetch-pdfium.ps1`、`configure-model.ps1`） |
| `frontend-demo/` | 配合 `lna-devtools` 使用的浏览器联调页 |
| `tests/e2e/` | 端到端冒烟测试套件（`mock_llm.py` + `smoke.mjs` + `extended.mjs` + `run-smoke.ps1` / `run-smoke.sh`） |

---

## 环境准备

### 通用依赖（所有平台）
- **Git**
- **Rust 工具链**：通过 [rustup](https://rustup.rs/) 安装最新稳定版 `rustc` 与 `cargo`

### 按平台附加依赖
- **Windows 客户端**：
  - [.NET 10 SDK](https://dotnet.microsoft.com/)
  - Visual Studio 2022 / Build Tools（勾选 **使用 C++ 的桌面开发**，`.NET NativeAOT` 编译必需）
  - PowerShell 7+ (`pwsh`，用于一键打包脚本 `scripts/package-windows-client.ps1`)
- **Android 客户端**：
  - Android SDK + **Android NDK**（已验证 NDK `30.0.16248370`）
  - **JDK 17+** 与 **Gradle 9.8+**
  - 安装 Rust Android 编译目标：`rustup target add aarch64-linux-android x86_64-linux-android`
- **Linux (GTK4) 客户端**：
  - GTK4 与基础构建依赖（Ubuntu/Debian 示例：`sudo apt install build-essential pkg-config libgtk-4-dev libssl-dev`）
- **macOS 客户端**：
  - Xcode 15+（Swift 5.9+，目标系统 macOS 14+）

---

## 快速编译指南

### 1. 编译与测试 Rust 核心（通用）

```powershell
# 编译全部 workspace crate
cargo build --workspace --all-targets

# 运行全部单元测试与集成测试
cargo test --workspace
```

> **提示**：
> - `ort`（ONNX Runtime）已在 `Cargo.lock` 中锁定为 `=2.0.0-rc.10`。若处于离线环境，可设置环境变量 `ORT_LIB_LOCATION` 指向本地 ONNX Runtime 动态库目录以跳过构建期下载。
> - PDF 解析依赖 PDFium 动态库（不随仓库提交）。运行 PDF 相关测试前可执行：`pwsh scripts/fetch-pdfium.ps1 -Dest target/debug`。

---

### 2. 编译 Windows 客户端

#### 方式 A：一键打包（推荐）
在仓库根目录运行 PowerShell 7+：

```powershell
pwsh scripts/package-windows-client.ps1
```

该脚本会自动完成：`cargo build --release -p lna-ffi -p lna-devtools` → `dotnet publish`（NativeAOT 自包含）→ 复制 `lna_ffi.dll`、`lna-devtools.exe`、`configure-model.ps1` 并下载校验 `pdfium.dll`，最终输出到 **`dist/LocalNotebook-test/`**。

#### 方式 B：分步手动编译
1. 编译 Rust FFI 动态库：
   ```powershell
   cargo build -p lna-ffi --release
   ```
2. 发布 WinUI 3 客户端（**必须**保留 `Platform`、`WindowsPackageType=None` 与 `WindowsAppSDKSelfContained=true` 参数）：
   ```powershell
   dotnet publish apps/windows/LocalNotebook/LocalNotebook.csproj `
     -c Release -r win-x64 `
     -p:Platform=x64 `
     -p:WindowsPackageType=None `
     -p:WindowsAppSDKSelfContained=true `
     -o dist/LocalNotebook-portable
   ```
3. 运行 `dist/LocalNotebook-portable/LocalNotebook.exe`（如需 PDF 支持，可将 `pdfium.dll` 放到同目录）。

---

### 3. 编译 Android 客户端

Android 客户端需要先编译原生动态库 `liblna_ffi.so` 到 `apps/android/LocalNotebook/app/src/main/jniLibs/<abi>/`，再构建 APK。

#### 方式 A：使用 `cargo-ndk`（Linux / macOS / Git Bash）
```bash
cargo install cargo-ndk
export ANDROID_NDK_HOME=/path/to/android-ndk
bash apps/android/LocalNotebook/build-android-rust.sh

cd apps/android/LocalNotebook
gradle :app:assembleDebug
```

#### 方式 B：Windows PowerShell 直连 NDK（以 `x86_64` 模拟器为例）
```powershell
# 1. 配置 NDK 工具链路径（请按本机 NDK 实际路径修改）
$ndkBin = "D:\Android\Sdk\ndk\30.0.16248370\toolchains\llvm\prebuilt\windows-x86_64\bin"
$env:CC_x86_64_linux_android = "$ndkBin\x86_64-linux-android28-clang.cmd"
$env:CXX_x86_64_linux_android = "$ndkBin\x86_64-linux-android28-clang++.cmd"
$env:AR_x86_64_linux_android = "$ndkBin\llvm-ar.exe"
$env:CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER = "$ndkBin\x86_64-linux-android28-clang.cmd"

# 2. 编译 liblna_ffi.so 并放入 jniLibs（真机请改用 aarch64-linux-android 与 jniLibs\arm64-v8a）
cargo build -p lna-ffi --target x86_64-linux-android --release
New-Item -ItemType Directory -Force -Path apps\android\LocalNotebook\app\src\main\jniLibs\x86_64 | Out-Null
Copy-Item target\x86_64-linux-android\release\liblna_ffi.so apps\android\LocalNotebook\app\src\main\jniLibs\x86_64\liblna_ffi.so -Force

# 3. 编译 Debug APK
$env:ANDROID_HOME = "D:\Android\Sdk"
gradle -p apps\android\LocalNotebook :app:assembleDebug --no-daemon
```

编译完成后，APK 位于 `apps/android/LocalNotebook/app/build/outputs/apk/debug/app-debug.apk`。可通过 ADB 安装到已启动的模拟器或真机：

```powershell
adb install -r apps\android\LocalNotebook\app\build\outputs\apk\debug\app-debug.apk
```

---

### 4. 编译 Linux (GTK4) 与 macOS 客户端

- **Linux (GTK4)**：
  ```bash
  cargo build --manifest-path apps/linux/local-notebook-gtk/Cargo.toml --release
  ```
- **macOS (SwiftUI)**：
  ```bash
  # 方式 A：一键打包为独立 .app 应用（输出至 dist/LocalNotebook.app）
  bash scripts/package-macos-app.sh

  # 方式 B：仅分步编译二进制
  cargo build -p lna-ffi --release
  swift build --package-path apps/macos/LocalNotebook -c release
  ```

---

## 首次运行与 AI 模型配置

启动应用后，打开右上角 **设置 → AI 模型**：

1. 在 **AI 提供商预设** 中直接选择内置预设（会自动填充协议、接口地址、推荐模型、上下文长度与请求头）：
   - **官方直连**：`OpenAI 官方`、`Anthropic 官方`、`Google Gemini 官方`、`DeepSeek 官方`、`GLM 智谱官方`、`MiMo 小米官方`
   - **中转服务**：`Opencode Go (中转)`（已预配置 `x-opencode-session` 与 `User-Agent` 额外请求头，可在界面中自由增删改）
2. 填入你的 **API Key**（可选点击“获取模型列表”或手动修改 Model ID）。
3. 点击 **保存**（桌面端支持直接点击 **测试连接** 验证可用性）。

---

## 端到端测试与 DevTools 联调

- **端到端冒烟测试**（需 Python 3.10+ 与 Node 22+）：
  ```powershell
  cargo build --workspace --all-targets
  pwsh tests/e2e/run-smoke.ps1 -Pdf      # Windows
  bash tests/e2e/run-smoke.sh --pdf      # macOS / Linux
  ```
- **DevTools 网页联调**：
  ```powershell
  cargo run -p lna-devtools -- serve
  ```
  复制终端输出的带 `token` 地址，打开 `frontend-demo/index.html` 即可调试 Core 接口。
- **检索评测与性能基线**：
  - 检索评测（Recall@8 / MRR / 拒答率）随 `cargo test --workspace` 自动执行，报告输出至 `target/eval-report.json`。
  - 性能基线：`cargo test -p lna-api --test bench --release -- --ignored --nocapture`（详见 [docs/benchmarks.md](docs/benchmarks.md)）。

---

## 常见问题排查

1. **Windows 自行 `dotnet publish` 后双击闪退（WinRT / COM 激活异常）**：
   请确认发布命令包含了 `-p:Platform=x64 -p:WindowsPackageType=None -p:WindowsAppSDKSelfContained=true`，切勿直接裸跑 `dotnet publish -c Release`。
2. **Android 提示 `Native core (liblna_ffi.so) not bundled yet`**：
   请确认已按设备架构（真机 `arm64-v8a` / 模拟器 `x86_64`）编译 `liblna_ffi.so` 并复制到 `apps/android/LocalNotebook/app/src/main/jniLibs/<abi>/`。
3. **跨平台构建时提示找不到 `clang` / `vswhere`**：
   - Windows NativeAOT 需安装 VS C++ 桌面开发工具链；
   - Android 交叉编译需正确设置 `CC_<target>` 与 `CARGO_TARGET_<TARGET>_LINKER` 指向 NDK `toolchains/llvm/prebuilt/.../bin` 下的对应编译器。
