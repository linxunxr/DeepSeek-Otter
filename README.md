# DeepSeek-Otter

把 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）打包成开箱即用的 Windows / macOS 桌面应用：下载即用，无需安装 Node.js、无需命令行，自动管理 dsh 后端与版本。

## 现状

项目处于设计阶段，方案见 [docs/桌面端设计方案.md](docs/桌面端设计方案.md)。设计目标按优先级排序：**资源占用低 > 启动快 > 功能全**。核心结论：

- **Tauri 2 薄壳 + 回环加载官方 Web UI**：壳进程 ~15 MB（Rust），用系统 WebView 渲染 dsh Web UI；不自研 UI、不 fork dsh；
- **后端"不用即停"**：关窗驻留托盘时停止 dsh 后端进程，驻留内存 ~15 MB；
- **松耦合版本策略**：安装包不内置 dsh，首次启动在线安装 pin 的精确版本；
- v1 只发 Windows x64（WebView2 与官方 UI 同为 Chromium 内核），macOS 验证后跟进；自动更新走 GitHub Releases。

## 文档索引

| 文档                               | 内容                     |
| ---------------------------------- | ------------------------ |
| [docs/桌面端设计方案.md](docs/桌面端设计方案.md) | 产品定位、方案选型、架构与发布设计 |
