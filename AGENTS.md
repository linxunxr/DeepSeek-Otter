# DeepSeek-Otter 代理指南

面向 AI 代理（与人协作者）的项目约定：构建测试命令、代码结构、注意事项。项目介绍与设计方案见 `README.md` 与 `docs/桌面端设计方案.md`。

## 构建与运行

前置要求：Node.js ≥ 22、pnpm（`corepack enable pnpm`）、Rust stable（Windows 需 MSVC 工具链）、WebView2（Win11 自带）。

```sh
# 安装依赖（npm 官方源慢/超时时先切镜像：pnpm config set registry https://registry.npmmirror.com）
pnpm install

# 下载自管运行时（node.exe sidecar + npm.tgz，~92 MB，产物被 .gitignore 忽略）
# 打包与无 PATH 实测前必须先跑；国内默认走 npmmirror 镜像，可用 MIRROR/NPM_REGISTRY 环境变量覆盖
node scripts/fetch-runtime.mjs [--force]

# 开发模式：Vite dev server + Rust 壳热编译
# 运行时解析顺序：exe 同级 node.exe → resources → src-tauri/binaries/ → (debug) 系统 PATH
pnpm tauri dev

# 编译 Rust 壳（不出安装包；build.rs 会把 externalBin 复制到 target/release/）
pnpm tauri build --no-bundle

# 出 NSIS 安装包
pnpm tauri build

# 仅构建壳本地页面（产出 dist/）
pnpm build
```

## 代码结构

```text
index.html / src/         壳本地页面（加载页/诊断页，纯 TS，无框架）
src-tauri/src/lib.rs      壳入口：窗口、托盘、单实例、关窗驻留、IPC 命令
src-tauri/src/backend.rs  dsh 后端生命周期状态机（spawn/就绪解析/停止/自动重启）
src-tauri/src/state.rs    全局状态（目前只有 Backend）
src-tauri/tauri.conf.json Tauri 配置（窗口、打包目标、capability 绑定）
src-tauri/capabilities/   IPC 权限声明（仅授予壳本地页面，不授予回环 dsh 页面）
```

## 关键约定

- **薄壳原则**：壳只做进程管理、窗口、托盘、更新；产品本体是 dsh Web UI。不自研聊天 UI，不 fork dsh，不改上游一行代码。
- **自管运行时（无 PATH 依赖）**：node.exe 以 Tauri externalBin 打包，npm CLI 以 resources 打包；dsh 本体首次启动在线安装（`upstream.json` pin 精确版本，默认 registry 走 npmmirror，可用 `OTTER_NPM_REGISTRY` 覆盖）。安装布局：`<appData>/dsh-runtime/{npm/, install/node_modules/@deepseek-ai/dsh}`。
- **后端"不用即停"**：关窗 = 隐藏窗口 + 停止后端（见 lib.rs 的 `CloseRequested` 处理）；托盘点开 = 重启后端 + 恢复窗口。改生命周期逻辑时同步更新 `docs/桌面端设计方案.md`。
- **dsh URL 必须从 stdout 解析**：dsh web 打印 `dsh web: http://127.0.0.1:<port>/?token=<…>`，token 是访问凭据（不带则 401）。壳不自拼 URL。
- **启动命令**：统一 `<node> <dsh>/lib/bin.js web --no-open --port 0`（OS 分配端口），不经 npx/cmd，摆脱 PATH 依赖。
- **Windows 进程终止**：用 `taskkill /PID <pid> /T /F` 杀整个 npx→node 进程树（实测一次调用即可释放监听端口），不单独 kill 顶层进程。
- **IPC 权限**：capability 只绑定壳本地页面。回环加载的 dsh Web UI 不授予任何 Tauri IPC 权限。
- **提交规范**：`类型(范围): 中文概要——补充说明`，范围常用 `shell`（壳）/ `backend`（后端管理）/ `ui`（壳页面）/ `docs`。

## 已知坑

- dsh 处于 developer preview，破坏性变更频发；每次发版前跑冒烟清单（启动 → 新建会话 → 发消息 → 关窗再开）。
- `pnpm tauri build` 需要有效图标（`src-tauri/icons/icon.ico`，当前是占位图标）。
- Git Bash 管道下 `taskkill` 输出中文乱码是编码问题，不影响功能。
