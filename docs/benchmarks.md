# 性能基线与公式渲染技术评估（G6 / G7）

## 1. G6 公式与富内容渲染技术验证结论
- `WebContentHost`（`apps/windows/LocalNotebook/Services/WebContentHost.cs`）采用**单例按需创建 + 闲置 60 秒自动释放**策略，避免每条消息创建一个 WebView2 实例导致空闲内存突破 40–80 MB 目标。
- 安全边界：禁用 `AreDefaultScriptDialogsEnabled` 与 `IsWebMessageEnabled`，拦截所有非 `data:`/`about:` 外部导航，并配合本地 KaTeX 与净化 HTML + 严格 CSP（禁止外链与脚本桥接）。
- 多公式列表优化方向：消息列表中多处公式按需渲染为缓存矢量/图片，仅在用户展开或查看互动报告时挂载唯一 `WebContentHost`。

## 2. G7 性能基线目标与测量记录表
| 指标 | 目标 | 实测命令/方法 |
| :--- | :--- | :--- |
| 空闲内存（Windows） | 40–80 MB | 打开 1 万分块笔记本静置 60s（无 WebView2），取进程私有工作集 |
| 冷启动 | < 500 ms | `LocalNotebook.exe` 启动到首帧可交互（20 次 P95） |
| 混合检索 | P95 < 300 ms | 10 万分块下 `lna-retrieval` 端到端基准 |
| 首个流式事件 | < 100 ms | 发送 `chat.send` 到首个 `chat.thought_step` 耗时 |
| 500 页 PDF 分块 | < 30 s | `lna-ingest` 解析并写入 SQLite 耗时 |
