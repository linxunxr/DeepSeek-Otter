// 控制中心"数据目录"设置：dsh 的用户数据默认在 ~/.dsh（C 盘），
// 可改为任意盘符路径——壳 spawn dsh 时注入 DSH_HOME 环境变量生效，
// plugins/skills 等壳内模块统一经 dsh_home() 解析同一位置。
//
// 迁移策略：目标目录为空时整树复制源数据（要求先停后端保证一致）；
// 目标已有 dsh 数据（存在 profiles/ 或 settings.yaml）视为"切换到已有数据"，
// 不覆盖直接切换——两个方向都安全。

use serde_json::Value;
use tauri::Manager;

fn settings_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("otter-settings.json"))
}

fn read_settings(app: &tauri::AppHandle) -> Value {
    settings_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// 壳内统一的 dsh 数据目录解析：配置优先，缺省 ~/.dsh（与 dsh 自身默认一致）。
pub fn dsh_home(app: &tauri::AppHandle) -> std::path::PathBuf {
    if let Some(p) = read_settings(app).get("dshHome").and_then(|v| v.as_str()) {
        if !p.is_empty() {
            return p.into();
        }
    }
    default_dsh_home()
}

pub fn default_dsh_home() -> std::path::PathBuf {
    if let Ok(h) = std::env::var("DSH_HOME") {
        if !h.is_empty() {
            return h.into();
        }
    }
    std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join(".dsh")
}

#[tauri::command]
pub fn get_otter_settings(app: tauri::AppHandle) -> String {
    let mut v = read_settings(&app);
    if v.get("dshHome").is_none() {
        v["dshHome"] = Value::String(default_dsh_home().to_string_lossy().into_owned());
    }
    v.to_string()
}

fn write_settings(app: &tauri::AppHandle, v: &Value) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| format!("appData 不可用：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 appData 失败：{e}"))?;
    std::fs::write(
        dir.join("otter-settings.json"),
        serde_json::to_string_pretty(v).unwrap_or_default(),
    )
    .map_err(|e| format!("写入设置失败：{e}"))
}

/// 迁移/切换数据目录：停后端 → 空目标则整树复制 → 保存设置 → 重启后端。
#[tauri::command]
pub fn migrate_dsh_home(app: tauri::AppHandle, new_home: String) -> Result<String, String> {
    let target = std::path::PathBuf::from(new_home.trim());
    if target.as_os_str().is_empty() {
        return Err("目标路径为空".into());
    }
    if !target.is_absolute() {
        return Err("目标须为绝对路径".into());
    }
    let source = default_dsh_home();
    if target == source {
        return Err("目标与当前默认目录相同".into());
    }
    // 语法误伤防御：路径里不应有引号等（写 JSON/环境变量都安全，仅防极端）。
    if new_home.contains('"') {
        return Err("路径含非法字符".into());
    }
    std::fs::create_dir_all(&target).map_err(|e| format!("创建目标目录失败：{e}"))?;

    let target_has_data = target.join("profiles").is_dir() || target.join("settings.yaml").is_file();
    let mut report = String::new();
    if !target_has_data {
        if !source.is_dir() {
            report.push_str("默认目录不存在，直接切换（首启将在新位置初始化）");
        } else {
            // 停后端保证复制一致性；失败不阻断（可能本就没在跑）。
            app.state::<crate::OtterState>().backend.stop();
            copy_dir_recursive(&source, &target).map_err(|e| format!("复制数据失败：{e}"))?;
            report.push_str(&format!(
                "已复制默认目录数据到 {}；",
                target.display()
            ));
        }
    } else {
        app.state::<crate::OtterState>().backend.stop();
        report.push_str("目标已有 dsh 数据，直接切换；");
    }

    let mut v = read_settings(&app);
    v["dshHome"] = Value::String(target.to_string_lossy().into_owned());
    write_settings(&app, &v)?;
    // 立即用新目录重启后端。
    app.state::<crate::OtterState>().backend.start(&app);
    report.push_str("已切换并重启后端");
    Ok(report)
}

fn copy_dir_recursive(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let from = e.path();
        let to = dest.join(e.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_home_reads_env() {
        // 只验证拼接形态；不设环境变量时为 USERPROFILE/.dsh。
        let home = std::path::PathBuf::from(
            std::env::var("USERPROFILE").unwrap_or_default(),
        )
        .join(".dsh");
        assert_eq!(default_dsh_home(), home);
    }
}
