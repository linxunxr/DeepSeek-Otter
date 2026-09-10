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

## 测试

```sh
pnpm test              # 类型检查（tsc）+ Rust 单元测试（cargo test）
pnpm smoke             # 冒烟：已有安装形态（要求先 tauri build --no-bundle）
pnpm smoke:fresh       # 冒烟：首装形态（删 appData，含在线安装 dsh，全程约 1 分钟）
```

- **单元测试**：`src-tauri/src/backend.rs` 的 `#[cfg(test)]` 模块，覆盖就绪行解析（`parse_ready_url`）与 upstream pin 解析（`parse_pinned_dsh_version`）等纯函数。新增可测逻辑优先抽纯函数再测。
- **冒烟测试**：`scripts/smoke-test.mjs` 黑盒驱动 release exe，断言五步：应用启动 → dsh 后端拉起 → 回环端口 + 无 token 401（鉴权在位）→ 第二实例不破坏主实例（单实例锁）→ 退出后进程树终止且端口释放。改动生命周期/启动链/打包配置后必须跑。
- **CI**：`.github/workflows/ci.yml`（windows-latest）：检查 → 单测 → fetch-runtime → 构建 → fresh 冒烟。跑中国镜像源，可用 `MIRROR`/`NPM_REGISTRY` 环境变量覆盖。

## 代码结构

```text
index.html / src/         壳本地页面（加载页/诊断页，纯 TS，无框架）
src-tauri/src/lib.rs      壳入口：窗口、托盘、单实例、关窗驻留、IPC 命令、诊断导出
src-tauri/src/backend.rs  dsh 后端生命周期状态机（安装/spawn/就绪解析/停止/自动重启）
src-tauri/src/logging.rs  落盘日志（appData/logs/otter-<日期>.log，按天滚动保留 7 份）
src-tauri/src/state.rs    全局状态（Backend + FileLog + 壳页面 URL）
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
- `pnpm tauri build` 首次会从 GitHub 下载 NSIS（~2.4 MB），网络差会 `unexpected end of file` 失败；手动下载 nsis-3.11.zip 解压到 `%LOCALAPPDATA%\tauri\nsis-3.11\` 再重试。
- `pnpm tauri build` 需要有效图标（`src-tauri/icons/icon.ico`，当前是占位图标）。
- Git Bash 里传 `/S /D=` 给 NSIS 安装器会被 MSYS 路径转换破坏参数，静默安装/卸载用 PowerShell `Start-Process -ArgumentList` 执行。
- Git Bash 管道下 `taskkill` 输出与日志 `cat` 的中文乱码是编码问题，不影响功能（日志文件本身 UTF-8 正常）。
