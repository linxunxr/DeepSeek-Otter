# DeepSeek-Otter

把 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）打包成开箱即用的 Windows / macOS 桌面应用：下载即用，无需安装 Node.js、无需命令行，自动管理 dsh 后端与版本。

## 现状

自管运行时已落地（2026-09-10）：应用自带 node.exe 与 npm，首次启动在线安装 pin 版本的 dsh（`upstream.json`），目标机器**无需安装 Node.js、不依赖系统 PATH**。端到端实测：清空数据目录启动 → 自动安装 dsh（~25 s）→ 后端拉起 → 窗口加载官方 Web UI。完整设计见 [docs/桌面端设计方案.md](docs/桌面端设计方案.md)。设计目标按优先级排序：**资源占用低 > 启动快 > 功能全**。核心结论：

- **Tauri 2 薄壳 + 回环加载官方 Web UI**：壳进程 ~15 MB（Rust），用系统 WebView 渲染 dsh Web UI；不自研 UI、不 fork dsh；
- **后端"不用即停"**：关窗驻留托盘时停止 dsh 后端进程，驻留内存 ~15 MB；
- **松耦合版本策略**：node/npm 打进安装包，dsh 首次启动在线安装 pin 的精确版本；
- 当前仅验证 Windows x64（WebView2 与官方 UI 同为 Chromium 内核），macOS 验证后跟进。

开发环境要求 Node.js ≥ 22、pnpm、Rust（MSVC）。构建与运行命令见 [AGENTS.md](AGENTS.md)。

## 文档索引

| 文档                               | 内容                     |
| ---------------------------------- | ------------------------ |
| [docs/桌面端设计方案.md](docs/桌面端设计方案.md) | 产品定位、方案选型、架构与发布设计 |
