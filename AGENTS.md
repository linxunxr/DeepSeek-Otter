# DeepSeek-Otter 代理指南

面向 AI 代理（与人协作者）的项目约定：构建测试命令、代码结构、注意事项。项目介绍与设计方案见 `README.md` 与 `docs/桌面端设计方案.md`。

## 构建与运行

前置要求：Node.js ≥ 22、pnpm（`corepack enable pnpm`）、Rust stable（Windows 需 MSVC 工具链）、WebView2（Win11 自带）。

```sh
# 安装依赖（npm 官方源慢/超时时先切镜像：pnpm config set registry https://registry.npmmirror.com）
pnpm install

# 生成自管运行时（node.exe sidecar + npm.tgz + dsh@pin 完整离线依赖树，~92 MB 下载 + ~268 MB store）
# 打包前必须先跑；升级 dsh 流程：改 upstream.json 的 dshVersion → 跑本脚本 → 重发 Otter 版本
node scripts/fetch-runtime.mjs [--force]

# 开发模式：Vite dev server + Rust 壳热编译
# 运行时解析顺序：exe 同级 node.exe → resources → src-tauri/binaries/ → (debug) 系统 PATH
pnpm tauri dev

# 编译 Rust 壳（不出安装包；build.rs 会把 externalBin 复制到 target/release/）
pnpm tauri build --no-bundle

# 出 NSIS 安装包（~76 MB，含离线 dsh 依赖树；本地构建需 TAURI_SIGNING_PRIVATE_KEY）
pnpm tauri build

# 仅构建壳本地页面（产出 dist/）
pnpm build
```

## 发版与热更新

**发版一律走 GitHub Actions**（`.github/workflows/release.yml`），本地只做日常开发验证：

1. 升级 dsh：改 `upstream.json` 的 `dshVersion` → `node scripts/fetch-runtime.mjs` → 提交（lock 入库）。
2. 改版本号（`package.json` + `src-tauri/tauri.conf.json` + `src-tauri/Cargo.toml` 三处一致）。
3. 推 tag `v*` → CI 自动：fetch-runtime → 单测 → 签名构建 → 冒烟守门 → 上传 GitHub Release（setup.exe + .sig + latest.json）→ 同步 Gitee（保底 GitHub-URL 清单由 runner 直传 → 云函数中转大附件 + 升级 Gitee-URL 清单）。
4. Gitee 侧异常（附件缺失/清单不对）单独补传：`gh workflow run resync-gitee.yml -f version=<版本号>`——免重跑全量发版，云函数自会从 GitHub Release 下载产物补齐 Gitee 并升级清单。仅清单有问题时用更轻的 `gh workflow run sync-manifest.yml -f version=<版本号>` 直推 GitHub-URL 保底清单（v0.1.1 发版新增）。

**必需 Secrets**：`TAURI_SIGNING_PRIVATE_KEY`（updater 私钥内容，`~/.tauri/deepseek-otter.key`，空密码；**丢失即无法向存量用户推更新，需离线备份**）、`GITEE_TOKEN`（Gitee 私人令牌，projects 权限；缺省时 CI 自动跳过 Gitee，仅 GitHub 单源）、`GITEE_SYNC_URL`（中转云函数 URL）+ `SYNC_SECRET`（触发口令，**必须与云函数同名环境变量的值一致**，两侧不一致 CI 恒 403 且被 continue-on-error 掩盖；建议纯字母数字规避 URL 编码问题）。

**云函数中转**（代码在独立仓库 `github.com/linxunxr/Scf` 的 `gitee-sync/`，香港 Region **Web 函数** `gitee-sync-web`：HTTP 服务监听 9000 + `scf_bootstrap` 启动，函数 URL 免 CAM、应用层 secret 鉴权）：从 GitHub Release **动态解析附件名**后下载 setup.exe/.sig → 建/查 Gitee 发行版 → attach_files 上传（幂等）→ 透传 GitHub 清单换 Gitee-URL 提交。之所以中转：GitHub Actions runner 直传 78MB 到 Gitee 跨洲超时 0 字节。**附件命名链（v0.1.5 下载 404 根因，勿再踩）**：本地 Tauri 产物是空格名（`DeepSeek Otter_…`），**GitHub 上传 asset 时自动把空格替换为点号**（`DeepSeek.Otter_…`），Gitee 附件经云函数上传同为点号名——拼下载 url 一律用 `assetName.replace(/ /g, ".")`（generate-latest-json.mjs 已修）；云函数侧从 release API assets 动态解析（天然点号名，勿硬编码）。**附件全量进内存 Buffer（下载 + base64 上传多份拷贝），SCF 内存须按附件体积留足余量，76MB 附件峰值约 500MB**——512MB 配置下 OOM（ret_code 200402，被 continue-on-error 掩盖；v0.1.2 曾临界侥幸通过，v0.1.3 必炸），2026-09-19 起 2048MB 实测稳定；控制台改内存后注意验证生效（zip 部署等操作可能重置配置）。Gitee raw 的 latest.json 有约 1 分钟 CDN 缓存，PUT 后立即拉到旧值别误判失败。SCF 事件函数形态曾三度翻车（CJS/ESM、热实例环境变量、草稿不落盘），经验全部沉淀在 Scf 仓库 `gitee-sync/README.md`。

更新链路（客户端）：自动更新只到"提示"级——启动时壳页面静默检查（发现新版才亮提示面板）+ 驻留期 Rust 侧每 24h 轮询（发现新版托盘菜单项改为"发现新版本 vX.Y.Z…"，见 lib.rs 的 `spawn_update_poll`）；下载/安装始终由用户经托盘"检查更新…"手动触发：壳页面展示版本/进度 → 确认后 downloadAndInstall（Windows 安装时应用自动退出重装，重启后版本对齐机制自动处理 appData 的 dsh 重装）。双源 endpoint：Gitee raw `latest.json`（国内主）+ GitHub `releases/latest/download/latest.json`（兜底）；注意 **updater 只在拉清单阶段回退，下载 url 失败不回退**（灵鉴 v0.5.1 事故教训），故发布链路必须"先降级后升级"。

## 测试

```sh
pnpm test              # 类型检查（tsc）+ Rust 单元测试（cargo test）
pnpm smoke:build       # 构建冒烟专用 exe（identifier 加 .smoke 后缀，与运行中实例隔离）
pnpm smoke             # 冒烟：已有安装形态（优先用 smoke exe）
pnpm smoke:fresh       # 冒烟：首装形态（删 smoke 独立 appData，含 dsh 离线安装，全程约 1 分钟）
```

- **单元测试**：`src-tauri/src/backend.rs` 的 `#[cfg(test)]` 模块，覆盖就绪行解析（`parse_ready_url`）与 upstream pin 解析（`parse_pinned_dsh_version`）等纯函数。新增可测逻辑优先抽纯函数再测。
- **冒烟测试**：`scripts/smoke-test.mjs` 黑盒驱动 release exe，断言五步：应用启动 → dsh 后端拉起 → 回环端口 + 无 token 401（鉴权在位）→ 第二实例不破坏主实例（单实例锁）→ 退出后进程树终止且端口释放。改动生命周期/启动链/打包配置后必须跑。**与运行中实例隔离**：`pnpm smoke:build` 用 `tauri.smoke.conf.json`（identifier 加 `.smoke`）构建专用 exe，单实例锁与 appData 全独立，测试不动用户正在运行的 Otter——注意 `--config` 构建会覆盖 `target/release/deepseek-otter.exe`，所以产物复制为 `deepseek-otter-smoke.exe` 保存，要出正式产物需重新 `pnpm tauri build --no-bundle`。无 smoke exe 时回退正式 exe 并预检停止已运行实例（兜底路径，会中断进行中的会话）。
- **排障/手动验证与生产隔离（硬约定）**：手动验证与冒烟同规则——用 smoke exe + `.smoke` 独立 appData（可往里塞测试配置，测完清理残留）；**绝不 kill/重启用户正在运行的正式实例**，故障循环中的实例也一样，确需操作先征得同意（2026-09-19 排障时 taskkill 正式实例被纠正）。定位正式版安装位置走注册表 Uninstall 键（HKCU/HKLM `...\Uninstall\*` 查 DisplayName），装机路径不固定。
- **CI**：`.github/workflows/ci.yml`（windows-latest）：检查 → 单测 → fetch-runtime → 构建 → fresh 冒烟。跑中国镜像源，可用 `MIRROR`/`NPM_REGISTRY` 环境变量覆盖。

## 代码结构

```text
index.html / src/         壳本地页面（加载页/诊断页，纯 TS，无框架）
control.html / src/control.ts  控制中心页面（更新/模型与供应商/诊断，同一壳页面体系）
src-tauri/src/lib.rs      壳入口：窗口、托盘、单实例、关窗驻留、IPC 命令、诊断导出
src-tauri/src/backend.rs  dsh 后端生命周期状态机（安装/spawn/就绪解析/停止/自动重启）
src-tauri/src/models.rs   模型与供应商配置（appData JSON 源 → 派生 YAML patch，--patch 注入 dsh；直填 key 经派生环境变量 OTTER_KEY_<路由名> 于 spawn 时注入，key 不落 dsh 配置）
src-tauri/src/plugins.rs  插件市场（读 web profile package.json；装/卸经 dsh plugin 转发 pnpm）
src-tauri/src/skills.rs   Skill 管理与导入（扫 ~/.dsh/skills；Zcode skills 批量导入、AGENTS.md 迁移）
src-tauri/src/sessions_archive.rs  会话归档（借内置 Node 的 node:sqlite 只读 Zcode 会话库，导出 Markdown 到 dshHome/imported-sessions/）
src-tauri/src/memories.rs 长期记忆迁移（Zcode 按项目记忆库 → dshHome/memories/ 正文 + AGENTS.md 标记段常驻索引；dsh 全局指令注入预算 64KiB，故索引常驻正文按需读）
src-tauri/src/logging.rs  落盘日志（appData/logs/otter-<日期>.log，按天滚动保留 7 份）
src-tauri/src/state.rs    全局状态（Backend + FileLog + 壳页面 URL）
src-tauri/tauri.conf.json Tauri 配置（窗口、打包目标、capability 绑定）
src-tauri/capabilities/   IPC 权限声明（仅授予壳本地页面，不授予回环 dsh 页面）
```

## 关键约定

- **薄壳原则**：壳只做进程管理、窗口、托盘、更新；产品本体是 dsh Web UI。不自研聊天 UI，不 fork dsh，不改上游一行代码。
- **壳与 dsh 强绑定（离线打包）**：dsh@pin 的完整依赖树打进安装包 resources（`runtime/dsh-store/`），首装纯离线拷贝到 `<appData>/dsh-runtime/install/`，零网络。运行时校验 store lock 与 upstream.json pin 一致，不一致拒绝启动——**升级 dsh 必须重发 Otter 版本**（改 upstream.json → fetch-runtime → 重新打包）。
- **后端"不用即停"**：关窗 = 隐藏窗口 + 停止后端（见 lib.rs 的 `CloseRequested` 处理）；托盘点开 = 重启后端 + 恢复窗口。改生命周期逻辑时同步更新 `docs/桌面端设计方案.md`。
- **dsh URL 必须从 stdout 解析**：dsh web 打印 `dsh web: http://127.0.0.1:<port>/?token=<…>`，token 是访问凭据（不带则 401）。壳不自拼 URL。
- **启动命令**：统一 `<node> <dsh>/lib/bin.js web --no-open --port 0`（OS 分配端口），不经 npx/cmd，摆脱 PATH 依赖。带供应商配置时为 `web --patch <otter-models.patch.yml> --no-open --port 0`——**`--patch` 必须在 `web` 子命令之后**，dsh 拒绝出现在子命令之前的父级 flag（顺序拼反 = 启动即崩，v0.1.9 事故；验证 patch 相关改动必须真跑 `web` 启动路径，`--dump-config` 根级调用测不出顺序问题）。patch 文件由 `models::ensure_patch` 在每次 spawn 前从 JSON 源重派生（JSON 缺失删孤儿、坏配置降级不带 patch），不要依赖"保存时写过一次"——两文件永不漂移。
- **Windows 进程终止**：用 `taskkill /PID <pid> /T /F` 杀整个 npx→node 进程树（实测一次调用即可释放监听端口），不单独 kill 顶层进程。
- **IPC 权限**：capability 只绑定壳本地页面。回环加载的 dsh Web UI 不授予任何 Tauri IPC 权限。
- **提交规范**：`类型(范围): 中文概要——补充说明`，范围常用 `shell`（壳）/ `backend`（后端管理）/ `ui`（壳页面）/ `docs`。

## 已知坑

- dsh 处于 developer preview，破坏性变更频发；每次发版前跑冒烟清单（启动 → 新建会话 → 发消息 → 关窗再开）。
- `pnpm tauri build` 首次会从 GitHub 下载 NSIS（~2.4 MB），网络差会 `unexpected end of file` 失败；手动下载 nsis-3.11.zip 解压到 `%LOCALAPPDATA%\tauri\nsis-3.11\` 再重试。
- `pnpm tauri build` 需要有效图标（`src-tauri/icons/icon.ico` 多尺寸）。源图 `app-icon.png`（1024×1024）由 `scripts/make-icon.ps1` 生成（PowerShell 5.1 读无 BOM 的 UTF-8 脚本会把中文注释读乱破坏解析，文件须带 BOM）；改图标流程：改脚本或换源图 → `pnpm tauri icon app-icon.png` 重新生成全套。
- Git Bash 里传 `/S /D=` 给 NSIS 安装器会被 MSYS 路径转换破坏参数，静默安装/卸载用 PowerShell `Start-Process -ArgumentList` 执行。
- Git Bash 管道下 `taskkill` 输出与日志 `cat` 的中文乱码是编码问题，不影响功能（日志文件本身 UTF-8 正常）。
- GitHub Actions step 级 `if` 里不能用 `secrets` 上下文（workflow_dispatch 校验直接 422）；统一经 `env:` 传值、脚本内判空。`curl -f` 会丢弃 4xx/5xx 响应体，排障时要拿 body 就别加 `-f`。
- 腾讯云 SCF 控制台的 Cloud Studio 在线编辑器对程序化粘贴不可靠（Ctrl+A/V 会把内容追加而非替换，且工作区草稿与线上已部署代码会不一致）；改代码用"提交方法 → 本地上传 zip 包"最稳。SCF 运行时坑见 Scf 仓库 `gitee-sync/README.md`。
