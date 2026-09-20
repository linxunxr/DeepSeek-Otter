// 控制中心"模型与供应商"的后端：配置源（appData/otter-models.json）→ dsh 官方
// 热更新面（$DSH_HOME/settings.yaml 的 llm-pi-ai/agent-default-model 节 +
// $DSH_HOME/.credentials.yaml 的 refs，均被 dsh chokidar 监听热发布）。
//
// 为什么弃用 --patch（v0.1.4–v0.1.13 的注入方式）：overlay 文件是启动时的
// 内存快照、不在 dsh watch 范围（watch 仅 profile/home 级 cordis.patch.yml），
// 改配置必须重启后端；settings.yaml 的 llm-pi-ai 路由内容按请求解析，保存后
// 下一请求即生效——这正是 dsh Web UI 模型页自己的写路径（dsh-base 出厂注释：
// "is what the web Models page writes"），格式有官方背书。
//
// 合并语义（settings.yaml 可能同时被 dsh 模型页编辑）：
// - llm-pi-ai.providers：JSON 名单内的键替换；名单外一律保留（用户手写路由）；
//   Otter 历史写入、已从名单删除的路由经 state 记账（appData/
//   otter-models.state.json 的 written 名单）精确移除，不误删手写。
// - credentials refs：OTTER_KEY_ 前缀是 Otter 命名空间，非本次派生名集合即删；
//   其余 refs 与 records 原样保留。
// - agent-default-model：JSON 有默认才整节写，无默认保留现状（可能来自模型页）。
// 已知取舍：两编辑器文件级并发无乐观锁，最后写者赢（单用户场景窗口极小）；
// 改写不保用户注释（dsh 自身改写 settings 同样不保）。

use serde_json::Value;
use serde_yaml::{Mapping, Value as Yaml};
use tauri::Manager;

use crate::settings::dsh_home;

fn config_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("otter-models.json"))
}

#[tauri::command]
pub fn get_model_config(app: tauri::AppHandle) -> Option<String> {
    let path = config_path(&app)?;
    std::fs::read_to_string(path).ok()
}

/// 保存配置：写 JSON 源并同步到 dsh 热更新面（settings.yaml + .credentials.yaml），
/// 后端运行中下一请求即生效、未运行则下次启动生效。返回 settings 路径（日志用）。
#[tauri::command]
pub fn set_model_config(app: tauri::AppHandle, config: String) -> Result<String, String> {
    let value: Value = serde_json::from_str(&config).map_err(|e| format!("配置 JSON 非法：{e}"))?;
    // 先整体校验/构造，再落任何文件——不写出半截状态。
    build_provider_maps(&value)?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("appData 不可用：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 appData 失败：{e}"))?;
    std::fs::write(dir.join("otter-models.json"), &config)
        .map_err(|e| format!("写入配置失败：{e}"))?;
    let home = dsh_home(&app);
    sync_settings_files(
        &dir.join("otter-models.json"),
        &home.join("settings.yaml"),
        &home.join(".credentials.yaml"),
        &dir.join("otter-models.state.json"),
        &dir.join("otter-models.patch.yml"),
    )?;
    Ok(home.join("settings.yaml").to_string_lossy().into_owned())
}

/// 后端每次 spawn 前调用：以 JSON 源为准幂等同步（正常情况保存时已同步过，
/// 这里兜底升级路径首启写入与两文件被外部清理后的自愈）。坏配置记日志降级，
/// 不阻断启动（后端仍可用，只是不带 Otter 供应商配置）。
pub fn ensure_settings(app: &tauri::AppHandle) {
    let Some(dir) = app.path().app_data_dir().ok() else { return };
    let home = dsh_home(app);
    let log = |msg: &str| app.state::<crate::OtterState>().log.log(msg);
    if let Err(e) = sync_settings_files(
        &dir.join("otter-models.json"),
        &home.join("settings.yaml"),
        &home.join(".credentials.yaml"),
        &dir.join("otter-models.state.json"),
        &dir.join("otter-models.patch.yml"),
    ) {
        log(&format!("[models] 同步供应商配置到 settings.yaml 失败，本次跳过：{e}"));
    }
}

/// 纯文件逻辑（可测）：JSON 存在则把供应商配置合并进 settings.yaml 与
/// .credentials.yaml、更新记账、清理 v0.1.13 及以前的 patch 残留，返回 true；
/// JSON 缺失（从未配置/被清理）不动 settings/credentials（可能已有用户手写
/// 内容），清理全部派生物（patch + state），返回 false。
fn sync_settings_files(
    json: &std::path::Path,
    settings: &std::path::Path,
    creds: &std::path::Path,
    state: &std::path::Path,
    legacy_patch: &std::path::Path,
) -> Result<bool, String> {
    let Some(text) = std::fs::read_to_string(json).ok() else {
        let _ = std::fs::remove_file(legacy_patch);
        let _ = std::fs::remove_file(state);
        return Ok(false);
    };
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("配置 JSON 非法：{e}"))?;
    let (providers, refs, default) = build_provider_maps(&value)?;
    let routes: Vec<String> = providers
        .keys()
        .filter_map(|k| k.as_str().map(String::from))
        .collect();
    let prev_written = read_written(state);
    merge_settings(settings, &providers, default.as_ref(), &prev_written, &routes)?;
    merge_credentials(creds, &refs)?;
    // v0.1.13 及以前经 --patch 注入的派生物：清理升级残留。
    let _ = std::fs::remove_file(legacy_patch);
    write_written(state, &routes)?;
    Ok(true)
}

/// 校验 provider 路由名：settings mapping 键位与 dsh GenerateOptions.provider
/// 的标识符，限 ASCII 字母数字-_，拒绝特殊字符。
fn valid_route_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 校验 apiKeyEnv：dsh 的 credential 引用名格式 `/^[A-Za-z_][A-Za-z0-9_]*$/`
/// （连字符/数字开头都非法——v0.1.9 曾因用户把 key 本体填进该字段而启动即崩）。
fn valid_env_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 直填 key 的派生 credential ref 名：路由名受限 [A-Za-z0-9_-]，大写并把 `-`
/// 转 `_` 后必满足 ref 文法；OTTER_KEY_ 前缀隔离出 Otter 命名空间（天然避开
/// 用户环境变量）。同名碰撞（`a-b` vs `a_b`）由 build_provider_maps 检测报错。
fn derived_env_name(route: &str) -> String {
    format!("OTTER_KEY_{}", route.to_uppercase().replace('-', "_"))
}

/// 由 JSON 源构造 (llm-pi-ai.providers 映射, credentials refs 映射, 默认模型)。
/// 全部校验集中在此：路由名/env 名格式、直填 key 与 apiKeyEnv 互斥、派生 ref
/// 名跨供应商碰撞、默认供应商须在名单内。
fn build_provider_maps(v: &Value) -> Result<(Mapping, Mapping, Option<(String, String)>), String> {
    let providers = v
        .get("providers")
        .and_then(|p| p.as_array())
        .ok_or("缺少 providers 数组")?;
    let mut prov_map = Mapping::new();
    let mut ref_map = Mapping::new();
    let mut used_env_names: Vec<String> = Vec::new();
    for p in providers {
        let route = p
            .get("name")
            .and_then(|s| s.as_str())
            .ok_or("provider 缺少 name")?;
        if !valid_route_name(route) {
            return Err(format!("供应商名 {route:?} 非法（仅限字母数字、-、_）"));
        }
        let mut m = Mapping::new();
        if let Some(d) = p.get("displayName").and_then(|s| s.as_str()) {
            if !d.is_empty() {
                m.insert(Yaml::String("displayName".into()), Yaml::String(d.into()));
            }
        }
        if let Some(api) = p.get("api").and_then(|s| s.as_str()) {
            if !api.is_empty() {
                m.insert(Yaml::String("api".into()), Yaml::String(api.into()));
            }
        }
        if let Some(base) = p.get("baseURL").and_then(|s| s.as_str()) {
            if !base.is_empty() {
                m.insert(Yaml::String("baseURL".into()), Yaml::String(base.into()));
            }
        }
        // key 三态：直填（写派生 ref 名 + refs 落 key 本体）/ 显式 env 名（校验
        // 格式后原样引用，key 留在用户环境或 .env）/ 都没有（免 key 路由）。
        let inline_key = p.get("apiKey").and_then(|s| s.as_str()).is_some_and(|s| !s.is_empty());
        let env_name = if inline_key {
            if p
                .get("apiKeyEnv")
                .and_then(|s| s.as_str())
                .is_some_and(|s| !s.is_empty())
            {
                return Err(format!("供应商 {route} 的 API Key 与 API Key 环境变量只能填其一"));
            }
            Some(derived_env_name(route))
        } else if let Some(env) = p.get("apiKeyEnv").and_then(|s| s.as_str()) {
            if env.is_empty() {
                None
            } else {
                // 保存时即拦截坏值：静默丢字段会让配置看似生效实则没用。
                if !valid_env_name(env) {
                    return Err(format!(
                        "供应商 {route} 的 apiKeyEnv 要填环境变量名（字母/数字/下划线、\
                         不以数字开头，如 MY_GATEWAY_API_KEY），不能直接填 key 本体；\
                         想直接用 key 请填「API Key」一栏"
                    ));
                }
                Some(env.to_string())
            }
        } else {
            None
        };
        if let Some(env) = env_name {
            if used_env_names.contains(&env) {
                return Err(format!("环境变量名 {env} 被多个供应商占用，请改用环境变量名区分"));
            }
            used_env_names.push(env.clone());
            m.insert(Yaml::String("apiKeyEnv".into()), Yaml::String(env.clone()));
            if inline_key {
                let key = p.get("apiKey").and_then(|s| s.as_str()).unwrap_or_default();
                ref_map.insert(Yaml::String(env), Yaml::String(key.into()));
            }
        }
        if let Some(models) = p.get("models").and_then(|m| m.as_array()) {
            if !models.is_empty() {
                let mut arr = Vec::new();
                for mm in models {
                    let id = mm
                        .get("id")
                        .and_then(|s| s.as_str())
                        .ok_or("模型缺少 id")?;
                    let mut e = Mapping::new();
                    e.insert(Yaml::String("id".into()), Yaml::String(id.into()));
                    if let Some(n) = mm.get("name").and_then(|s| s.as_str()) {
                        if !n.is_empty() {
                            e.insert(Yaml::String("name".into()), Yaml::String(n.into()));
                        }
                    }
                    if let Some(cw) = mm.get("contextWindow").and_then(|n| n.as_u64()) {
                        e.insert(Yaml::String("contextWindow".into()), Yaml::Number(cw.into()));
                    }
                    arr.push(Yaml::Mapping(e));
                }
                m.insert(Yaml::String("models".into()), Yaml::Sequence(arr));
            }
        }
        prov_map.insert(Yaml::String(route.into()), Yaml::Mapping(m));
    }
    // 默认模型：两者齐备才写（空值视为未选）；供应商必须在名单内。
    let default = match (
        v.get("defaultProvider").and_then(|s| s.as_str()),
        v.get("defaultModel").and_then(|s| s.as_str()),
    ) {
        (Some(p), Some(m)) if !p.is_empty() && !m.is_empty() => {
            if !valid_route_name(p) {
                return Err(format!("默认供应商 {p:?} 非法"));
            }
            if !prov_map.contains_key(&Yaml::String(p.into())) {
                return Err(format!("默认供应商 {p} 不在供应商列表中"));
            }
            Some((p.to_string(), m.to_string()))
        }
        _ => None,
    };
    Ok((prov_map, ref_map, default))
}

/// 读 YAML 文档为 mapping：缺失/空白 → 空 mapping；存在但非法 → Err（不静默
/// 覆盖用户文件）；顶层非 mapping → Err。
fn read_yaml_mapping(path: &std::path::Path) -> Result<Mapping, String> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Mapping::new()),
        Err(e) => Err(format!("读取 {} 失败：{e}", path.display())),
        Ok(text) if text.trim().is_empty() => Ok(Mapping::new()),
        Ok(text) => {
            let v: Yaml = serde_yaml::from_str(&text)
                .map_err(|e| format!("{} 不是合法 YAML：{e}", path.display()))?;
            v.as_mapping()
                .cloned()
                .ok_or_else(|| format!("{} 顶层不是映射", path.display()))
        }
    }
}

/// 取子 mapping，不存在则插入空表；已存在但非 mapping → Err（节损坏不吞）。
fn ensure_mapping<'a>(parent: &'a mut Mapping, key: &str) -> Result<&'a mut Mapping, String> {
    let k = Yaml::String(key.into());
    if !parent.contains_key(&k) {
        parent.insert(k.clone(), Yaml::Mapping(Mapping::new()));
    }
    parent
        .get_mut(&k)
        .and_then(|v| v.as_mapping_mut())
        .ok_or_else(|| format!("settings.yaml 的 {key} 节损坏（非映射）"))
}

/// 合并进 settings.yaml：Otter 名单内路由替换、prev_written 中已删除路由移除、
/// 名单外（用户手写）保留；agent-default-model 有默认才整节写。
fn merge_settings(
    path: &std::path::Path,
    otter_providers: &Mapping,
    default: Option<&(String, String)>,
    prev_written: &[String],
    routes: &[String],
) -> Result<(), String> {
    let mut doc = read_yaml_mapping(path)?;
    if !otter_providers.is_empty() || !prev_written.is_empty() {
        let llm = ensure_mapping(&mut doc, "llm-pi-ai")?;
        let providers = ensure_mapping(llm, "providers")?;
        // Otter 历史路由中已从名单删除的：精确移除（state 记账，不碰手写）。
        for r in prev_written {
            if !routes.contains(r) {
                providers.remove(Yaml::String(r.clone()));
            }
        }
        for (k, v) in otter_providers {
            providers.insert(k.clone(), v.clone());
        }
    }
    if let Some((p, m)) = default {
        let mut dm = Mapping::new();
        dm.insert(Yaml::String("provider".into()), Yaml::String(p.clone()));
        dm.insert(Yaml::String("model".into()), Yaml::String(m.clone()));
        doc.insert(Yaml::String("agent-default-model".into()), Yaml::Mapping(dm));
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = serde_yaml::to_string(&Yaml::Mapping(doc))
        .map_err(|e| format!("序列化 settings.yaml 失败：{e}"))?;
    std::fs::write(path, text).map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

/// 合并进 .credentials.yaml：refs 的 OTTER_KEY_ 前缀为 Otter 命名空间（本次
/// 派生名之外的清掉），其余 refs 与 records 原样保留；version 必须为 1。
fn merge_credentials(path: &std::path::Path, otter_refs: &Mapping) -> Result<(), String> {
    let mut doc = read_yaml_mapping(path)?;
    match doc.get(&Yaml::String("version".into())) {
        None => {
            doc.insert(Yaml::String("version".into()), Yaml::Number(1.into()));
        }
        Some(v) if v.as_u64() == Some(1) => {}
        Some(_) => return Err(format!("{} 的 version 非 1，Otter 不识别该格式", path.display())),
    }
    let refs = ensure_mapping(&mut doc, "refs")?;
    let mut merged = Mapping::new();
    for (k, v) in refs.iter() {
        if k.as_str().is_some_and(|s| s.starts_with("OTTER_KEY_")) {
            continue;
        }
        merged.insert(k.clone(), v.clone());
    }
    for (k, v) in otter_refs {
        merged.insert(k.clone(), v.clone());
    }
    doc.insert(Yaml::String("refs".into()), Yaml::Mapping(merged));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = serde_yaml::to_string(&Yaml::Mapping(doc))
        .map_err(|e| format!("序列化 credentials 失败：{e}"))?;
    std::fs::write(path, text).map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

/// 记账：Otter 已写入 settings 的路由名单（删除同步的依据）。
fn read_written(state: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(state)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            v.get("written").and_then(|w| w.as_array()).map(|a| {
                a.iter().filter_map(|x| x.as_str().map(String::from)).collect()
            })
        })
        .unwrap_or_default()
}

fn write_written(state: &std::path::Path, routes: &[String]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&serde_json::json!({ "written": routes }))
        .map_err(|e| format!("序列化记账失败：{e}"))?;
    std::fs::write(state, text).map_err(|e| format!("写入记账失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("otter-settings-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn full_config() -> Value {
        serde_json::json!({
            "providers": [
                {
                    "name": "my-gw",
                    "displayName": "我的网关",
                    "api": "openai-completions",
                    "baseURL": "https://gw.example/v1",
                    "apiKey": "sk-SECRET",
                    "models": [
                        { "id": "m1", "name": "模型一", "contextWindow": 128000 }
                    ]
                },
                { "name": "free-gw", "api": "openai-responses", "baseURL": "https://f.example/v1",
                  "apiKeyEnv": "FREE_KEY", "models": [{ "id": "f1" }] }
            ],
            "defaultProvider": "my-gw",
            "defaultModel": "m1"
        })
    }

    /// 全字段构造：providers/refs/default 齐备；key 本体只进 refs（credentials），
    /// providers 节里只有派生 ref 名。
    #[test]
    fn build_maps_full() {
        let (prov, refs, default) = build_provider_maps(&full_config()).unwrap();
        assert_eq!(prov.len(), 2);
        let my_gw = prov.get(&Yaml::String("my-gw".into())).unwrap();
        let my_gw = my_gw.as_mapping().unwrap();
        assert_eq!(
            my_gw.get(&Yaml::String("apiKeyEnv".into())).unwrap(),
            &Yaml::String("OTTER_KEY_MY_GW".into())
        );
        assert!(serde_yaml::to_string(&Yaml::Mapping(my_gw.clone()))
            .unwrap()
            .contains("baseURL: https://gw.example/v1"));
        assert_eq!(
            refs.get(&Yaml::String("OTTER_KEY_MY_GW".into())),
            Some(&Yaml::String("sk-SECRET".into()))
        );
        assert_eq!(default.unwrap(), ("my-gw".to_string(), "m1".to_string()));
    }

    /// 校验集：互斥/坏 env 名/派生碰撞/坏路由名/默认供应商不在名单。
    #[test]
    fn build_maps_rejects_bad_config() {
        let both = serde_json::json!({
            "providers": [{ "name": "gw", "apiKey": "sk-x", "apiKeyEnv": "GW_KEY" }]
        });
        assert!(build_provider_maps(&both).unwrap_err().contains("只能填其一"));

        let bad_env = serde_json::json!({
            "providers": [{ "name": "gw", "apiKeyEnv": "sk-bad-key" }]
        });
        assert!(build_provider_maps(&bad_env).unwrap_err().contains("环境变量名"));

        let clash = serde_json::json!({
            "providers": [
                { "name": "my-gw", "apiKey": "sk-a" },
                { "name": "my_gw", "apiKey": "sk-b" }
            ]
        });
        assert!(build_provider_maps(&clash).unwrap_err().contains("被多个供应商占用"));

        let bad_route = serde_json::json!({ "providers": [{ "name": "a: b" }] });
        assert!(build_provider_maps(&bad_route).is_err());

        let ghost_default = serde_json::json!({
            "providers": [{ "name": "gw", "models": [{ "id": "m" }] }],
            "defaultProvider": "other",
            "defaultModel": "m"
        });
        assert!(build_provider_maps(&ghost_default).unwrap_err().contains("不在供应商列表"));
    }

    /// 合并语义：用户手写路由与其他节保留、Otter 路由写入；无默认时
    /// agent-default-model 保持现状。
    #[test]
    fn settings_merge_keeps_user_content() {
        let dir = tmpdir("merge");
        let (json, settings, creds, state, legacy) = paths(&dir);
        std::fs::write(&settings, "ui-theme:\n  preference: dark\nllm-pi-ai:\n  providers:\n    manual-route:\n      api: openai-completions\nagent-default-model:\n  provider: manual-route\n  model: x\n").unwrap();

        let mut cfg = full_config();
        cfg["defaultProvider"] = serde_json::json!(null);
        cfg["defaultModel"] = serde_json::json!(null);
        std::fs::write(&json, serde_json::to_string(&cfg).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &settings, &creds, &state, &legacy).unwrap());

        let out = std::fs::read_to_string(&settings).unwrap();
        assert!(out.contains("ui-theme"), "用户设置节被丢：{out}");
        assert!(out.contains("manual-route"), "手写路由被删：{out}");
        assert!(out.contains("my-gw:"), "Otter 路由未写入：{out}");
        assert!(out.contains("provider: manual-route"), "无默认时不应覆盖 agent-default-model：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 增改删全链路：改 baseURL + 删 free-gw 后再同步——被删路由消失（state
    /// 记账）、幸存路由更新、手写路由仍在、credentials 派生 ref 同步清理。
    #[test]
    fn settings_replace_and_remove_via_state() {
        let dir = tmpdir("replace");
        let (json, settings, creds, state, legacy) = paths(&dir);
        std::fs::write(&settings, "llm-pi-ai:\n  providers:\n    manual-route:\n      api: openai-completions\n").unwrap();
        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &settings, &creds, &state, &legacy).unwrap());

        // 第二轮：删 free-gw、改 my-gw 的 baseURL。
        let mut cfg = full_config();
        cfg["providers"].as_array_mut().unwrap().retain(|p| p["name"] != "free-gw");
        cfg["providers"][0]["baseURL"] = serde_json::json!("https://gw2.example/v1");
        std::fs::write(&json, serde_json::to_string(&cfg).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &settings, &creds, &state, &legacy).unwrap());

        let out = std::fs::read_to_string(&settings).unwrap();
        assert!(!out.contains("free-gw"), "已删路由残留：{out}");
        assert!(out.contains("gw2.example"), "路由更新未生效：{out}");
        assert!(out.contains("manual-route"), "手写路由被误删：{out}");
        let creds_out = std::fs::read_to_string(&creds).unwrap();
        assert!(!creds_out.contains("FREE"), "显式 env 名不应写 refs：{creds_out}");
        assert!(creds_out.contains("OTTER_KEY_MY_GW"), "直填 key 的 ref 丢失：{creds_out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// credentials 合并：他人 refs/records/version 原样保留，OTTER_KEY_ 命名
    /// 空间外删内增；version 非 1 拒写。
    #[test]
    fn credentials_otter_namespace_only() {
        let dir = tmpdir("creds");
        let (json, settings, creds, state, legacy) = paths(&dir);
        std::fs::write(&creds, "version: 1\nrefs:\n  USER_X: keep-me\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload:\n      secret: s\n").unwrap();

        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &settings, &creds, &state, &legacy).unwrap());
        let out = std::fs::read_to_string(&creds).unwrap();
        assert!(out.contains("USER_X") && out.contains("keep-me"), "他人 ref 被清：{out}");
        assert!(out.contains("kind: grant"), "records 被清：{out}");
        assert!(out.contains("OTTER_KEY_MY_GW"), "派生 ref 未写入：{out}");
        // key 本体就该落在 refs（credentials 文件是 dsh 官方明文 key 存储位），
        // 但绝不能进 settings.yaml 的 provider 节。
        assert!(!std::fs::read_to_string(&settings).unwrap().contains("sk-SECRET"));

        // 删直填 key 路由 → OTTER_KEY ref 清理、他人 ref 保留。
        let mut cfg = full_config();
        cfg["providers"].as_array_mut().unwrap().retain(|p| p["name"] != "my-gw");
        cfg["defaultProvider"] = serde_json::json!("free-gw");
        cfg["defaultModel"] = serde_json::json!("f1");
        std::fs::write(&json, serde_json::to_string(&cfg).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &settings, &creds, &state, &legacy).unwrap());
        let out = std::fs::read_to_string(&creds).unwrap();
        assert!(!out.contains("OTTER_KEY_MY_GW"), "已删路由的 ref 未清理：{out}");
        assert!(out.contains("USER_X"), "清理波及他人 ref：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏 settings.yaml：报错且文件不被覆盖。
    #[test]
    fn bad_settings_rejected() {
        let dir = tmpdir("bad");
        let (json, settings, creds, state, legacy) = paths(&dir);
        std::fs::write(&settings, "ui-theme: [broken\n").unwrap();
        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &settings, &creds, &state, &legacy).is_err());
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "ui-theme: [broken\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// JSON 缺失：settings/credentials 不动，patch/state 派生物清理。
    #[test]
    fn json_missing_cleans_derivatives_only() {
        let dir = tmpdir("missing");
        let (json, settings, creds, state, legacy) = paths(&dir);
        std::fs::write(&settings, "ui-theme:\n  preference: dark\n").unwrap();
        std::fs::write(&creds, "version: 1\nrefs:\n  USER_X: keep\n").unwrap();
        std::fs::write(&legacy, "stale patch").unwrap();
        std::fs::write(&state, r#"{"written":["old"]}"#).unwrap();
        assert!(!sync_settings_files(&json, &settings, &creds, &state, &legacy).unwrap());
        assert!(!legacy.exists() && !state.exists());
        assert!(std::fs::read_to_string(&settings).unwrap().contains("ui-theme"));
        assert!(std::fs::read_to_string(&creds).unwrap().contains("USER_X"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 默认模型覆盖语义：JSON 有默认时整节写。
    #[test]
    fn default_model_overwrites() {
        let dir = tmpdir("default");
        let (json, settings, creds, state, legacy) = paths(&dir);
        std::fs::write(&settings, "agent-default-model:\n  provider: manual-route\n  model: x\n").unwrap();
        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &settings, &creds, &state, &legacy).unwrap());
        let out = std::fs::read_to_string(&settings).unwrap();
        assert!(out.contains("provider: my-gw"), "默认模型未覆盖：{out}");
        assert!(!out.contains("manual-route"), "旧默认未替换：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空供应商名单：不创建空 llm-pi-ai 节（保持 settings 干净）。
    #[test]
    fn empty_providers_no_op_section() {
        let dir = tmpdir("empty");
        let (json, settings, creds, state, legacy) = paths(&dir);
        std::fs::write(&settings, "ui-theme:\n  preference: dark\n").unwrap();
        std::fs::write(&json, r#"{"providers":[]}"#).unwrap();
        assert!(sync_settings_files(&json, &settings, &creds, &state, &legacy).unwrap());
        let out = std::fs::read_to_string(&settings).unwrap();
        assert!(!out.contains("llm-pi-ai"), "空名单不应写节：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn paths(dir: &std::path::Path) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
        (
            dir.join("otter-models.json"),
            dir.join("settings.yaml"),
            dir.join(".credentials.yaml"),
            dir.join("otter-models.state.json"),
            dir.join("otter-models.patch.yml"),
        )
    }
}
