# DeepSeek-Otter

把 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）打包成开箱即用的 Windows / macOS 桌面应用：下载即用，无需安装 Node.js、无需命令行，自动管理 dsh 后端与版本。

## 现状

项目处于设计阶段，方案见 [docs/桌面端设计方案.md](docs/桌面端设计方案.md)。核心结论：

- **薄壳 Electron + 回环加载官方 Web UI**：不自研 UI、不 fork dsh，官方 Web UI 即产品本体；
- **松耦合版本策略**：安装包不内置 dsh，首次启动在线安装 pin 的精确版本；
- v1 支持 Windows x64 与 macOS Universal，自动更新走 GitHub Releases。

## 文档索引

| 文档                               | 内容                     |
| ---------------------------------- | ------------------------ |
| [docs/桌面端设计方案.md](docs/桌面端设计方案.md) | 产品定位、方案选型、架构与发布设计 |
