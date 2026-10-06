// 控制中心"模型与供应商"的后端：配置源（appData/otter-models.json）→ dsh 官方
// 热更新面（$DSH_HOME/profiles/web/cordis.patch.yml 的 llm-pi-ai/
// agent-default-model entry + $DSH_HOME/.credentials.yaml 的 refs，均被 dsh
// chokidar 监听热发布）。
//
// 为什么弃 --patch（v0.1.4–v0.1.13）与根 settings.yaml（v0.1.14）：dsh 0.2.0
// 移除了 settings.yaml 节体系——启动即把该文件 rename 成 .imported 并把各节
// 自动迁移进 active profile 的 cordis.patch.yml（loader patch entry 数组），
// Otter 再写 settings.yaml 没有任何消费者。cordis.patch.yml 是 dsh-hmr 的
// chokidar 精确 watch 目标，外部进程改文件即 reconcileProfilePatches 热重载，
// 路由内容按请求解析、保存后下一请求生效——这正是 dsh Web UI 模型页自己的
// 写路径（dsh-config-editor 落盘同一文件），格式有官方背书。实测：dsh 首启
// 自建 profile 不覆盖预写的 patch 文件，Otter 可安全在 spawn 前写入。
//
// 合并语义（cordis.patch.yml 可能同时被 dsh 模型页编辑）：
// - id 为 llm-pi-ai 的 entry：config.providers 中 JSON 名单内的键替换；名单外
//   一律保留（用户手写路由）；Otter 历史写入、已从名单删除的路由经 state 记账
//   （appData/otter-models.state.json 的 written 名单）精确移除，不误删手写。
// - id 为 agent-default-model 的 entry：JSON 有默认才替换 config，无默认保留
//   现状（可能来自模型页）。其余 entry（用户/其他工具写的）原样保留。
// - credentials refs：OTTER_KEY_ 前缀是 Otter 命名空间，非本次派生名集合即删；
//   其余 refs 与 records 原样保留。
// - 升级路径（v0.1.14 的根 settings.yaml）：模型节（llm-pi-ai/
//   agent-default-model）由 Otter 摘除——名单外手写路由并入 patch 后从该文件
//   删除模型节，其余节留给 dsh 自动迁移；不先摘除会让 dsh 迁移把已删路由复活。
// 已知取舍：两编辑器文件级并发无乐观锁，最后写者赢（单用户场景窗口极小）；
// 改写不保用户注释（dsh 自身改写同一文件同样不保）；用户手写 entry 若含
// serde_yaml 不支持的 tag（如 !!js 表达式）会拒写整个文件（不覆盖）。

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

/// 保存配置：写 JSON 源并同步到 dsh 热更新面（cordis.patch.yml +
/// .credentials.yaml），后端运行中下一请求即生效、未运行则下次启动生效。
/// 返回 patch 路径（日志用）。
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
    let patch = profile_patch_path(&home);
    sync_settings_files(
        &dir.join("otter-models.json"),
        &patch,
        &home.join("settings.yaml"),
        &home.join(".credentials.yaml"),
        &dir.join("otter-models.state.json"),
        &dir.join("otter-models.patch.yml"),
    )?;
    Ok(patch.to_string_lossy().into_owned())
}

/// Otter 投影的落点：spawn 固定 boot web profile（backend 的 dsh_args），
/// 故只管 web profile 的 patch 层；其他 profile 用户自建自管（薄壳边界）。
fn profile_patch_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("profiles").join("web").join("cordis.patch.yml")
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
        &profile_patch_path(&home),
        &home.join("settings.yaml"),
        &home.join(".credentials.yaml"),
        &dir.join("otter-models.state.json"),
        &dir.join("otter-models.patch.yml"),
    ) {
        log(&format!("[models] 同步供应商配置到 cordis.patch.yml 失败，本次跳过：{e}"));
    }
}

/// 纯文件逻辑（可测）：JSON 存在则把供应商配置合并进 web profile 的
/// cordis.patch.yml 与 .credentials.yaml、摘除 v0.1.14 根 settings.yaml 的
/// 模型节（手写路由先并入 patch）、更新记账、清理 v0.1.13 及以前的 patch
/// 残留，返回 true；JSON 缺失（从未配置/被清理）不动 patch/credentials
/// （可能已有用户手写内容），清理全部派生物（patch + state），返回 false。
fn sync_settings_files(
    json: &std::path::Path,
    patch: &std::path::Path,
    legacy_settings: &std::path::Path,
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
    // 升级摘除：v0.1.14 根 settings.yaml 的手写路由并入、模型节移除（其余节
    // 留给 dsh 启动迁移）。摘除失败（坏 YAML/坏结构）让整个同步报错，不写半套。
    let (legacy_providers, legacy_default) = strip_legacy_settings(legacy_settings, &routes)?;
    let mut merged = providers.clone();
    for (k, v) in &legacy_providers {
        merged.insert(k.clone(), v.clone());
    }
    let default = default.or(legacy_default);
    let prev_written = read_written(state);
    merge_patch(patch, &merged, default.as_ref(), &prev_written, &routes)?;
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

/// 读 YAML 文档为 sequence：缺失/空白 → 空表；存在但非法 → Err（不静默
/// 覆盖用户文件）；顶层非 sequence → Err。
fn read_yaml_sequence(path: &std::path::Path) -> Result<Vec<Yaml>, String> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("读取 {} 失败：{e}", path.display())),
        Ok(text) if text.trim().is_empty() => Ok(Vec::new()),
        Ok(text) => {
            let v: Yaml = serde_yaml::from_str(&text)
                .map_err(|e| format!("{} 不是合法 YAML：{e}", path.display()))?;
            v.as_sequence()
                .cloned()
                .ok_or_else(|| format!("{} 顶层不是数组（loader patch 应为 entry 列表）", path.display()))
        }
    }
}

/// mapping 文档取子 mapping，不存在则插入空表；已存在但非 mapping → Err。
fn ensure_mapping<'a>(parent: &'a mut Mapping, key: &str) -> Result<&'a mut Mapping, String> {
    let k = Yaml::String(key.into());
    if !parent.contains_key(&k) {
        parent.insert(k.clone(), Yaml::Mapping(Mapping::new()));
    }
    parent
        .get_mut(&k)
        .and_then(|v| v.as_mapping_mut())
        .ok_or_else(|| format!("YAML 的 {key} 节损坏（非映射）"))
}

/// 合并进 web profile 的 cordis.patch.yml：id 为 llm-pi-ai 的 entry 里，
/// Otter 名单内路由替换、prev_written 中已删除路由移除、名单外（用户手写，
/// 含 dsh 从旧 settings.yaml 自动迁移来的）保留；id 为 agent-default-model
/// 的 entry 有默认才替换 config；其余 entry 原样保留。需要的 entry 不存在
/// 则追加（带官方包名 name）。
fn merge_patch(
    path: &std::path::Path,
    otter_providers: &Mapping,
    default: Option<&(String, String)>,
    prev_written: &[String],
    routes: &[String],
) -> Result<(), String> {
    let mut entries = read_yaml_sequence(path)?;
    if !otter_providers.is_empty() || !prev_written.is_empty() {
        let idx = find_entry(&entries, "llm-pi-ai");
        if idx.is_none() {
            entries.push(new_entry("llm-pi-ai", "@deepseek-ai/dsh-llm-pi-ai"));
        }
        let idx = find_entry(&entries, "llm-pi-ai").unwrap();
        let entry = entries[idx]
            .as_mapping_mut()
            .ok_or_else(|| "cordis.patch.yml 的 llm-pi-ai entry 损坏（非映射）".to_string())?;
        let config = ensure_entry_mapping(entry, "config")?;
        let providers = ensure_entry_mapping(config, "providers")?;
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
        let idx = find_entry(&entries, "agent-default-model");
        if idx.is_none() {
            entries.push(new_entry(
                "agent-default-model",
                "@deepseek-ai/dsh-agent-default-model",
            ));
        }
        let idx = find_entry(&entries, "agent-default-model").unwrap();
        let entry = entries[idx].as_mapping_mut().ok_or_else(|| {
            "cordis.patch.yml 的 agent-default-model entry 损坏（非映射）".to_string()
        })?;
        let mut dm = Mapping::new();
        dm.insert(Yaml::String("provider".into()), Yaml::String(p.clone()));
        dm.insert(Yaml::String("model".into()), Yaml::String(m.clone()));
        entry.insert(Yaml::String("config".into()), Yaml::Mapping(dm));
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = serde_yaml::to_string(&Yaml::Sequence(entries))
        .map_err(|e| format!("序列化 cordis.patch.yml 失败：{e}"))?;
    std::fs::write(path, text).map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

/// 在 entry 列表中按 id 定位（loader patch 的定位键是 id，name 仅提示包名）。
fn find_entry(entries: &[Yaml], id: &str) -> Option<usize> {
    entries.iter().position(|e| {
        e.as_mapping()
            .and_then(|m| m.get("id"))
            .and_then(|v| v.as_str())
            .is_some_and(|s| s == id)
    })
}

/// 构造一个最小 patch entry（id + name，config 由调用方补）。
fn new_entry(id: &str, name: &str) -> Yaml {
    let mut m = Mapping::new();
    m.insert(Yaml::String("id".into()), Yaml::String(id.into()));
    m.insert(Yaml::String("name".into()), Yaml::String(name.into()));
    Yaml::Mapping(m)
}

/// 取 entry 的子 mapping，不存在则插入空表；已存在但非 mapping → Err。
fn ensure_entry_mapping<'a>(parent: &'a mut Mapping, key: &str) -> Result<&'a mut Mapping, String> {
    let k = Yaml::String(key.into());
    if !parent.contains_key(&k) {
        parent.insert(k.clone(), Yaml::Mapping(Mapping::new()));
    }
    parent
        .get_mut(&k)
        .and_then(|v| v.as_mapping_mut())
        .ok_or_else(|| format!("entry 的 {key} 节损坏（非映射）"))
}

/// 摘除 v0.1.14 根 settings.yaml 的模型节（dsh 0.2.0 已不读该文件，但启动
/// 会把各节自动迁移进 profile——不摘除会把 Otter 已删的路由复活）。手写路由
/// （Otter 名单外）与默认模型先提取出来交还调用方并入 patch；其余节原样保留
/// （dsh 迁移它们）。摘完无剩余节则删除文件，有则写回。文件不存在/无模型节
/// → 无操作。
fn strip_legacy_settings(
    path: &std::path::Path,
    routes: &[String],
) -> Result<(Mapping, Option<(String, String)>), String> {
    let mut doc = match read_yaml_mapping(path) {
        Ok(doc) => doc,
        // 读不了且不是"不存在"类错误（坏 YAML）：让调用方整体报错，不写半套。
        Err(e) => return Err(e),
    };
    if !doc.contains_key(&Yaml::String("llm-pi-ai".into()))
        && !doc.contains_key(&Yaml::String("agent-default-model".into()))
    {
        return Ok((Mapping::new(), None));
    }
    // 提取手写路由（名单外）与旧默认（保持现状用）。
    let mut keep = Mapping::new();
    let mut legacy_default = None;
    if let Some(Yaml::Mapping(llm)) = doc.get(&Yaml::String("llm-pi-ai".into())) {
        if let Some(Yaml::Mapping(providers)) = llm.get(&Yaml::String("providers".into())) {
            for (k, v) in providers {
                let outside = k
                    .as_str()
                    .is_none_or(|s| !routes.iter().any(|r| r == s));
                if outside {
                    keep.insert(k.clone(), v.clone());
                }
            }
        }
    }
    if let Some(Yaml::Mapping(dm)) = doc.get(&Yaml::String("agent-default-model".into())) {
        let provider = dm.get(&Yaml::String("provider".into())).and_then(|v| v.as_str());
        let model = dm.get(&Yaml::String("model".into())).and_then(|v| v.as_str());
        if let (Some(p), Some(m)) = (provider, model) {
            if !p.is_empty() && !m.is_empty() {
                legacy_default = Some((p.to_string(), m.to_string()));
            }
        }
    }
    doc.remove(Yaml::String("llm-pi-ai".into()));
    doc.remove(Yaml::String("agent-default-model".into()));
    if doc.is_empty() {
        std::fs::remove_file(path).map_err(|e| format!("删除 {} 失败：{e}", path.display()))?;
    } else {
        let text = serde_yaml::to_string(&Yaml::Mapping(doc))
            .map_err(|e| format!("序列化 {} 失败：{e}", path.display()))?;
        std::fs::write(path, text).map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    }
    Ok((keep, legacy_default))
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

    /// 合并语义：用户手写路由与其他 entry 保留、Otter 路由写入；无默认时
    /// agent-default-model entry 保持现状。
    #[test]
    fn patch_merge_keeps_user_content() {
        let dir = tmpdir("merge");
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&patch, "- id: ui-theme\n  config:\n    preference: dark\n- id: llm-pi-ai\n  name: \"@deepseek-ai/dsh-llm-pi-ai\"\n  config:\n    providers:\n      manual-route:\n        api: openai-completions\n- id: agent-default-model\n  config:\n    provider: manual-route\n    model: x\n").unwrap();

        let mut cfg = full_config();
        cfg["defaultProvider"] = serde_json::json!(null);
        cfg["defaultModel"] = serde_json::json!(null);
        std::fs::write(&json, serde_json::to_string(&cfg).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());

        let out = std::fs::read_to_string(&patch).unwrap();
        assert!(out.contains("ui-theme"), "用户 entry 被丢：{out}");
        assert!(out.contains("manual-route"), "手写路由被删：{out}");
        assert!(out.contains("my-gw:"), "Otter 路由未写入：{out}");
        assert!(out.contains("provider: manual-route"), "无默认时不应覆盖 agent-default-model：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 增改删全链路：改 baseURL + 删 free-gw 后再同步——被删路由消失（state
    /// 记账）、幸存路由更新、手写路由仍在、credentials 派生 ref 同步清理。
    #[test]
    fn patch_replace_and_remove_via_state() {
        let dir = tmpdir("replace");
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&patch, "- id: llm-pi-ai\n  config:\n    providers:\n      manual-route:\n        api: openai-completions\n").unwrap();
        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());

        // 第二轮：删 free-gw、改 my-gw 的 baseURL。
        let mut cfg = full_config();
        cfg["providers"].as_array_mut().unwrap().retain(|p| p["name"] != "free-gw");
        cfg["providers"][0]["baseURL"] = serde_json::json!("https://gw2.example/v1");
        std::fs::write(&json, serde_json::to_string(&cfg).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());

        let out = std::fs::read_to_string(&patch).unwrap();
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
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&creds, "version: 1\nrefs:\n  USER_X: keep-me\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload:\n      secret: s\n").unwrap();

        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());
        let out = std::fs::read_to_string(&creds).unwrap();
        assert!(out.contains("USER_X") && out.contains("keep-me"), "他人 ref 被清：{out}");
        assert!(out.contains("kind: grant"), "records 被清：{out}");
        assert!(out.contains("OTTER_KEY_MY_GW"), "派生 ref 未写入：{out}");
        // key 本体就该落在 refs（credentials 文件是 dsh 官方明文 key 存储位），
        // 但绝不能进 patch 的 provider 节。
        assert!(!std::fs::read_to_string(&patch).unwrap().contains("sk-SECRET"));

        // 删直填 key 路由 → OTTER_KEY ref 清理、他人 ref 保留。
        let mut cfg = full_config();
        cfg["providers"].as_array_mut().unwrap().retain(|p| p["name"] != "my-gw");
        cfg["defaultProvider"] = serde_json::json!("free-gw");
        cfg["defaultModel"] = serde_json::json!("f1");
        std::fs::write(&json, serde_json::to_string(&cfg).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());
        let out = std::fs::read_to_string(&creds).unwrap();
        assert!(!out.contains("OTTER_KEY_MY_GW"), "已删路由的 ref 未清理：{out}");
        assert!(out.contains("USER_X"), "清理波及他人 ref：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏 cordis.patch.yml：报错且文件不被覆盖。
    #[test]
    fn bad_patch_rejected() {
        let dir = tmpdir("bad");
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&patch, "ui-theme: [broken\n").unwrap();
        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).is_err());
        assert_eq!(std::fs::read_to_string(&patch).unwrap(), "ui-theme: [broken\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// JSON 缺失：patch/credentials 不动，state/legacy 派生物清理。
    #[test]
    fn json_missing_cleans_derivatives_only() {
        let dir = tmpdir("missing");
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&patch, "- id: ui-theme\n  config:\n    preference: dark\n").unwrap();
        std::fs::write(&creds, "version: 1\nrefs:\n  USER_X: keep\n").unwrap();
        std::fs::write(&legacyp, "stale patch").unwrap();
        std::fs::write(&state, r#"{"written":["old"]}"#).unwrap();
        assert!(!sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());
        assert!(!legacyp.exists() && !state.exists());
        assert!(std::fs::read_to_string(&patch).unwrap().contains("ui-theme"));
        assert!(std::fs::read_to_string(&creds).unwrap().contains("USER_X"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 默认模型覆盖语义：JSON 有默认时替换 entry config。
    #[test]
    fn default_model_overwrites() {
        let dir = tmpdir("default");
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&patch, "- id: agent-default-model\n  config:\n    provider: manual-route\n    model: x\n").unwrap();
        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());
        let out = std::fs::read_to_string(&patch).unwrap();
        assert!(out.contains("provider: my-gw"), "默认模型未覆盖：{out}");
        assert!(!out.contains("manual-route"), "旧默认未替换：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空供应商名单：不创建 llm-pi-ai entry（保持 patch 干净）。
    #[test]
    fn empty_providers_no_op_section() {
        let dir = tmpdir("empty");
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&patch, "- id: ui-theme\n  config:\n    preference: dark\n").unwrap();
        std::fs::write(&json, r#"{"providers":[]}"#).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());
        let out = std::fs::read_to_string(&patch).unwrap();
        assert!(!out.contains("llm-pi-ai"), "空名单不应写节：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// v0.1.14 升级路径：根 settings.yaml 的模型节被摘除——手写路由并入
    /// patch、其他节（ui-theme）保留给 dsh 迁移、Otter 名单路由不重复搬运
    /// （以 JSON 源为准重写）、旧默认在 JSON 无默认时保持。
    #[test]
    fn legacy_settings_stripped_and_merged() {
        let dir = tmpdir("strip");
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&legacy, "ui-theme:\n  preference: dark\nllm-pi-ai:\n  providers:\n    manual-route:\n      api: openai-completions\nagent-default-model:\n  provider: manual-route\n  model: legacy-model\n").unwrap();

        let mut cfg = full_config();
        cfg["defaultProvider"] = serde_json::json!(null);
        cfg["defaultModel"] = serde_json::json!(null);
        std::fs::write(&json, serde_json::to_string(&cfg).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());

        let out = std::fs::read_to_string(&patch).unwrap();
        assert!(out.contains("manual-route"), "手写路由未并入 patch：{out}");
        assert!(out.contains("my-gw:"), "Otter 路由未写入：{out}");
        assert!(out.contains("provider: manual-route"), "旧默认未保持：{out}");
        let legacy_out = std::fs::read_to_string(&legacy).unwrap();
        assert!(legacy_out.contains("ui-theme"), "非模型节被误删：{legacy_out}");
        assert!(!legacy_out.contains("llm-pi-ai"), "模型节未摘除：{legacy_out}");
        assert!(!legacy_out.contains("agent-default-model"), "默认节未摘除：{legacy_out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 升级路径续：只剩模型节的旧 settings.yaml 摘除后整文件删除（省得 dsh
    /// 做空迁移）；JSON 有默认时覆盖旧默认。
    #[test]
    fn legacy_settings_all_model_sections_removed() {
        let dir = tmpdir("strip2");
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&legacy, "llm-pi-ai:\n  providers:\n    manual-route:\n      api: openai-completions\nagent-default-model:\n  provider: manual-route\n  model: legacy-model\n").unwrap();
        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).unwrap());
        assert!(!legacy.exists(), "纯模型节文件应删除：{legacy:?}");
        let out = std::fs::read_to_string(&patch).unwrap();
        assert!(out.contains("provider: my-gw"), "JSON 默认未生效：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 升级路径防线：坏 legacy settings.yaml 拒绝整个同步（不写半套）。
    #[test]
    fn bad_legacy_settings_rejected() {
        let dir = tmpdir("strip3");
        let (json, patch, legacy, creds, state, legacyp) = paths(&dir);
        std::fs::write(&legacy, "ui-theme: [broken\n").unwrap();
        std::fs::write(&json, serde_json::to_string(&full_config()).unwrap()).unwrap();
        assert!(sync_settings_files(&json, &patch, &legacy, &creds, &state, &legacyp).is_err());
        assert!(!patch.exists(), "失败时不应写 patch：{patch:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn paths(dir: &std::path::Path) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
        (
            dir.join("otter-models.json"),
            dir.join("cordis.patch.yml"),
            dir.join("settings.yaml"),
            dir.join(".credentials.yaml"),
            dir.join("otter-models.state.json"),
            dir.join("otter-models.patch.yml"),
        )
    }
}
