// DeepSeek Otter 壳入口：窗口/托盘/单实例/生命周期编排。
// 薄壳原则：壳只做进程管理与窗口，dsh Web UI 是产品本体。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

mod backend;
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
            backend::BackendState::Starting => {}
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 第二实例：唤起已有窗口。
            show_main_window(app);
        }))
        .manage(OtterState::new())
        .invoke_handler(tauri::generate_handler![
            get_backend_status,
            restart_backend
        ])
        .setup(|app| {
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
            handle.state::<OtterState>().backend.start(&handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri 应用启动失败");
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示 DeepSeek Otter", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("otter-tray")
        .icon(app.default_window_icon().cloned().unwrap())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
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
    Ok(())
}
