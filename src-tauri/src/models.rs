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

pub fn patch_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("otter-models.patch.yml"))
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
}
