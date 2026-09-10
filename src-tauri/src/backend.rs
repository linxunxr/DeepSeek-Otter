// dsh 后端生命周期状态机：运行时自检、首次在线安装、spawn、等待就绪、停止、崩溃重启。
// 设计原则见 docs/桌面端设计方案.md「后端生命周期策略」：
// 不用即停——关窗驻留托盘时停止后端，点开时重启恢复。
//
// 自管运行时（摆脱系统 PATH 依赖）：
// - node.exe 以 Tauri externalBin 打包（resourcesPath/node.exe 或 dev 时源目录）；
// - npm CLI 包以 resources 打包（resourcesPath/runtime/npm.tgz），安装时解包到数据目录；
// - dsh 本体首次启动在线安装（upstream.json pin 精确版本）到数据目录 dsh-runtime/；
// - 之后直接用 node 跑 dsh 的 lib/bin.js，不经 npx/cmd。
//
// 就绪信号与访问 URL 均从 stdout 解析（实测 dsh web 启动后打印
// `dsh web: http://127.0.0.1:<port>/?token=<...>`；token 是访问凭据，
// 带它请求返回 303/200，不带则 401）。壳不自拼 URL，以解析结果为准。

use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::Manager;

/// 就绪判定前缀：dsh web 就绪后向 stdout 打印 `dsh web: http://…/?token=…`。
const READY_PREFIX: &str = "dsh web:";

/// 等待后端就绪的上限；首次含在线安装 dsh 依赖，故给足余量。
const READY_TIMEOUT: Duration = Duration::from_secs(300);

/// 意外退出后的自动重启次数上限。
const MAX_AUTO_RESTARTS: u32 = 3;

/// 后端日志环形缓冲行数。
const LOG_LINES: usize = 200;

/// dsh npm 包名（安装与入口解析共用）。
const DSH_PACKAGE: &str = "@deepseek-ai/dsh";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendState {
    Stopped,
    Installing,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendStatus {
    pub state: BackendState,
    pub url: Option<String>,
    pub message: Option<String>,
    pub recent_log: Vec<String>,
}

struct BackendInner {
    state: BackendState,
    url: Option<String>,
    message: Option<String>,
    log: std::collections::VecDeque<String>,
    child: Option<Child>,
    installer: Option<Child>,
    restarts: u32,
    /// 已就绪过的 dsh 入口路径（安装产物），跨 start/stop 复用避免重复探测。
    dsh_entry: Option<PathBuf>,
}

impl BackendInner {
    fn status(&self) -> BackendStatus {
        BackendStatus {
            state: self.state,
            url: self.url.clone(),
            message: self.message.clone(),
            recent_log: self.log.iter().cloned().collect(),
        }
    }
}

pub struct Backend {
    inner: Mutex<BackendInner>,
    /// 每次 start/stop 递增；后台监控线程据此判断自己是否已过期（被显式停止/重启）。
    generation: AtomicU32,
}

impl Backend {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BackendInner {
                state: BackendState::Stopped,
                url: None,
                message: None,
                log: Default::default(),
                child: None,
                installer: None,
                restarts: 0,
                dsh_entry: None,
            }),
            generation: AtomicU32::new(0),
        }
    }

    pub fn status(&self) -> BackendStatus {
        self.inner.lock().unwrap().status()
    }

    /// 启动后端：必要时先安装，再 spawn。若正在安装/启动/运行则直接返回。
    pub fn start(&self, app: &tauri::AppHandle) {
        {
            let mut inner = self.inner.lock().unwrap();
            if matches!(
                inner.state,
                BackendState::Installing | BackendState::Starting | BackendState::Running
            ) {
                return;
            }
            inner.state = BackendState::Starting;
            inner.url = None;
            inner.message = None;
            inner.restarts = 0;
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        emit_status(app);

        let node = resolve_node(app);
        let dsh_entry = {
            let inner = self.inner.lock().unwrap();
            inner
                .dsh_entry
                .clone()
                .or_else(|| resolve_dsh_entry(app, installed_dsh_dir(app)))
        };

        // 版本对齐（强绑定的运行时保证）：已装 dsh 的指纹必须与安装包内 store 的
        // lock 指纹一致；不一致（壳升级携带了新依赖树 / 残留旧安装）→ 视为未安装，
        // 走离线重装。指纹一致（仅壳升级且依赖树未变）→ 秒开不重装。
        let dsh_entry = dsh_entry.filter(|_| {
            let aligned = installed_matches_store(app);
            if !aligned {
                app.state::<crate::OtterState>()
                    .log
                    .log("[install] 检测到依赖树指纹变化，重新安装 dsh");
            }
            aligned
        });

        match (node, dsh_entry) {
            (Some(node), Some(entry)) => {
                self.inner.lock().unwrap().dsh_entry = Some(entry.clone());
                self.spawn_backend(app, generation, node, entry);
            }
            (Some(node), None) => {
                // 尚未安装 dsh 或指纹不匹配：进入离线安装流程，完成后自动继续 start。
                {
                    let mut inner = self.inner.lock().unwrap();
                    inner.state = BackendState::Installing;
                    inner.message = Some("正在安装 dsh 运行库…".into());
                }
                emit_status(app);
                self.install_dsh(app, generation, node);
            }
            (None, _) => {
                let mut inner = self.inner.lock().unwrap();
                inner.state = BackendState::Failed;
                inner.message =
                    Some("找不到内置 node 运行时（resourcesPath/node.exe）".into());
                drop(inner);
                emit_status(app);
            }
        }
    }

    /// 离线安装：把安装包内置的 dsh-store（含完整 node_modules 依赖树）拷贝到数据目录。
    /// 壳与 dsh 强绑定（见 upstream.json）：store 里的 dsh 版本必须与 pin 完全一致，
    /// 否则视为构建配置错误直接失败——升级 dsh 必须重发 Otter 版本。
    /// 产物结构：dsh-runtime/install/node_modules/@deepseek-ai/dsh/lib/bin.js。
    fn install_dsh(&self, app: &tauri::AppHandle, generation: u32, _node: PathBuf) {
        let Some(store_dir) = resolve_dsh_store(app) else {
            // 诊断信息：落盘候选路径与 exe 位置，store 找不到时定位布局差异。
            let diag = format!(
                "找不到内置 dsh 运行库。exe={:?} cwd={:?} resource_dir={:?}",
                std::env::current_exe(),
                std::env::current_dir(),
                app.path().resource_dir()
            );
            app.state::<crate::OtterState>().log.log(&diag);
            let mut inner = self.inner.lock().unwrap();
            inner.state = BackendState::Failed;
            inner.message =
                Some("找不到内置 dsh 运行库（resources/runtime/dsh-store）".into());
            drop(inner);
            emit_status(app);
            return;
        };
        let Some(pinned) = read_pinned_dsh_version(app) else {
            let mut inner = self.inner.lock().unwrap();
            inner.state = BackendState::Failed;
            inner.message = Some("upstream.json 缺失：无法确定绑定的 dsh 版本".into());
            drop(inner);
            emit_status(app);
            return;
        };

        let app_handle = app.clone();
        thread::spawn(move || {
            let backend = &app_handle.state::<crate::OtterState>().backend;
            let progress = |msg: &str| {
                app_handle
                    .state::<crate::OtterState>()
                    .log
                    .log(&format!("[install] {msg}"));
                let mut inner = backend.inner.lock().unwrap();
                if inner.log.len() >= LOG_LINES {
                    inner.log.pop_front();
                }
                inner.log.push_back(msg.to_string());
                let status = inner.status();
                drop(inner);
                let _ = tauri::Emitter::emit(&app_handle, "backend-status", &status);
            };

            // 1. 强绑定校验：store 的 lock 里 dsh 版本必须等于 pin。
            let store_lock = store_dir.join("package-lock.json");
            let Ok(lock_text) = std::fs::read_to_string(&store_lock) else {
                backend.fail_install(
                    &app_handle,
                    format!("无法读取内置依赖锁：{}", store_lock.display()),
                );
                return;
            };
            let store_version =
                parse_dsh_version_from_lock(&lock_text).unwrap_or_default();
            if store_version != pinned {
                backend.fail_install(
                    &app_handle,
                    format!(
                        "版本绑定不一致：安装包内置 dsh@{store_version}，upstream.json pin 为 {pinned}。
请重新构建安装包（升级 dsh 必须重发 Otter 版本）。"
                    ),
                );
                return;
            }
            progress(&format!("版本绑定一致：dsh@{pinned}"));

            // 2. 拷贝整树到数据目录（先写 staging 再原子改名，中断不留半成品）。
            let runtime_dir = installed_dsh_dir(&app_handle);
            let install_dir = runtime_dir.join("install");
            let staging_dir = runtime_dir.join("install-staging");
            let _ = std::fs::remove_dir_all(&staging_dir);
            progress("正在安装内置 dsh 运行库（离线）…");
            if let Err(e) = copy_dir_recursive(&store_dir.join("node_modules"), &staging_dir.join("node_modules")) {
                backend.fail_install(&app_handle, format!("拷贝依赖树失败：{e}"));
                let _ = std::fs::remove_dir_all(&staging_dir);
                return;
            }
            // package.json/lock 也带上，npm 生态的插件安装未来可复用。
            let _ = std::fs::copy(store_dir.join("package.json"), staging_dir.join("package.json"));
            let _ = std::fs::copy(&store_lock, staging_dir.join("package-lock.json"));
            // 指纹标记：下次启动比对，依赖树未变（仅壳升级）则跳过重装。
            if let Some(fp) = store_lock_fingerprint(&store_dir) {
                let _ = std::fs::write(staging_dir.join(STORE_LOCK_MARKER), &fp);
            }

            let _ = std::fs::remove_dir_all(&install_dir);
            if let Err(e) = std::fs::rename(&staging_dir, &install_dir) {
                backend.fail_install(&app_handle, format!("激活安装目录失败：{e}"));
                let _ = std::fs::remove_dir_all(&staging_dir);
                return;
            }

            // 3. 验证入口后自动启动。
            let entry = install_dir
                .join("node_modules")
                .join(DSH_PACKAGE)
                .join("lib")
                .join("bin.js");
            if !entry.exists() {
                backend.fail_install(
                    &app_handle,
                    format!("安装后找不到 dsh 入口：{}", entry.display()),
                );
                return;
            }
            progress("安装完成，正在启动后端…");
            {
                let mut inner = backend.inner.lock().unwrap();
                inner.dsh_entry = None;
                inner.state = BackendState::Stopped;
                inner.installer = None;
            }
            if backend.is_current(generation) {
                backend.start(&app_handle);
            }
        });
    }

    fn fail_install(&self, app: &tauri::AppHandle, msg: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.state = BackendState::Failed;
        inner.message = Some(msg.clone());
        if inner.log.len() >= LOG_LINES {
            inner.log.pop_front();
        }
        inner.log.push_back(msg);
        inner.installer = None;
        let status = inner.status();
        drop(inner);
        let _ = tauri::Emitter::emit(app, "backend-status", &status);
    }

    /// spawn dsh web 并挂监控线程。
    fn spawn_backend(
        &self,
        app: &tauri::AppHandle,
        generation: u32,
        node: PathBuf,
        entry: PathBuf,
    ) {
        let mut cmd = Command::new(&node);
        cmd.arg(&entry)
            .args(["web", "--no-open", "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        set_no_window(&mut cmd);
        match cmd.spawn() {
            Ok(mut child) => {
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                self.inner.lock().unwrap().child = Some(child);
                spawn_stdout_monitor(app.clone(), generation, stdout);
                spawn_stderr_tail(app.clone(), generation, stderr);
                spawn_watchdog(app.clone(), generation);
            }
            Err(e) => {
                let mut inner = self.inner.lock().unwrap();
                inner.state = BackendState::Failed;
                inner.message = Some(format!("无法启动 dsh 后端进程：{e}"));
                drop(inner);
                emit_status(app);
            }
        }
    }

    /// 停止后端：终止进程树 → 置 Stopped。供关窗驻留与退出使用。
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.inner.lock().unwrap();
        if let Some(mut installer) = inner.installer.take() {
            kill_pid_tree(installer.id());
            let _ = installer.wait();
        }
        if let Some(mut child) = inner.child.take() {
            kill_pid_tree(child.id());
            let _ = child.wait();
        }
        inner.state = BackendState::Stopped;
        inner.url = None;
        inner.message = None;
    }

    /// 用户显式重试（诊断页按钮）。
    pub fn restart(&self, app: &tauri::AppHandle) {
        self.stop();
        self.start(app);
    }

    fn push_log(&self, app: &tauri::AppHandle, line: String) {
        // 内存环形缓冲（前端展示）+ 落盘（诊断导出）双写。
        app.state::<crate::OtterState>().log.log(&line);
        let mut inner = self.inner.lock().unwrap();
        if inner.log.len() >= LOG_LINES {
            inner.log.pop_front();
        }
        inner.log.push_back(line);
    }

    fn is_current(&self, generation: u32) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }
}

/// 把当前状态广播给壳页面（backend-status 事件）。
fn emit_status(app: &tauri::AppHandle) {
    let status = app.state::<crate::OtterState>().backend.status();
    let _ = tauri::Emitter::emit(app, "backend-status", &status);
}

/// 监控 stdout：解析就绪行（含 token 的 URL）、EOF 即进程退出。
fn spawn_stdout_monitor(
    app: tauri::AppHandle,
    generation: u32,
    stdout: Option<std::process::ChildStdout>,
) {
    thread::spawn(move || {
        let Some(out) = stdout else { return };
        let backend = &app.state::<crate::OtterState>().backend;
        let mut reader = BufReader::new(out);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    on_monitor_exit(&app, generation, "dsh 后端进程意外退出");
                    return;
                }
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Some(url) = parse_ready_url(&trimmed) {
                        backend.push_log(&app, format!("{READY_PREFIX} {url}"));
                        let mut inner = backend.inner.lock().unwrap();
                        if backend.is_current(generation) && inner.state == BackendState::Starting {
                            inner.state = BackendState::Running;
                            inner.url = Some(url.clone());
                            inner.message = None;
                            drop(inner);
                            emit_status(&app);
                            crate::navigate_to_backend(&app, &url);
                        }
                        return;
                    }
                    backend.push_log(&app, trimmed);
                    emit_status(&app);
                }
                Err(_) => {
                    on_monitor_exit(&app, generation, "读取 dsh 后端输出失败");
                    return;
                }
            }
        }
    });
}

/// 跟随 stderr：错误信息进日志，便于诊断页展示。
fn spawn_stderr_tail(
    app: tauri::AppHandle,
    generation: u32,
    stderr: Option<std::process::ChildStderr>,
) {
    thread::spawn(move || {
        let Some(err) = stderr else { return };
        let backend = &app.state::<crate::OtterState>().backend;
        let mut reader = BufReader::new(err);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() || !backend.is_current(generation) {
                        continue;
                    }
                    backend.push_log(&app, trimmed);
                    emit_status(&app);
                }
            }
        }
    });
}

/// 就绪超时看门狗：仅负责终止卡死的子进程，状态迁移交由 stdout EOF 路径统一处理，
/// 避免双路径并发改状态。
fn spawn_watchdog(app: tauri::AppHandle, generation: u32) {
    thread::spawn(move || {
        thread::sleep(READY_TIMEOUT);
        let state = app.state::<crate::OtterState>();
        let backend = &state.backend;
        if !backend.is_current(generation) {
            return;
        }
        let inner = backend.inner.lock().unwrap();
        if inner.state != BackendState::Starting {
            return;
        }
        if let Some(child) = inner.child.as_ref() {
            let pid = child.id();
            drop(inner);
            kill_pid_tree(pid);
        }
    });
}

/// stdout EOF/读错的后续处理：标记失败并按上限自动重启（指数退避 2s/4s/8s）。
fn on_monitor_exit(app: &tauri::AppHandle, generation: u32, reason: &str) {
    let backend = &app.state::<crate::OtterState>().backend;
    if !backend.is_current(generation) {
        return; // 已被显式停止/重启，本次监控结果作废。
    }
    let restarts = {
        let mut inner = backend.inner.lock().unwrap();
        if inner.state == BackendState::Running {
            return;
        }
        inner.state = BackendState::Failed;
        if inner.message.is_none() {
            inner.message = Some(reason.to_string());
        }
        if inner.restarts >= MAX_AUTO_RESTARTS {
            inner.message = Some(format!(
                "自动重启已达上限（{MAX_AUTO_RESTARTS} 次）：{}",
                inner.message.take().unwrap_or_default()
            ));
            drop(inner);
            emit_status(app);
            return;
        }
        inner.restarts += 1;
        inner.restarts
    };
    emit_status(app);

    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(1 << restarts));
        let state = app.state::<crate::OtterState>();
        let backend = &state.backend;
        if backend.is_current(generation) && backend.status().state == BackendState::Failed {
            backend.start(&app);
        }
    });
}

// ---------------------------------------------------------------------------
// 自管运行时路径解析
// ---------------------------------------------------------------------------

/// 数据目录下的 dsh 运行时根：<appData>/dsh-runtime。
fn installed_dsh_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("app data dir")
        .join("dsh-runtime")
}

/// 解析内置 node：打包形态在 exe 同级目录（Tauri externalBin 部署位置），
/// dev/--no-bundle 形态在 src-tauri/binaries/（带 target triple 后缀）。
fn resolve_node(app: &tauri::AppHandle) -> Option<PathBuf> {
    let exe_name = if cfg!(windows) { "node.exe" } else { "node" };
    let triple_name = if cfg!(windows) {
        "node-x86_64-pc-windows-msvc.exe"
    } else {
        "node-x86_64-unknown-linux-gnu"
    };
    // 1. exe 同级（NSIS 安装形态：externalBin 部署在安装根目录）。
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let node = dir.join(exe_name);
            if node.exists() {
                return Some(node);
            }
        }
    }
    // 2. resources 目录（resource_dir 形态兜底）。
    if let Ok(dir) = app.path().resource_dir() {
        let node = dir.join(exe_name);
        if node.exists() {
            return Some(node);
        }
        let sidecar = dir.join(triple_name);
        if sidecar.exists() {
            return Some(sidecar);
        }
    }
    // 3. cwd/binaries（dev 从 src-tauri 目录直接跑 target/release 产物时）。
    if let Ok(cwd) = std::env::current_dir() {
        let sidecar = cwd.join("binaries").join(triple_name);
        if sidecar.exists() {
            return Some(sidecar);
        }
    }
    // 4. debug 兜底：系统 PATH 的 node（未跑 fetch-runtime 的开发机）。
    if cfg!(debug_assertions) {
        return which_node_from_path();
    }
    None
}

/// dev 兜底：从 PATH 找 node（debug 构建且未跑 fetch-runtime 时）。
fn which_node_from_path() -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    for dir in path_var.split(';') {
        let candidate = Path::new(dir).join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// 解析内置 dsh 离线 store。三种形态（按序解析）：
/// 1. exe 同级 `runtime/dsh-store`：NSIS 安装与 --no-bundle 目录形态
///    （build.rs 把 resources 复制到 target/release/，与安装布局一致）；
/// 2. resource_dir 下 `runtime/dsh-store`：resource_dir 形态兜底；
/// 3. cwd/resources/runtime/dsh-store：dev 从 src-tauri 目录跑时。
fn resolve_dsh_store(app: &tauri::AppHandle) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = {
        let mut out = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                out.push(dir.join("runtime").join("dsh-store"));
            }
        }
        if let Ok(dir) = app.path().resource_dir() {
            out.push(dir.join("runtime").join("dsh-store"));
        }
        if let Ok(cwd) = std::env::current_dir() {
            out.push(cwd.join("resources").join("runtime").join("dsh-store"));
        }
        out
    };
    candidates
        .into_iter()
        .find(|p| p.join("package-lock.json").exists())
}

/// 从 package-lock.json 文本提取 node_modules/@deepseek-ai/dsh 的精确版本
/// （强绑定校验用）。独立纯函数便于单测。
pub(crate) fn parse_dsh_version_from_lock(lock_text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(lock_text).ok()?;
    v.get("packages")?
        .get("node_modules/@deepseek-ai/dsh")?
        .get("version")?
        .as_str()
        .map(|s| s.to_string())
}

/// 递归拷贝目录（离线安装 store → appData）。
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else if ty.is_symlink() {
            // npm 依赖树里的 bin 链接：按目标内容拷贝，避免跨目录符号链接失效。
            if let Some(parent) = entry.path().parent() {
                if let Ok(target) = std::fs::read_link(entry.path()) {
                    let resolved = parent.join(&target);
                    if resolved.is_dir() {
                        copy_dir_recursive(&resolved, &to)?;
                    } else {
                        let _ = std::fs::copy(&resolved, &to);
                    }
                }
            }
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// 就绪行解析：`dsh web: http://127.0.0.1:<port>/?token=<…>` → 提取 URL 部分。
/// 非 dsh 输出行（npm 下载进度、警告等）返回 None。token 是访问凭据必须原样保留。
/// 行首空白要容忍：子进程 stdout 经管道转发时可能混入缩进（实测 Windows）。
pub(crate) fn parse_ready_url(line: &str) -> Option<String> {
    let url = line.trim_start().strip_prefix(READY_PREFIX)?.trim();
    if url.starts_with("http") {
        Some(url.to_string())
    } else {
        None
    }
}

/// 从 upstream.json 文本解析 pin 的 dsh 版本；文本非法或缺字段返回 None。
/// 独立于 Tauri AppHandle，便于单元测试与 CI 校验。
pub(crate) fn parse_pinned_dsh_version(json_text: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json_text)
        .ok()?
        .get("dshVersion")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 从候选路径列表读 upstream.json 里 pin 的 dsh 版本。
fn read_pinned_dsh_version(app: &tauri::AppHandle) -> Option<String> {
    for path in pinned_version_candidates(app) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(v) = parse_pinned_dsh_version(&text) {
                return Some(v);
            }
        }
    }
    None
}

/// upstream.json 的候选路径：resources（打包形态）、仓库根（dev 形态）、exe 附近（目录形态兜底）。
fn pinned_version_candidates(app: &tauri::AppHandle) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        out.push(dir.join("upstream.json"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        // dev：从 src-tauri cwd 向上到仓库根。
        out.push(cwd.join("..").join("upstream.json"));
        out.push(cwd.join("upstream.json"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // --no-bundle 目录形态：资源未收进 resources，exe 同级找。
            out.push(dir.join("upstream.json"));
            out.push(dir.join("..").join("upstream.json"));
        }
    }
    out
}

/// 解析已安装 dsh 的入口 bin.js（安装布局见 install_dsh）。
fn resolve_dsh_entry(_app: &tauri::AppHandle, runtime_dir: PathBuf) -> Option<PathBuf> {
    let entry = runtime_dir
        .join("install")
        .join("node_modules")
        .join(DSH_PACKAGE)
        .join("lib")
        .join("bin.js");
    entry.exists().then_some(entry)
}

/// 指纹标记文件名（写在 install/ 根，内容为 store lock 的 SHA-256 hex）。
const STORE_LOCK_MARKER: &str = "STORE_LOCK_SHA";

/// 计算 store 依赖树指纹：package-lock.json 内容的 SHA-256。
/// lock 完整锁定整棵依赖树，其指纹即"依赖树是否变化"的判据。
fn store_lock_fingerprint(store_dir: &Path) -> Option<String> {
    let bytes = std::fs::read(store_dir.join("package-lock.json")).ok()?;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(format!("{:x}", hasher.finalize()))
}

/// 版本对齐检查：appData 已装 dsh 的指纹 == 安装包 store 的指纹。
/// 指纹文件缺失（更早版本安装的）也视为不一致，触发一次重装补上标记。
fn installed_matches_store(app: &tauri::AppHandle) -> bool {
    let Some(store_dir) = resolve_dsh_store(app) else {
        return false; // 连 store 都没有，install 流程会兜底报错。
    };
    let Some(store_fp) = store_lock_fingerprint(&store_dir) else {
        return false;
    };
    match std::fs::read_to_string(installed_dsh_dir(app).join("install").join(STORE_LOCK_MARKER)) {
        Ok(installed_fp) => installed_fp.trim() == store_fp,
        Err(_) => false,
    }
}

/// Windows 上 GUI 进程启动控制台程序（node/tar/taskkill）时，系统会为新进程
/// 自动创建一个控制台窗口；CREATE_NO_WINDOW 抑制它。所有子进程必须经过这里。
#[cfg(windows)]
pub(crate) fn set_no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(unix)]
pub(crate) fn set_no_window(_cmd: &mut Command) {}

/// Windows 上无 SIGTERM，用 taskkill /T /F 一次终止整个进程树
/// （实测可同时结束父链并释放监听端口），无需再等宽限超时。
#[cfg(windows)]
fn kill_pid_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status();
}

#[cfg(unix)]
fn kill_pid_tree(pid: u32) {
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 就绪行解析：标准格式（实测 dsh 0.1.2-rc.1 输出）。
    #[test]
    fn parse_ready_url_standard() {
        let line = "dsh web: http://127.0.0.1:1902/?token=piYJGZHDTZr6jsJ_8UOMRHkVwDyNu8MjZ";
        assert_eq!(
            parse_ready_url(line).as_deref(),
            Some("http://127.0.0.1:1902/?token=piYJGZHDTZr6jsJ_8UOMRHkVwDyNu8MjZ")
        );
    }

    /// 带 \r\n 与多余空白的行也要能解析（Windows 管道输出）。
    #[test]
    fn parse_ready_url_whitespace() {
        assert_eq!(
            parse_ready_url("  dsh web:   http://127.0.0.1:8080/?token=x  \r\n").as_deref(),
            Some("http://127.0.0.1:8080/?token=x")
        );
    }

    /// 非就绪行（npm 进度、警告、普通日志）不得误判为就绪。
    #[test]
    fn parse_ready_url_ignores_noise() {
        for line in [
            "",
            "added 520 packages in 25s",
            "npm warn deprecated foo@1.0.0",
            "dsh web:",
            "dsh web: not-a-url",
            "some other prefix http://127.0.0.1:1/?token=x",
        ] {
            assert!(parse_ready_url(line).is_none(), "误判就绪：{line:?}");
        }
    }

    /// URL 必须保留 token 查询串（它是访问凭据，丢失即 401）。
    #[test]
    fn parse_ready_url_keeps_token() {
        let url = parse_ready_url("dsh web: http://127.0.0.1:39021/?token=abc_DEF-123").unwrap();
        assert!(url.contains("token=abc_DEF-123"), "token 丢失：{url}");
        assert!(url.starts_with("http://127.0.0.1:"), "非回环地址：{url}");
    }

    /// upstream.json 解析：合法 pin。
    #[test]
    fn parse_pinned_version_ok() {
        let json = r#"{"nodeVersion":"24.21.0","dshVersion":"0.1.2-rc.1"}"#;
        assert_eq!(
            parse_pinned_dsh_version(json).as_deref(),
            Some("0.1.2-rc.1")
        );
    }

    /// upstream.json 解析：缺字段 / 空串 / 非法 JSON / 类型错误都返回 None（退化为 latest 前的判定依据）。
    #[test]
    fn parse_pinned_version_rejects_invalid() {
        for json in [
            r#"{"nodeVersion":"24.21.0"}"#,
            r#"{"dshVersion":""}"#,
            "not json at all",
            r#"{"dshVersion":123}"#,
        ] {
            assert!(parse_pinned_dsh_version(json).is_none(), "应拒绝：{json}");
        }
    }

    /// 强绑定校验：从 store 的 package-lock.json 提取 dsh 精确版本。
    #[test]
    fn parse_lock_extracts_dsh_version() {
        let lock = r#"{
          "name": "otter-dsh-store",
          "packages": {
            "": {"name": "otter-dsh-store"},
            "node_modules/@deepseek-ai/dsh": {"version": "0.1.2-rc.1"},
            "node_modules/other": {"version": "9.9.9"}
          }
        }"#;
        assert_eq!(
            parse_dsh_version_from_lock(lock).as_deref(),
            Some("0.1.2-rc.1")
        );
    }

    /// lock 解析：缺 dsh 条目/非法 JSON 返回 None（触发安装失败路径）。
    #[test]
    fn parse_lock_rejects_missing() {
        for lock in [
            r#"{"packages": {}}"#,
            r#"{"packages": {"node_modules/other": {"version": "1.0.0"}}}"#,
            "not json",
        ] {
            assert!(parse_dsh_version_from_lock(lock).is_none(), "应拒绝：{lock}");
        }
    }

    /// 指纹：同内容同指纹、异内容异指纹（版本对齐的判据基础）。
    #[test]
    fn store_fingerprint_stable_and_sensitive() {
        let dir = std::env::temp_dir().join(format!("otter-fp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lock_a = dir.join("package-lock.json");
        std::fs::write(&lock_a, r#"{"packages":{"node_modules/@deepseek-ai/dsh":{"version":"0.1.2-rc.1"}}}"#).unwrap();
        let fp1 = store_lock_fingerprint(&dir).expect("应能计算指纹");
        let fp2 = store_lock_fingerprint(&dir).expect("应能计算指纹");
        assert_eq!(fp1, fp2, "同内容指纹必须稳定");
        assert_eq!(fp1.len(), 64, "SHA-256 hex 长度：{fp1}");
        // 内容变化 → 指纹变化。
        std::fs::write(&lock_a, r#"{"packages":{"node_modules/@deepseek-ai/dsh":{"version":"0.2.0"}}}"#).unwrap();
        let fp3 = store_lock_fingerprint(&dir).expect("应能计算指纹");
        assert_ne!(fp1, fp3, "内容变化指纹必须变化");
        // lock 文件缺失 → None（触发重装路径）。
        std::fs::remove_file(&lock_a).unwrap();
        assert!(store_lock_fingerprint(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 安装布局约定：install 目录下的 dsh 入口路径组装（跨平台路径分段）。
    #[test]
    fn install_layout_entry_path() {
        let dir = PathBuf::from("C:/appdata/dsh-runtime");
        let entry = dir
            .join("install")
            .join("node_modules")
            .join(DSH_PACKAGE)
            .join("lib")
            .join("bin.js");
        let s = entry.to_string_lossy().replace('\\', "/");
        assert!(s.ends_with("node_modules/@deepseek-ai/dsh/lib/bin.js"), "入口路径异常：{s}");
    }
}
