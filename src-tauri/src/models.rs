// 控制中心"模型与供应商"的后端：配置源（appData/otter-models.json）与派生产物
// （appData/otter-models.patch.yml，dsh 启动时经 --patch 注入，见 backend.rs）。
//
// 设计：不碰用户手写的 ~/.dsh/profiles/web/cordis.patch.yml——壳管理的配置走独立
// overlay 文件，dsh 在 profile 层之后应用它（README: --patch extra patch-list overlay
// applied after the profile layer）。patch 内容按 dsh-llm-pi-ai 的 providers 字典
// schema 生成（路由名 → api/baseURL/apiKeyEnv/models）+ agent-default-model 默认模型。

use serde_json::Value;
use tauri::Manager;

fn config_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("otter-models.json"))
}

#[tauri::command]
pub fn get_model_config(app: tauri::AppHandle) -> Option<String> {
    let path = config_path(&app)?;
    std::fs::read_to_string(path).ok()
}

/// 保存配置：写 JSON 源文件并派生 YAML patch。返回生成的 patch 路径（日志用）。
#[tauri::command]
pub fn set_model_config(app: tauri::AppHandle, config: String) -> Result<String, String> {
    let value: Value = serde_json::from_str(&config).map_err(|e| format!("配置 JSON 非法：{e}"))?;
    let yaml = render_patch_yaml(&value)?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("appData 不可用：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 appData 失败：{e}"))?;
    std::fs::write(dir.join("otter-models.json"), &config)
        .map_err(|e| format!("写入配置失败：{e}"))?;
    let patch = dir.join("otter-models.patch.yml");
    std::fs::write(&patch, yaml).map_err(|e| format!("写入 patch 失败：{e}"))?;
    Ok(patch.to_string_lossy().into_owned())
}

/// 后端每次 spawn 前调用：以 JSON 源为准重派生 patch，消除两文件漂移
/// （v0.1.10 时曾出现"JSON 在、patch 丢"：UI 无改动则「保存」灰着、「重启」
/// 又只重启不重建，配置永远生效不了）。JSON 缺失 → 清掉孤儿 patch 不注入；
/// 派生失败（手改坏 JSON 等）→ 记日志、删旧 patch 降级启动，后端仍可用。
pub fn ensure_patch(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    let json = dir.join("otter-models.json");
    let patch = dir.join("otter-models.patch.yml");
    let log = |msg: &str| app.state::<crate::OtterState>().log.log(msg);
    match sync_patch_files(&json, &patch) {
        Ok(made) => made.then_some(patch),
        Err(e) => {
            log(&format!("[models] 重派生 patch 失败，本次不带供应商配置启动：{e}"));
            None
        }
    }
}

/// 纯文件逻辑（可测）：JSON 存在则渲染并写 patch 返回 true；JSON 缺失则删除
/// 孤儿 patch 返回 false；渲染失败删除旧 patch 后返回 Err（调用方决定降级策略）。
fn sync_patch_files(json: &std::path::Path, patch: &std::path::Path) -> Result<bool, String> {
    let Some(text) = std::fs::read_to_string(json).ok() else {
        // JSON 没了（从未配置/被清理）：派生物一并清掉，保持"以源为准"。
        let _ = std::fs::remove_file(patch);
        return Ok(false);
    };
    let result = (|| -> Result<bool, String> {
        let value: Value =
            serde_json::from_str(&text).map_err(|e| format!("配置 JSON 非法：{e}"))?;
        let yaml = render_patch_yaml(&value)?;
        if let Some(parent) = patch.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(patch, yaml).map_err(|e| format!("写入 patch 失败：{e}"))?;
        Ok(true)
    })();
    if result.is_err() {
        // 坏配置不注入：删旧 patch，避免盘上残留与 JSON 不一致的派生物。
        let _ = std::fs::remove_file(patch);
    }
    result
}

/// YAML 双引号转义（拼接安全：注入字符全部落在引号字符串内）。
fn yq(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))
}

/// 校验 provider 路由名：YAML mapping 键位与 dsh GenerateOptions.provider 的标识符，
/// 限 ASCII 字母数字-_，拒绝特殊字符。
fn valid_route_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 校验 apiKeyEnv：dsh 的 credential 引用要求环境变量名格式
/// `/^[A-Za-z_][A-Za-z0-9_]*$/`（连字符/数字开头都会让整个 boot 失败——
/// v0.1.9 曾因用户把 key 本体填进该字段而启动即崩）。
fn valid_env_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 按 dsh patch entry 语法渲染（providers 非空才有 llm-pi-ai 条目；
/// 默认模型选了才写 agent-default-model 条目）。
fn render_patch_yaml(v: &Value) -> Result<String, String> {
    let mut out = String::from(
        "# 由 Otter 控制中心（模型与供应商页）生成；手动编辑会在下次保存时被覆盖。\n",
    );
    let providers = v
        .get("providers")
        .and_then(|p| p.as_array())
        .ok_or("缺少 providers 数组")?;
    if !providers.is_empty() {
        out.push_str("- id: llm-pi-ai\n  config:\n    providers:\n");
        for p in providers {
            let route = p
                .get("name")
                .and_then(|s| s.as_str())
                .ok_or("provider 缺少 name")?;
            if !valid_route_name(route) {
                return Err(format!("供应商名 {route:?} 非法（仅限字母数字、-、_）"));
            }
            out.push_str(&format!("      {route}:\n"));
            if let Some(d) = p.get("displayName").and_then(|s| s.as_str()) {
                if !d.is_empty() {
                    out.push_str(&format!("        displayName: {}\n", yq(d)));
                }
            }
            if let Some(api) = p.get("api").and_then(|s| s.as_str()) {
                if !api.is_empty() {
                    out.push_str(&format!("        api: {api}\n"));
                }
            }
            if let Some(base) = p.get("baseURL").and_then(|s| s.as_str()) {
                if !base.is_empty() {
                    out.push_str(&format!("        baseURL: {}\n", yq(base)));
                }
            }
            if let Some(env) = p.get("apiKeyEnv").and_then(|s| s.as_str()) {
                if !env.is_empty() {
                    // 保存时即拦截：坏值（如 key 本体、含连字符）会让 dsh 整个
                    // boot 失败，静默丢字段则配置看似生效实则没用，两者都不能要。
                    if !valid_env_name(env) {
                        return Err(format!(
                            "供应商 {route} 的 apiKeyEnv 要填环境变量名（字母/数字/下划线、\
                             不以数字开头，如 MY_GATEWAY_API_KEY），不能直接填 key 本体"
                        ));
                    }
                    out.push_str(&format!("        apiKeyEnv: {env}\n"));
                }
            }
            if let Some(models) = p.get("models").and_then(|m| m.as_array()) {
                if !models.is_empty() {
                    out.push_str("        models:\n");
                    for m in models {
                        let id = m
                            .get("id")
                            .and_then(|s| s.as_str())
                            .ok_or("模型缺少 id")?;
                        out.push_str(&format!("          - id: {}\n", yq(id)));
                        if let Some(n) = m.get("name").and_then(|s| s.as_str()) {
                            if !n.is_empty() {
                                out.push_str(&format!("            name: {}\n", yq(n)));
                            }
                        }
                        if let Some(cw) = m.get("contextWindow").and_then(|n| n.as_u64()) {
                            out.push_str(&format!("            contextWindow: {cw}\n"));
                        }
                    }
                }
            }
        }
    }
    if let (Some(provider), Some(model)) = (
        v.get("defaultProvider").and_then(|s| s.as_str()),
        v.get("defaultModel").and_then(|s| s.as_str()),
    ) {
        if !provider.is_empty() && !model.is_empty() {
            if !valid_route_name(provider) {
                return Err(format!("默认供应商 {provider:?} 非法"));
            }
            out.push_str(&format!(
                "- id: agent-default-model\n  config:\n    provider: {provider}\n    model: {}\n",
                yq(model)
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_full_patch() {
        let cfg = serde_json::json!({
            "providers": [{
                "name": "my-gw",
                "displayName": "我的网关",
                "api": "openai-completions",
                "baseURL": "https://gw.example/v1",
                "apiKeyEnv": "GW_KEY",
                "models": [
                    { "id": "m1", "name": "模型一", "contextWindow": 128000 }
                ]
            }],
            "defaultProvider": "my-gw",
            "defaultModel": "m1"
        });
        let y = render_patch_yaml(&cfg).unwrap();
        assert!(y.contains("- id: llm-pi-ai"));
        assert!(y.contains("      my-gw:"));
        assert!(y.contains("        baseURL: \"https://gw.example/v1\""));
        assert!(y.contains("          - id: \"m1\""));
        assert!(y.contains("- id: agent-default-model"));
        assert!(y.contains("    provider: my-gw"));
    }

    #[test]
    fn render_empty_and_rejects_bad_name() {
        let empty = serde_json::json!({ "providers": [] });
        assert!(!render_patch_yaml(&empty).unwrap().contains("llm-pi-ai"));
        let bad = serde_json::json!({
            "providers": [{ "name": "a: b", "models": [] }]
        });
        assert!(render_patch_yaml(&bad).is_err());
    }

    #[test]
    fn yaml_quote_escapes() {
        assert_eq!(yq("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    }

    /// apiKeyEnv 按环境变量名格式校验：key 本体（sk-… 含连字符）、数字开头等
    /// 会让 dsh boot 失败的值必须在保存时拦截，空值（可选字段）放行。
    #[test]
    fn rejects_bad_api_key_env() {
        for bad in [
            "sk-bcf076969a072461-1430b9", // key 本体（连字符）
            "1ABC",                       // 数字开头
            "A-B",                        // 连字符
            "MY KEY",                     // 空格
        ] {
            let cfg = serde_json::json!({
                "providers": [{ "name": "gw", "apiKeyEnv": bad }]
            });
            let err = render_patch_yaml(&cfg).unwrap_err();
            assert!(err.contains("环境变量名"), "报错文案：{err}");
        }
        let ok = serde_json::json!({
            "providers": [{ "name": "gw", "apiKeyEnv": "OMNIROUTE_API_KEY" }]
        });
        assert!(render_patch_yaml(&ok).unwrap().contains("apiKeyEnv: OMNIROUTE_API_KEY"));
        // 可选字段：空值直接省略，不产出该行。
        let none = serde_json::json!({ "providers": [{ "name": "gw" }] });
        assert!(!render_patch_yaml(&none).unwrap().contains("apiKeyEnv"));
    }

    /// spawn 前重派生：JSON 在则写 patch；JSON 缺失删孤儿；坏 JSON 报错并清残留。
    #[test]
    fn sync_patch_files_source_of_truth() {
        let dir = std::env::temp_dir().join(format!("otter-patch-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let json = dir.join("otter-models.json");
        let patch = dir.join("otter-models.patch.yml");

        // 1) JSON 在（哪怕 patch 丢失/过期）→ 重派生并写盘。
        std::fs::write(&json, r#"{"providers":[{"name":"gw","apiKeyEnv":"GW_KEY"}]}"#).unwrap();
        assert!(sync_patch_files(&json, &patch).unwrap());
        assert!(patch.exists() && std::fs::read_to_string(&patch).unwrap().contains("gw:"));

        // 2) 坏 JSON（含非法 apiKeyEnv）→ Err 且旧 patch 被清（坏配置不注入）。
        std::fs::write(&json, r#"{"providers":[{"name":"gw","apiKeyEnv":"sk-bad"}]}"#).unwrap();
        assert!(sync_patch_files(&json, &patch).is_err());
        assert!(!patch.exists(), "坏配置后残留 patch 未清理");

        // 3) JSON 缺失 → 孤儿 patch 删除、返回不注入。
        std::fs::write(&patch, "stale").unwrap();
        std::fs::remove_file(&json).unwrap();
        assert!(!sync_patch_files(&json, &patch).unwrap());
        assert!(!patch.exists(), "孤儿 patch 未清理");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
