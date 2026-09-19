// 控制中心"插件市场"的后端：插件是 npm 包，安装在 web profile 目录
// （~/.dsh/profiles/web），经 `dsh plugin --profile web <pnpm args>` 转发 pnpm。
// 已装列表读 profile 的 package.json；安装/卸载是网络长任务（pnpm install），
// 命令内阻塞等待并回传输出——前端按长任务展示 loading。

use serde::Serialize;
use std::process::Command;

use crate::backend::{resolve_dsh_entry, resolve_node, installed_dsh_dir};

pub(crate) fn dsh_home() -> std::path::PathBuf {
    if let Ok(h) = std::env::var("DSH_HOME") {
        if !h.is_empty() {
            return h.into();
        }
    }
    std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join(".dsh")
}

#[derive(Serialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
}

#[tauri::command]
pub fn list_plugins() -> Vec<PluginInfo> {
    let pkg = dsh_home().join("profiles/web/package.json");
    let Ok(text) = std::fs::read_to_string(pkg) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v.get("dependencies")
        .and_then(|d| d.as_object())
        .map(|deps| {
            deps.iter()
                .map(|(name, ver)| PluginInfo {
                    name: name.clone(),
                    version: ver.as_str().unwrap_or("?").to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 安装/卸载共用：spawn `node bin.js plugin --profile web <args...>` 并等待完成。
fn run_plugin_cmd(app: &tauri::AppHandle, args: &[&str]) -> Result<String, String> {
    let node = resolve_node(app).ok_or("找不到内置 node 运行时")?;
    let entry = resolve_dsh_entry(app, installed_dsh_dir(app)).ok_or("找不到 dsh 入口")?;
    let mut cmd = Command::new(&node);
    cmd.arg(&entry)
        .arg("plugin")
        .arg("--profile")
        .arg("web")
        .args(args);
    crate::backend::set_no_window(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("无法启动 plugin 命令：{e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        return Err(format!("pnpm 退出码 {:?}：{}", out.status.code(), text));
    }
    Ok(text)
}

#[tauri::command]
pub fn install_plugin(app: tauri::AppHandle, package: String) -> Result<String, String> {
    let pkg = package.trim();
    // 只放行 npm 包名/含版本的形式，避免把任意参数交给 pnpm（如 --config.xxx 注入）。
    if pkg.is_empty()
        || !pkg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@/-_.^~ ".contains(c))
    {
        return Err("包名仅限字母数字与 @/-_.^~ 等版本字符".into());
    }
    run_plugin_cmd(&app, &["add", pkg])
}

#[tauri::command]
pub fn uninstall_plugin(app: tauri::AppHandle, package: String) -> Result<String, String> {
    let pkg = package.trim();
    if pkg.is_empty() || pkg.starts_with('-') || pkg.contains(char::is_whitespace) {
        return Err("包名非法".into());
    }
    run_plugin_cmd(&app, &["remove", pkg])
}
