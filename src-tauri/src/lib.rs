// DeepSeek Otter 壳入口：窗口/托盘/单实例/生命周期编排。
// 薄壳原则：壳只做进程管理与窗口，dsh Web UI 是产品本体。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

mod backend;
mod logging;
mod models;
mod plugins;
mod settings;
mod skills;
mod state;

use state::OtterState;

#[tauri::command]
fn get_backend_status(app: tauri::AppHandle) -> backend::BackendStatus {
    app.state::<OtterState>().backend.status()
}

#[tauri::command]
fn restart_backend(app: tauri::AppHandle) {
    app.state::<OtterState>().backend.restart(&app);
}

/// 壳页面（加载页/诊断页）打开控制中心：入口按钮用。
#[tauri::command]
fn show_control_center(app: tauri::AppHandle) {
    open_control_center(&app);
}

/// 更新安装前停 dsh 后端：释放安装目录 node.exe 的文件锁
/// （NSIS 写入被锁文件报 "Error opening file for writing"，v0.1.5 实证）。
#[tauri::command]
fn stop_backend(app: tauri::AppHandle) {
    app.state::<OtterState>().backend.stop();
}

/// 导出诊断包：appData/diagnostics/otter-diag-<时间戳>.txt，
/// 含壳版本、运行时版本、后端状态与近期日志。返回写入路径。
#[tauri::command]
fn export_diagnostics(app: tauri::AppHandle) -> Result<String, String> {    use std::fmt::Write as _;
    let state = app.state::<OtterState>();
    let status = state.backend.status();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut report = String::new();
    let _ = writeln!(report, "DeepSeek Otter 诊断包");
    let _ = writeln!(report, "导出时间（UTC epoch）：{now}");
    let _ = writeln!(report, "壳版本：{}", app.package_info().version);
    let _ = writeln!(report, "平台：{}", std::env::consts::OS);
    let _ = writeln!(report, "\n== upstream pin ==");
    let _ = writeln!(
        report,
        "{}",
        std::fs::read_to_string(upstream_json_path(&app)).unwrap_or_else(|_| "(upstream.json 不可读)".into())
    );
    let _ = writeln!(report, "\n== 后端状态 ==");
    let _ = writeln!(report, "state: {:?}", status.state);
    let _ = writeln!(report, "url: {:?}", status.url);
    let _ = writeln!(report, "message: {:?}", status.message);
    let _ = writeln!(report, "\n== 近期日志（内存环形缓冲）==");
    for line in &status.recent_log {
        let _ = writeln!(report, "{line}");
    }
    let _ = writeln!(report, "\n== 落盘日志（当前文件）==");
    if let Ok(text) = std::fs::read_to_string(state.log.current_path()) {
        // 只取尾部，避免超大文件。
        let tail: String = text.chars().rev().take(8000).collect::<Vec<_>>().into_iter().rev().collect();
        let _ = writeln!(report, "{tail}");
    } else {
        let _ = writeln!(report, "(无落盘日志)");
    }

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("appData 不可用：{e}"))?
        .join("diagnostics");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败：{e}"))?;
    let path = dir.join(format!("otter-diag-{now}.txt"));
    std::fs::write(&path, report).map_err(|e| format!("写入失败：{e}"))?;
    state
        .log
        .log(&format!("诊断包已导出：{}", path.display()));
    Ok(path.to_string_lossy().into_owned())
}

/// upstream.json 的可读位置（诊断导出用，与 backend 的候选顺序一致但只取首个存在项）。
fn upstream_json_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    use std::path::PathBuf;
    let mut candidates = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join("upstream.json"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("..").join("upstream.json"));
        candidates.push(cwd.join("upstream.json"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("upstream.json"));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("upstream.json"))
}

/// 后端就绪后由 backend.rs 调用：把主窗口导航到带 token 的回环 URL。
pub fn navigate_to_backend(app: &tauri::AppHandle, url: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.navigate(url.parse().expect("dsh URL 合法"));
    }
}

/// 显示主窗口并确保后端在跑（托盘点开/第二实例唤起共用）。
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let state = app.state::<OtterState>();
        let status = state.backend.status();
        match status.state {
            backend::BackendState::Running => {
                if let Some(url) = status.url {
                    // 已在运行：直接导航（窗口可能还停在壳页面）。
                    let _ = window.navigate(url.parse().expect("dsh URL 合法"));
                }
            }
            backend::BackendState::Stopped | backend::BackendState::Failed => {
                // 驻留后被停掉/失败：回壳页面并重启后端。
                if let Some(shell_url) = state.shell_url() {
                    let _ = window.navigate(shell_url);
                }
                state.backend.start(app);
            }
            backend::BackendState::Installing | backend::BackendState::Starting => {}
        }
    }
}

/// 启动时记一条壳版本日志（诊断时确认用户实际运行的构建）。
fn state_log_boot(app: &tauri::AppHandle) {
    let state = app.state::<OtterState>();
    state.log.log(&format!(
        "=== Otter 启动：v{}（{}）===",
        app.package_info().version,
        std::env::consts::OS
    ));
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 第二实例：唤起已有窗口。
            show_main_window(app);
        }))
        .plugin(
            // 全局快捷键呼出控制中心：不依赖任何窗口焦点（dsh 前台也能呼出）。
            // 快捷键在 setup 里动态注册（Builder 声明式注册在插件 initialize 阶段
            // 失败会 panic 整个应用）；被其他应用占用时仅记日志降级，
            // 托盘/按钮入口不受影响。
            tauri_plugin_global_shortcut::Builder::new().build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            get_backend_status,
            restart_backend,
            stop_backend,
            show_control_center,
            export_diagnostics,
            models::get_model_config,
            models::set_model_config,
            plugins::list_plugins,
            plugins::install_plugin,
            plugins::uninstall_plugin,
            skills::list_skills,
            skills::list_source_skills,
            skills::import_skills,
            skills::delete_skill,
            skills::import_agents_md,
            settings::get_otter_settings,
            settings::migrate_dsh_home
        ])
        .setup(|app| {
            // 全局状态在 setup 里构造：需要 AppHandle 解析 appData（日志目录）。
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("appData 目录可用");
            app.manage(OtterState::new(data_dir));

            setup_tray(app)?;
            let window = app.get_webview_window("main").expect("主窗口在配置中声明");

            // 记录壳页面初始 URL（dev 为 devUrl，打包为 tauri:// 或 http://tauri.localhost），
            // 后续"回壳页面"导航复用它，避免硬编码 scheme 在不同模式下失效。
            if let Ok(url) = window.url() {
                app.state::<OtterState>().set_shell_url(url);
            }

            // 关窗不退出：隐藏窗口并停止后端（不用即停，见设计文档）。
            let app_handle = app.handle().clone();
            let window_for_close = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window_for_close.hide();
                    app_handle.state::<OtterState>().backend.stop();
                }
            });

            // 启动即拉起后端；前端壳页面同步展示状态。
            let handle = app.handle().clone();
            state_log_boot(&handle);
            handle.state::<OtterState>().backend.start(&handle);

            // 注册全局快捷键（占用冲突时降级：仅日志，不影响托盘/按钮入口）。
            {
                use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
                if let Err(e) =
                    app.global_shortcut().on_shortcut("ctrl+alt+o", |app, _shortcut, event| {
                        if event.state == ShortcutState::Pressed {
                            open_control_center(app);
                        }
                    })
                {
                    app.state::<OtterState>().log.log(&format!(
                        "全局快捷键 Ctrl+Alt+O 注册失败（可能被其他应用占用）：{e}"
                    ));
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri 应用启动失败");
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示 DeepSeek Otter", true, None::<&str>)?;
    let control = MenuItem::with_id(app, "control", "控制中心", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &control, &quit])?;

    let mut builder = TrayIconBuilder::with_id("otter-tray")
        .icon(app.default_window_icon().cloned().unwrap())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "control" => {
                // 控制中心：更新/版本/诊断等壳功能的常驻前台窗口。
                open_control_center(app);
            }
            "quit" => {
                app.state::<OtterState>().backend.stop();
                app.exit(0);
            }
            _ => {}
        });

    builder = builder.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            show_main_window(tray.app_handle());
        }
    });

    builder.build(app)?;

    // 驻留期更新轮询：关窗驻留可能多天不重启，启动时的壳页面检查覆盖不到，
    // 定时拉一次清单补盲区（发现新版弹控制中心 + 改写菜单项文字，仍不自动下载）。
    spawn_update_poll(app.handle().clone(), control);
    Ok(())
}

/// 打开控制中心窗口：已存在则前置，否则新建（WebviewUrl::App 按路径加载，
/// dev 解析到 devUrl、prod 解析到 tauri 虚拟域，勿手拼 External URL——
/// prod 下 tauri.localhost 的手拼绝对地址加载白屏，v0.1.3 实证）。
fn open_control_center(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("control") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "control",
        tauri::WebviewUrl::App("control.html".into()),
    )
    .title("Otter 控制中心")
    .inner_size(780.0, 560.0)
    .min_inner_size(640.0, 480.0)
    .center()
    .build();
}

/// 驻留期更新轮询间隔：24 小时。首轮等满一个间隔（启动检查已由壳页面做过）。
const UPDATE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// 后台轮询新版本：发现新版把托盘"控制中心"菜单项改为"发现新版本 vX.Y.Z…"并
/// 弹出控制中心（前台直达更新面板），安装仍由用户确认。
/// 仅提示不下载——不自动消耗流量；检查失败静默等下一轮。
fn spawn_update_poll(app: tauri::AppHandle, item: MenuItem<tauri::Wry>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(UPDATE_POLL_INTERVAL);
        use tauri_plugin_updater::UpdaterExt;
        let Ok(updater) = app.updater() else { continue };
        match tauri::async_runtime::block_on(updater.check()) {
            Ok(Some(update)) => {
                let _ = item.set_text(format!("发现新版本 v{}…", update.version));
                open_control_center(&app);
            }
            Ok(None) => {}
            Err(_) => {}
        }
    });
}
