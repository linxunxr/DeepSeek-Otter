// dsh 后端生命周期状态机：spawn、等待就绪、停止、崩溃重启。
// 设计原则见 docs/桌面端设计方案.md「后端生命周期策略」：
// 不用即停——关窗驻留托盘时停止后端，点开时重启恢复。
//
// 就绪信号与访问 URL 均从 stdout 解析（实测 dsh web 启动后打印
// `dsh web: http://127.0.0.1:<port>/?token=<...>`；token 是访问凭据，
// 带它请求返回 303/200，不带则 401）。壳不自拼 URL，以解析结果为准。

use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::Manager;

/// 就绪判定前缀：dsh web 就绪后向 stdout 打印 `dsh web: http://…/?token=…`。
const READY_PREFIX: &str = "dsh web:";

/// 等待后端就绪的上限；开发模式 npx 首次运行需下载包，故给足余量。
const READY_TIMEOUT: Duration = Duration::from_secs(300);

/// 意外退出后的自动重启次数上限。
const MAX_AUTO_RESTARTS: u32 = 3;

/// 后端日志环形缓冲行数。
const LOG_LINES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendState {
    Stopped,
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
    restarts: u32,
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
                restarts: 0,
            }),
            generation: AtomicU32::new(0),
        }
    }

    pub fn status(&self) -> BackendStatus {
        self.inner.lock().unwrap().status()
    }

    /// 启动后端。若正在启动/已运行则直接返回，避免重复 spawn。
    pub fn start(&self, app: &tauri::AppHandle) {
        {
            let mut inner = self.inner.lock().unwrap();
            if matches!(inner.state, BackendState::Starting | BackendState::Running) {
                return;
            }
            inner.state = BackendState::Starting;
            inner.url = None;
            inner.message = None;
            inner.restarts = 0;
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        emit_status(app);

        match spawn_dsh_web() {
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
        if let Some(mut child) = inner.child.take() {
            kill_tree(&mut child);
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

    fn push_log(&self, line: String) {
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
    use tauri::Emitter;
    let status = app.state::<crate::OtterState>().backend.status();
    let _ = app.emit("backend-status", &status);
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
                    if let Some(url) = trimmed.strip_prefix(READY_PREFIX) {
                        let url = url.trim().to_string();
                        backend.push_log(format!("{READY_PREFIX} {url}"));
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
                    backend.push_log(trimmed);
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

/// 跟随 stderr：npx 下载进度与错误信息进日志，便于诊断页展示。
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
                    backend.push_log(trimmed);
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

/// 组装并 spawn `dsh web --no-open --port 0`。
/// 端口交由 OS 分配（dsh 支持 `--port 0`），真实 URL 从 stdout 解析。
/// stderr 单独管道收集进诊断日志。
fn spawn_dsh_web() -> std::io::Result<Child> {
    let (program, args) = resolve_dsh_command();
    Command::new(program)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// 解析 dsh 启动命令。
/// 打包环境：环境变量指定 sidecar node.exe + dsh 入口（首次启动安装产物）。
/// 开发环境：npx @deepseek-ai/dsh（要求 PATH 里有 node/npm）。
/// Windows 上 npx 是 .cmd 脚本，无法被 CreateProcess 直接执行，需经 cmd /C；
/// 且 release 子系统不走 shell PATHEXT 解析，故显式找 npx.cmd。
fn resolve_dsh_command() -> (String, Vec<String>) {
    if let Ok(node_sidecar) = std::env::var("OTTER_NODE_SIDECAR") {
        let dsh_entry = std::env::var("OTTER_DSH_ENTRY").unwrap_or_else(|_| "dsh".into());
        return (
            node_sidecar,
            vec![dsh_entry, "web".into(), "--no-open".into(), "--port".into(), "0".into()],
        );
    }
    let dsh_args = vec![
        "@deepseek-ai/dsh".into(),
        "web".into(),
        "--no-open".into(),
        "--port".into(),
        "0".into(),
    ];
    if cfg!(windows) {
        ("cmd".into(), ["/C".into(), "npx".into(), "-y".into()].into_iter().chain(dsh_args).collect())
    } else {
        ("npx".into(), dsh_args)
    }
}

/// Windows 上无 SIGTERM，用 taskkill /T /F 一次终止 npx→node 整个进程树
/// （实测可同时结束父链并释放监听端口），无需再等宽限超时。
#[cfg(windows)]
fn kill_tree(child: &mut Child) {
    kill_pid_tree(child.id());
    let _ = child.wait();
}

#[cfg(windows)]
fn kill_pid_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status();
}

#[cfg(unix)]
fn kill_tree(child: &mut Child) {
    kill_pid_tree(child.id());
    let _ = child.wait();
}

#[cfg(unix)]
fn kill_pid_tree(pid: u32) {
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status();
}
