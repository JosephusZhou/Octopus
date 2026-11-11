//! Pi / Oh-My-Pi 供应商与模型配置读写（通用模型配置框架的 Pi 家族后端）。
//!
//! - Pi：`~/.pi/agent/models.json`（JSON，顶层 `providers`）
//!   - 密钥：`~/.pi/agent/auth.json`（`{ providerId: { type: "api_key", key } }`）
//!   - 默认模型：`~/.pi/agent/settings.json` 的 `defaultProvider` / `defaultModel`
//! - Oh-My-Pi：`~/.omp/agent/models.yml`（YAML；兼容读 `.yaml` 与旧版 `models.json`）
//!   - 密钥：自定义 provider 的 `apiKey`（可为环境变量名或字面密钥）；旧版
//!     `auth.json` 仅在首次读取时迁移，不再作为运行时来源
//!   - 默认模型：`~/.omp/agent/config.yml` 的 `modelRoles.default`
//!   - 第二模型槽位：`~/.omp/agent/config.yml` 的 `modelRoles.fast`
//! - OmO Native：`~/.omo/agent/models.json`（JSONC；与 Pi 同一 senpi 引擎 schema）
//!   - 密钥：`~/.omo/agent/auth.json`（`{ providerId: { type: "api_key", key } }`）
//!   - 默认模型：`~/.omo/agent/settings.json` 的 `defaultProvider` / `defaultModel`
//!   - 第二模型槽位：`~/.omo/omo.jsonc` 的 `[native].categories.quick.model`
//!   - 上游以 0600 权限写 models.json（provider headers 可能携带 token）
//!
//! 字段映射到通用 DTO：`contextWindow`→limitContext、`maxTokens`→limitOutput、
//! `input`→modalitiesInput；未建模字段经 extraOptions 原样往返。
//! 列表 DTO **永不**回传明文 API Key，与 OpenCode 后端一致。

use crate::agent_model_config::ModelConfigAgent;
use crate::opencode_config::{
    api_key_source, atomic_write, atomic_write_secret, auth_get_key, auth_has_key, bool_opt,
    display_path, f64_opt, load_auth_file, save_auth_file, str_opt, string_list, AgentActionResult,
    AgentModelConfigView, AgentModelView, AgentProviderView, SetDefaultsPayload,
    UpsertModelPayload, UpsertProviderPayload,
};
use crate::platform;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Pi 家族模型条目的已知键；其余字段进 extraOptions 原样往返。
const MODEL_KNOWN_KEYS: &[&str] = &[
    "id",
    "name",
    "reasoning",
    "input",
    "contextWindow",
    "maxTokens",
];

/// Pi 家族的 models schema 只接受 `text` 或 `text + image`。
/// 其它通用目录模态（pdf/audio/video）不能写入 Pi 家族配置。
fn normalize_input_modalities(input: &[String]) -> Vec<String> {
    let has_text = input.iter().any(|value| value == "text");
    let has_image = input.iter().any(|value| value == "image");
    match (has_text, has_image) {
        (false, false) => Vec::new(),
        (true, false) => vec!["text".into()],
        (_, true) => vec!["text".into(), "image".into()],
    }
}

/* ===== Paths ===== */

struct FamilyPaths {
    /// 模型配置文件（存在者优先，否则取默认名）。
    models: PathBuf,
    /// models 文件是否按 YAML 读写。
    yaml: bool,
    /// 模型配置可能包含明文 apiKey / token，写入后必须限制为当前用户可读写。
    private_models: bool,
    /// 密钥以 `auth.json` 为运行时来源（Pi / OmO）；否则以 provider `apiKey` 为准（OMP）。
    auth_primary: bool,
    auth: PathBuf,
    /// 默认模型 settings（Pi: settings.json；OMP: config.yml）。
    settings: Option<PathBuf>,
    settings_yaml: bool,
}

fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "无法解析用户主目录".to_string())
}

/// 按候选顺序返回第一个存在的路径；都不存在时返回第一个候选。
fn pick_existing(base: &Path, names: &[&str]) -> PathBuf {
    names
        .iter()
        .map(|name| base.join(name))
        .find(|path| path.exists())
        .unwrap_or_else(|| base.join(names[0]))
}

fn family_paths(agent: ModelConfigAgent) -> Result<FamilyPaths, String> {
    let home = home_dir()?;
    match agent {
        ModelConfigAgent::Pi => Ok(FamilyPaths {
            models: home.join(".pi/agent/models.json"),
            yaml: false,
            private_models: false,
            auth_primary: true,
            auth: home.join(".pi/agent/auth.json"),
            settings: Some(home.join(".pi/agent/settings.json")),
            settings_yaml: false,
        }),
        ModelConfigAgent::Omo => Ok(FamilyPaths {
            models: home.join(".omo/agent/models.json"),
            yaml: false,
            private_models: true,
            auth_primary: true,
            auth: home.join(".omo/agent/auth.json"),
            // 上游 settings.jsonc 优先于 settings.json。
            settings: Some(pick_existing(
                &home.join(".omo/agent"),
                &["settings.jsonc", "settings.json"],
            )),
            settings_yaml: false,
        }),
        ModelConfigAgent::OhMyPi => {
            let base = home.join(".omp/agent");
            let models = pick_existing(&base, &["models.yml", "models.yaml", "models.json"]);
            let yaml = matches!(
                models.extension().and_then(|e| e.to_str()),
                Some("yml") | Some("yaml")
            );
            Ok(FamilyPaths {
                models,
                yaml,
                private_models: true,
                auth_primary: false,
                auth: base.join("auth.json"),
                settings: Some(pick_existing(&base, &["config.yml", "config.yaml"])),
                settings_yaml: true,
            })
        }
        ModelConfigAgent::Opencode => unreachable!("opencode 不走 Pi 家族后端"),
    }
}

/* ===== IO ===== */

/// YAML 序列化并确保以换行结尾。
fn yaml_text(value: &Value) -> Result<String, String> {
    let mut text = serde_yaml::to_string(value).map_err(|e| format!("序列化失败: {e}"))?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

fn parse_root_text(path: &Path, raw: &str, yaml: bool) -> Result<Value, String> {
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    if yaml {
        serde_yaml::from_str(raw).map_err(|e| format!("解析 {} 失败: {e}", path.display()))
    } else {
        json5::from_str(raw).map_err(|e| format!("解析 {} 失败: {e}", path.display()))
    }
}

fn load_models_root(paths: &FamilyPaths) -> Result<(Value, bool), String> {
    if !paths.models.exists() {
        return Ok((json!({}), false));
    }
    let raw = fs::read_to_string(&paths.models)
        .map_err(|e| format!("读取 {} 失败: {e}", paths.models.display()))?;
    let v = parse_root_text(&paths.models, &raw, paths.yaml)?;
    match v {
        Value::Object(_) => Ok((v, true)),
        _ => Err(format!("{} 根节点必须是对象", paths.models.display())),
    }
}

fn write_models_root(paths: &FamilyPaths, root: &Value) -> Result<(), String> {
    let text = if paths.yaml {
        yaml_text(root)?
    } else {
        let t = serde_json::to_string_pretty(root).map_err(|e| format!("序列化失败: {e}"))?;
        format!("{t}\n")
    };
    // OMP 的 models 文件可能含明文 apiKey，写入后限制为当前用户可读写。
    if paths.private_models {
        atomic_write_secret(&paths.models, &text)
    } else {
        atomic_write(&paths.models, &text)
    }
}

fn set_or_remove_str(map: &mut Map<String, Value>, key: &str, value: &Option<String>) {
    if let Some(v) = value {
        if v.is_empty() {
            map.remove(key);
        } else {
            map.insert(key.to_string(), Value::String(v.clone()));
        }
    }
}

fn providers_map_mut(root: &mut Value) -> Result<&mut Map<String, Value>, String> {
    let root_obj = root
        .as_object_mut()
        .ok_or_else(|| "配置根节点无效".to_string())?;
    let providers = root_obj
        .entry("providers".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !providers.is_object() {
        return Err("providers 节点必须是对象".into());
    }
    Ok(providers.as_object_mut().unwrap())
}

fn provider_object_mut<'a>(
    root: &'a mut Value,
    id: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    let map = providers_map_mut(root)?;
    let entry = map
        .entry(id.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    entry
        .as_object_mut()
        .ok_or_else(|| format!("providers.{id} 必须是对象"))
}

/* ===== Parse view ===== */

fn parse_model(raw: &Value) -> AgentModelView {
    let mut extra = Map::new();
    if let Some(obj) = raw.as_object() {
        for (k, v) in obj {
            if !MODEL_KNOWN_KEYS.contains(&k.as_str()) {
                extra.insert(k.clone(), v.clone());
            }
        }
    }
    AgentModelView {
        id: str_opt(raw, "id").unwrap_or_default(),
        name: str_opt(raw, "name"),
        limit_context: f64_opt(raw, "contextWindow"),
        limit_input: None,
        limit_output: f64_opt(raw, "maxTokens"),
        modalities_input: normalize_input_modalities(&string_list(raw, "input")),
        modalities_output: Vec::new(),
        reasoning: bool_opt(raw, "reasoning"),
        tool_call: None,
        attachment: None,
        status: None,
        thinking_type: None,
        thinking_budget_tokens: None,
        reasoning_effort: None,
        text_verbosity: None,
        variants: Vec::new(),
        extra_options: extra,
    }
}

fn parse_provider(id: &str, raw: &Value, auth: &Map<String, Value>) -> AgentProviderView {
    let in_auth = auth_has_key(auth, id);
    // Pi 家族 config 侧 apiKey 常见为环境变量名，非空即视为已配置。
    let in_config = raw
        .get("apiKey")
        .and_then(|k| k.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    let models = raw
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            let mut list: Vec<_> = arr.iter().map(parse_model).collect();
            list.sort_by(|a, b| a.id.cmp(&b.id));
            list
        })
        .unwrap_or_default();

    AgentProviderView {
        id: id.to_string(),
        name: str_opt(raw, "name"),
        package: None,
        api: str_opt(raw, "api"),
        has_api_key: in_auth || in_config,
        api_key_source: api_key_source(in_auth, in_config),
        base_url: str_opt(raw, "baseUrl").or_else(|| str_opt(raw, "baseURL")),
        set_cache_key: None,
        timeout: None,
        chunk_timeout: None,
        whitelist: Vec::new(),
        blacklist: Vec::new(),
        models,
    }
}

fn read_defaults(paths: &FamilyPaths) -> (Option<String>, Option<String>) {
    let Some(sp) = &paths.settings else {
        return (None, None);
    };
    if !sp.exists() {
        return (None, None);
    }
    let Ok(raw) = fs::read_to_string(sp) else {
        return (None, None);
    };
    if paths.settings_yaml {
        // OMP：`modelRoles.default` / `modelRoles.fast` 原样返回（模型 id 可含 `/`，不拆分）。
        let Ok(v) = serde_yaml::from_str::<Value>(&raw) else {
            return (None, None);
        };
        let role = |name: &str| {
            v.get("modelRoles")
                .and_then(|r| r.get(name))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        };
        return (role("default"), role("fast"));
    }
    let Ok(v) = json5::from_str::<Value>(&raw) else {
        return (None, None);
    };
    let provider = str_opt(&v, "defaultProvider");
    let model = str_opt(&v, "defaultModel");
    let combined = match (provider, model) {
        (Some(p), Some(m)) => Some(format!("{p}/{m}")),
        (None, Some(m)) => Some(m),
        _ => None,
    };
    (combined, None)
}

/* ===== OmO model_profile ===== */

/// OmO 固定模型配置文件：`~/.omo/omo.jsonc`，旧版 `omo.json` 兜底；都不存在时返回 `None`。
fn omo_profile_file() -> Option<PathBuf> {
    let home = home_dir().ok()?;
    let base = home.join(".omo");
    ["omo.jsonc", "omo.json"]
        .iter()
        .map(|name| base.join(name))
        .find(|path| path.is_file())
}

/// OmO harness 块键：`[native]` 是现行拼写，`native` 为兼容，`[senpi]` 是旧名称。
/// 读取按此顺序取第一个存在的块；写入回落到第一个已存在的块，避免新建 `[native]`
/// 遮蔽只写了旧 `[senpi]` 的配置（与上游写入器行为一致）。
/// 全大写是 Rust 常量命名约定，勿跟随品牌展示名（OmO）改名。
const OMO_HARNESS_KEYS: &[&str] = &["[native]", "native", "[senpi]"];
const OMO_DEFAULT_HARNESS_KEY: &str = "[native]";

fn omo_container_key(root: &Value) -> Option<&'static str> {
    OMO_HARNESS_KEYS
        .iter()
        .copied()
        .find(|key| root.get(*key).map(Value::is_object).unwrap_or(false))
}

fn omo_container_display_key(root: &Value) -> &'static str {
    omo_container_key(root).unwrap_or(OMO_DEFAULT_HARNESS_KEY)
}

/// 读取 OmO 固定 `model_profile`；兼容容器键与顶层键。
fn read_omo_model_profile() -> Option<(PathBuf, String)> {
    let path = omo_profile_file()?;
    let raw = fs::read_to_string(&path).ok()?;
    let root: Value = json5::from_str(&raw).ok()?;
    let value = omo_container_key(&root)
        .and_then(|key| root.get(key))
        .and_then(|container| container.get("model_profile"))
        .and_then(Value::as_str)
        .or_else(|| root.get("model_profile").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    Some((path, value))
}

/// 读取 OmO `categories.quick.model`；返回 (harness 块键, 模型值)。
///
/// 只读 typed harness 块（`[native]` / `native` / `[senpi]`），不做顶层回退。
fn read_omo_quick_model() -> Option<(&'static str, String)> {
    let path = omo_profile_file()?;
    let raw = fs::read_to_string(&path).ok()?;
    let root: Value = json5::from_str(&raw).ok()?;
    let key = omo_container_key(&root)?;
    let value = root
        .get(key)
        .and_then(|container| container.get("categories"))
        .and_then(|categories| categories.get("quick"))
        .and_then(|quick| quick.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    Some((key, value))
}

/// 写入或移除 OmO 固定 `model_profile` / `categories.quick.model`。
///
/// 两个参数均为三态：`None` = 不改动，`Some("")` = 删除，其它 = 写入。
/// 两项在同一次读-改-写内完成，避免重复重写 `omo.jsonc`。
/// 文件以 JSON5 读取、标准 JSON 写回（注释会被丢弃）。
fn write_omo_aux(
    model_profile: Option<&str>,
    quick_model: Option<&str>,
) -> Result<PathBuf, String> {
    // 两个候选文件都不存在时按空配置在标准路径创建（atomic_write 会建父目录），
    // 让「首次设置 quick 模型」不必先去手工建 omo.jsonc。
    let (path, raw) = match omo_profile_file() {
        Some(path) => {
            let raw = fs::read_to_string(&path)
                .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
            (path, raw)
        }
        None => (home_dir()?.join(".omo").join("omo.jsonc"), "{}".to_string()),
    };
    let mut root: Value =
        json5::from_str(&raw).map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
    if !root.is_object() {
        return Err(format!("{} 根节点必须是对象", path.display()));
    }
    let key = omo_container_display_key(&root);
    // harness 块存在但不是对象时直接报错，绝不覆盖用户已有值。
    if root.get(key).is_some_and(|block| !block.is_object()) {
        return Err(format!("{} 的 {} 必须是对象", path.display(), key));
    }
    {
        let obj = root.as_object_mut().unwrap();
        if !obj.contains_key(key) {
            obj.insert(key.to_string(), json!({}));
        }
    }
    let container = root.get_mut(key).and_then(Value::as_object_mut).unwrap();

    if let Some(value) = model_profile {
        if value.is_empty() {
            container.remove("model_profile");
        } else {
            container.insert("model_profile".into(), Value::String(value.to_string()));
        }
    }

    if let Some(value) = quick_model {
        // 删除分支原先遇到非对象会静默跳过；这里与写入分支一致，先校验再改。
        if let Some(categories) = container.get("categories") {
            if !categories.is_object() {
                return Err("categories 必须是对象".into());
            }
            if categories
                .get("quick")
                .is_some_and(|quick| !quick.is_object())
            {
                return Err("categories.quick 必须是对象".into());
            }
        }
        if value.is_empty() {
            let drop_categories = {
                let mut drop_categories = false;
                if let Some(categories) = container
                    .get_mut("categories")
                    .and_then(Value::as_object_mut)
                {
                    if let Some(quick) = categories.get_mut("quick").and_then(Value::as_object_mut)
                    {
                        quick.remove("model");
                        if quick.is_empty() {
                            categories.remove("quick");
                        }
                    }
                    drop_categories = categories.is_empty();
                }
                drop_categories
            };
            if drop_categories {
                container.remove("categories");
            }
        } else {
            match container.get_mut("categories") {
                Some(categories) if !categories.is_object() => {
                    return Err("categories 必须是对象".into());
                }
                Some(_) => {}
                None => {
                    container.insert("categories".into(), json!({}));
                }
            }
            let categories = container
                .get_mut("categories")
                .and_then(Value::as_object_mut)
                .unwrap();
            match categories.get_mut("quick") {
                Some(quick) if !quick.is_object() => {
                    return Err("categories.quick 必须是对象".into());
                }
                Some(_) => {}
                None => {
                    categories.insert("quick".into(), json!({}));
                }
            }
            categories
                .get_mut("quick")
                .and_then(Value::as_object_mut)
                .unwrap()
                .insert("model".into(), Value::String(value.to_string()));
        }
    }

    let text = serde_json::to_string_pretty(&root).map_err(|e| format!("序列化失败: {e}"))?;
    atomic_write(&path, &format!("{text}\n"))?;
    Ok(path)
}

/// OMP 旧版 `auth.json` 一次性迁移：把密钥写入 `providers.<id>.apiKey`。
///
/// 只迁移「有 models、`auth` 不为 `none`、且没有非空 `apiKey`」的 provider；
/// 迁移成功后从 `auth.json` 移除对应条目，避免同一密钥存在两份来源。
fn migrate_omp_legacy_auth(paths: &FamilyPaths, root: &mut Value) -> Result<bool, String> {
    if !paths.auth.exists() {
        return Ok(false);
    }
    let auth = load_auth_file(&paths.auth)?;
    if auth.is_empty() {
        return Ok(false);
    }
    let Some(providers) = root.get_mut("providers").and_then(|p| p.as_object_mut()) else {
        return Ok(false);
    };
    let mut migrated = Vec::new();
    for (id, raw) in providers.iter_mut() {
        let Some(obj) = raw.as_object_mut() else {
            continue;
        };
        let has_models = obj
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);
        if !has_models {
            continue;
        }
        if obj.get("auth").and_then(|a| a.as_str()) == Some("none") {
            continue;
        }
        let has_key = obj
            .get("apiKey")
            .and_then(|k| k.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if has_key {
            continue;
        }
        if let Some(key) = auth_get_key(&auth, id) {
            obj.insert("apiKey".into(), Value::String(key));
            migrated.push(id.clone());
        }
    }
    if migrated.is_empty() {
        return Ok(false);
    }
    write_models_root(paths, root)?;
    let mut remaining = auth.clone();
    for id in &migrated {
        remaining.remove(id);
    }
    save_auth_file(&paths.auth, &remaining)?;
    Ok(true)
}

/* ===== Public API ===== */

pub fn get_config(agent: ModelConfigAgent) -> Result<AgentModelConfigView, String> {
    let paths = family_paths(agent)?;
    let installed = crate::sniff::is_agent_installed(agent.id());
    let exists = paths.models.exists();
    let credential_path = if paths.auth_primary {
        paths.auth.clone()
    } else {
        paths.models.clone()
    }
    .to_string_lossy()
    .into_owned();

    let (model, defaults_small_model) = read_defaults(&paths);

    // OmO 另有一个固定 model_profile（headless / desktop 会话会覆盖默认模型），
    // 由前端在保存默认模型时提示是否同步。
    let omo_profile = if agent == ModelConfigAgent::Omo {
        read_omo_model_profile()
    } else {
        None
    };
    let (omo_model_profile, omo_profile_path) = match &omo_profile {
        Some((path, value)) => (
            Some(value.clone()),
            Some(path.to_string_lossy().into_owned()),
        ),
        None => (None, None),
    };

    // 第二模型槽位按 agent 映射：OMP=modelRoles.fast，OmO=categories.quick.model，Pi 无。
    let omo_quick = if agent == ModelConfigAgent::Omo {
        read_omo_quick_model()
    } else {
        None
    };
    let (small_model, small_model_key) = match agent {
        ModelConfigAgent::OhMyPi => (defaults_small_model, Some("modelRoles.fast".to_string())),
        ModelConfigAgent::Omo => {
            let key = omo_quick
                .as_ref()
                .map(|(key, _)| *key)
                .unwrap_or(OMO_DEFAULT_HARNESS_KEY);
            (
                omo_quick.as_ref().map(|(_, value)| value.clone()),
                Some(format!("{key}.categories.quick.model")),
            )
        }
        _ => (None, None),
    };
    let small_model_supported = small_model_key.is_some();

    let not_installed_view = || AgentModelConfigView {
        agent: agent.id().into(),
        config_path: display_path(&paths.models),
        credential_path: credential_path.clone(),
        config_exists: exists,
        is_jsonc: false,
        installed: false,
        defaults_supported: true,
        small_model_supported,
        model: None,
        small_model: None,
        small_model_key: small_model_key.clone(),
        omo_model_profile: omo_model_profile.clone(),
        omo_profile_path: omo_profile_path.clone(),
        enabled_providers: None,
        disabled_providers: None,
        providers: Vec::new(),
        warnings: Vec::new(),
        // Pi 家族没有 1.x → 2.x 的迁移场景，前端不会渲染「迁移」按钮。
        legacy_migration: None,
    };
    if !installed {
        return Ok(not_installed_view());
    }

    let (mut root, _) = load_models_root(&paths)?;
    let mut warnings = Vec::new();
    if paths.yaml {
        warnings.push("当前配置为 YAML；保存后文件内注释会丢失。".to_string());
    }
    let mut migration_failed = false;
    let mut auth = load_auth_file(&paths.auth).unwrap_or_default();
    if agent == ModelConfigAgent::OhMyPi {
        match migrate_omp_legacy_auth(&paths, &mut root) {
            Ok(_) => auth = load_auth_file(&paths.auth).unwrap_or_default(),
            Err(e) => {
                warnings.push(format!("旧版 auth.json 迁移失败：{e}"));
                migration_failed = true;
            }
        }
    }
    let providers = if migration_failed {
        Vec::new()
    } else {
        root.get("providers")
            .and_then(|p| p.as_object())
            .map(|obj| {
                let mut list: Vec<_> = obj
                    .iter()
                    .map(|(id, raw)| parse_provider(id, raw, &auth))
                    .collect();
                list.sort_by(|a, b| a.id.cmp(&b.id));
                list
            })
            .unwrap_or_default()
    };

    Ok(AgentModelConfigView {
        agent: agent.id().into(),
        config_path: display_path(&paths.models),
        credential_path,
        config_exists: exists,
        is_jsonc: false,
        installed: true,
        defaults_supported: true,
        small_model_supported,
        model,
        small_model,
        small_model_key,
        omo_model_profile,
        omo_profile_path,
        enabled_providers: None,
        disabled_providers: None,
        providers,
        warnings,
        legacy_migration: None,
    })
}

pub fn upsert_provider(
    agent: ModelConfigAgent,
    payload: UpsertProviderPayload,
) -> Result<AgentActionResult, String> {
    let id = payload.id.trim().to_string();
    if id.is_empty() {
        return Err("供应商 ID 不能为空".into());
    }
    if id.contains('/') || id.contains(' ') {
        return Err("供应商 ID 不能包含空格或 /".into());
    }

    let paths = family_paths(agent)?;
    let (mut root, _) = load_models_root(&paths)?;

    // Rename: move previous_id → id（同步移动 auth 条目）
    if let Some(prev) = payload
        .previous_id
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && *s != id)
    {
        let map = providers_map_mut(&mut root)?;
        if let Some(old) = map.remove(&prev) {
            if map.contains_key(&id) {
                return Err(format!("供应商 `{id}` 已存在，无法重命名"));
            }
            map.insert(id.clone(), old);
        }
        let mut auth = load_auth_file(&paths.auth)?;
        if let Some(a) = auth.remove(&prev) {
            auth.insert(id.clone(), a);
            save_auth_file(&paths.auth, &auth)?;
        }
    }

    {
        let p = provider_object_mut(&mut root, &id)?;
        set_or_remove_str(p, "name", &payload.name);
        set_or_remove_str(p, "api", &payload.api);
        if let Some(ref bu) = payload.base_url {
            if bu.is_empty() {
                p.remove("baseUrl");
            } else {
                p.insert("baseUrl".into(), Value::String(bu.clone()));
            }
        }
        // OpenCode 特有字段（npm/setCacheKey/timeout/whitelist/blacklist）在 Pi 家族中忽略。
    }

    // Pi / OmO 的 config 内 apiKey 多为环境变量名，保持原样不动；密钥写入 auth.json。
    // OMP 的密钥归属 provider 的 apiKey（可为环境变量名或字面密钥），同时清理旧 auth 条目。
    if let Some(ref key) = payload.api_key {
        if agent == ModelConfigAgent::OhMyPi {
            {
                let p = provider_object_mut(&mut root, &id)?;
                if key.is_empty() {
                    p.remove("apiKey");
                } else {
                    p.insert("apiKey".into(), Value::String(key.clone()));
                }
            }
            let mut auth = load_auth_file(&paths.auth)?;
            if auth.remove(&id).is_some() {
                save_auth_file(&paths.auth, &auth)?;
            }
        } else {
            let mut auth = load_auth_file(&paths.auth)?;
            if key.is_empty() {
                auth.remove(&id);
            } else {
                auth.insert(
                    id.clone(),
                    json!({
                        "type": "api_key",
                        "key": key,
                    }),
                );
            }
            save_auth_file(&paths.auth, &auth)?;
        }
    }

    write_models_root(&paths, &root)?;
    Ok(AgentActionResult {
        ok: true,
        message: format!("供应商 `{id}` 已保存"),
        view: Some(get_config(agent)?),
    })
}

pub fn delete_provider(
    agent: ModelConfigAgent,
    provider_id: String,
    delete_auth: bool,
) -> Result<AgentActionResult, String> {
    let id = provider_id.trim().to_string();
    if id.is_empty() {
        return Err("供应商 ID 不能为空".into());
    }
    let paths = family_paths(agent)?;
    let (mut root, exists) = load_models_root(&paths)?;
    if !exists {
        return Err("配置文件不存在".into());
    }
    let removed = root
        .as_object_mut()
        .and_then(|o| o.get_mut("providers"))
        .and_then(|p| p.as_object_mut())
        .and_then(|m| m.remove(&id))
        .is_some();
    if !removed {
        return Err(format!("未找到供应商 `{id}`"));
    }
    write_models_root(&paths, &root)?;
    if delete_auth {
        let mut auth = load_auth_file(&paths.auth)?;
        auth.remove(&id);
        save_auth_file(&paths.auth, &auth)?;
    }
    Ok(AgentActionResult {
        ok: true,
        message: format!("已删除供应商 `{id}`"),
        view: Some(get_config(agent)?),
    })
}

/// 在 provider 的 models 数组中按 id 定位条目下标。
fn find_model_index(arr: &[Value], id: &str) -> Option<usize> {
    arr.iter()
        .position(|m| m.get("id").and_then(|x| x.as_str()) == Some(id))
}

pub fn upsert_model(
    agent: ModelConfigAgent,
    payload: UpsertModelPayload,
) -> Result<AgentActionResult, String> {
    let pid = payload.provider_id.trim().to_string();
    let mid = payload.id.trim().to_string();
    if pid.is_empty() || mid.is_empty() {
        return Err("供应商 ID 与模型 ID 均不能为空".into());
    }

    let paths = family_paths(agent)?;
    let (mut root, _) = load_models_root(&paths)?;

    // Ensure provider exists
    {
        let _ = provider_object_mut(&mut root, &pid)?;
    }

    let p = provider_object_mut(&mut root, &pid)?;
    let models_val = p
        .entry("models".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !models_val.is_array() {
        return Err("models 必须是数组".into());
    }
    let arr = models_val.as_array_mut().unwrap();

    // Rename: previous_id → mid
    if let Some(prev) = payload
        .previous_id
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && *s != mid)
    {
        if let Some(idx) = find_model_index(arr, &prev) {
            if find_model_index(arr, &mid).is_some() {
                return Err(format!("模型 `{mid}` 已存在"));
            }
            arr[idx]
                .as_object_mut()
                .unwrap()
                .insert("id".into(), Value::String(mid.clone()));
        }
    }

    let idx = match find_model_index(arr, &mid) {
        Some(i) => i,
        None => {
            arr.push(json!({ "id": mid }));
            arr.len() - 1
        }
    };
    let m = arr[idx]
        .as_object_mut()
        .ok_or_else(|| format!("模型 `{mid}` 必须是对象"))?;

    set_or_remove_str(m, "name", &payload.name);

    if let Some(r) = payload.reasoning {
        m.insert("reasoning".into(), Value::Bool(r));
    }
    if let Some(ref inputs) = payload.modalities_input {
        let normalized = normalize_input_modalities(inputs);
        if normalized.is_empty() {
            m.remove("input");
        } else {
            m.insert(
                "input".into(),
                Value::Array(normalized.into_iter().map(Value::String).collect()),
            );
        }
    }
    match payload.limit_context {
        Some(v) if v > 0.0 => {
            m.insert("contextWindow".into(), json!(v));
        }
        Some(_) => {
            m.remove("contextWindow");
        }
        None => {}
    }
    match payload.limit_output {
        Some(v) if v > 0.0 => {
            m.insert("maxTokens".into(), json!(v));
        }
        Some(_) => {
            m.remove("maxTokens");
        }
        None => {}
    }

    if let Some(ref extra) = payload.extra_options {
        if payload.replace_extra_options.unwrap_or(false) {
            let retained: Map<String, Value> = m
                .iter()
                .filter(|(k, _)| MODEL_KNOWN_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            *m = retained;
        }
        for (k, v) in extra {
            m.insert(k.clone(), v.clone());
        }
    }

    write_models_root(&paths, &root)?;
    Ok(AgentActionResult {
        ok: true,
        message: format!("模型 `{pid}/{mid}` 已保存"),
        view: Some(get_config(agent)?),
    })
}

pub fn delete_model(
    agent: ModelConfigAgent,
    provider_id: String,
    model_id: String,
) -> Result<AgentActionResult, String> {
    let pid = provider_id.trim().to_string();
    let mid = model_id.trim().to_string();
    let paths = family_paths(agent)?;
    let (mut root, exists) = load_models_root(&paths)?;
    if !exists {
        return Err("配置文件不存在".into());
    }
    let removed = root
        .as_object_mut()
        .and_then(|o| o.get_mut("providers"))
        .and_then(|p| p.as_object_mut())
        .and_then(|pm| pm.get_mut(&pid))
        .and_then(|p| p.as_object_mut())
        .and_then(|p| p.get_mut("models"))
        .and_then(|m| m.as_array_mut())
        .map(|arr| match find_model_index(arr, &mid) {
            Some(i) => {
                arr.remove(i);
                true
            }
            None => false,
        })
        .unwrap_or(false);
    if !removed {
        return Err(format!("未找到模型 `{pid}/{mid}`"));
    }
    write_models_root(&paths, &root)?;
    Ok(AgentActionResult {
        ok: true,
        message: format!("已删除模型 `{pid}/{mid}`"),
        view: Some(get_config(agent)?),
    })
}

pub fn get_provider_secret(agent: ModelConfigAgent, provider_id: String) -> Result<String, String> {
    let id = provider_id.trim().to_string();
    let paths = family_paths(agent)?;
    let auth = load_auth_file(&paths.auth)?;
    // OMP 以 provider `apiKey` 为准，旧 auth.json 只是兼容来源；Pi / OmO 以 auth.json 为准。
    let (root, _) = load_models_root(&paths)?;
    let config_key = root
        .pointer(&format!("/providers/{id}/apiKey"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let auth_key = auth_get_key(&auth, &id);
    let key = if paths.auth_primary {
        auth_key.or(config_key)
    } else {
        config_key.or(auth_key)
    };
    Ok(key.unwrap_or_default())
}

pub fn set_provider_secret(
    agent: ModelConfigAgent,
    provider_id: String,
    api_key: String,
) -> Result<AgentActionResult, String> {
    upsert_provider(
        agent,
        UpsertProviderPayload {
            id: provider_id,
            previous_id: None,
            name: None,
            package: None,
            api: None,
            base_url: None,
            timeout: None,
            chunk_timeout: None,
            api_key: Some(api_key),
        },
    )
}

pub fn set_defaults(
    agent: ModelConfigAgent,
    payload: SetDefaultsPayload,
) -> Result<AgentActionResult, String> {
    match agent {
        ModelConfigAgent::Pi | ModelConfigAgent::Omo => {
            let paths = family_paths(agent)?;
            let sp = paths
                .settings
                .as_ref()
                .ok_or_else(|| "settings.json 路径缺失".to_string())?;
            let mut root = if sp.exists() {
                crate::opencode_config::read_json_value(sp)?
            } else {
                json!({})
            };
            if !root.is_object() {
                root = json!({});
            }
            // payload.model: None=不改动；Some("")=删除；Some("provider/model")=拆分写入
            if let Some(ref m) = payload.model {
                let obj = root.as_object_mut().unwrap();
                if m.is_empty() {
                    obj.remove("defaultProvider");
                    obj.remove("defaultModel");
                } else if let Some((p, mid)) = m.split_once('/') {
                    obj.insert("defaultProvider".into(), Value::String(p.to_string()));
                    obj.insert("defaultModel".into(), Value::String(mid.to_string()));
                } else {
                    obj.remove("defaultProvider");
                    obj.insert("defaultModel".into(), Value::String(m.clone()));
                }
            }
            let text =
                serde_json::to_string_pretty(&root).map_err(|e| format!("序列化失败: {e}"))?;
            atomic_write(sp, &format!("{text}\n"))?;

            // OmO 的辅助模型统一写 omo.jsonc：model_profile（显式勾选）与
            // categories.quick.model（small_model 槽位），一次读-改-写完成。
            let mut profile_update: Option<String> = None;
            let mut skip_profile_reason: Option<&str> = None;
            if agent == ModelConfigAgent::Omo && payload.sync_omo_model_profile == Some(true) {
                match payload.model.as_deref() {
                    Some("") => profile_update = Some(String::new()),
                    Some(m) if m.contains('/') => profile_update = Some(m.to_string()),
                    Some(_) => skip_profile_reason = Some("模型需为 provider/model 形式"),
                    None => {}
                }
            }
            let quick_update = if agent == ModelConfigAgent::Omo {
                payload.small_model.clone()
            } else {
                None
            };
            let synced_profile = profile_update.is_some();
            let synced_quick = quick_update.is_some();
            let mut message = "默认模型已更新".to_string();
            if synced_profile || synced_quick {
                match write_omo_aux(profile_update.as_deref(), quick_update.as_deref()) {
                    Ok(path) => {
                        let mut items = Vec::new();
                        if synced_profile {
                            items.push("model_profile");
                        }
                        if synced_quick {
                            items.push("categories.quick.model");
                        }
                        message = format!(
                            "默认模型已更新，并已同步 {} 的 {}",
                            path.display(),
                            items.join(" 与 ")
                        );
                    }
                    Err(e) => {
                        message = format!("默认模型已更新；同步 omo.jsonc 失败：{e}");
                    }
                }
            } else if let Some(reason) = skip_profile_reason {
                message = format!("默认模型已更新；未同步 model_profile：{reason}");
            }
            Ok(AgentActionResult {
                ok: true,
                message,
                view: Some(get_config(agent)?),
            })
        }
        ModelConfigAgent::OhMyPi => {
            let paths = family_paths(agent)?;
            let sp = paths
                .settings
                .as_ref()
                .ok_or_else(|| "Oh-My-Pi config.yml 路径缺失".to_string())?;
            let mut root = if sp.exists() {
                let raw = fs::read_to_string(sp)
                    .map_err(|e| format!("读取 {} 失败: {e}", sp.display()))?;
                parse_root_text(sp, &raw, true)?
            } else {
                json!({})
            };
            if !root.is_object() {
                root = json!({});
            }
            // payload.model / small_model: None=不改动；Some("")=删除；
            // 有值时原样写入（模型 id 可含 `/`），保留其它角色与顶层键。
            if payload.model.is_some() || payload.small_model.is_some() {
                let obj = root.as_object_mut().unwrap();
                if !obj
                    .get("modelRoles")
                    .map(|r| r.is_object())
                    .unwrap_or(false)
                {
                    obj.insert("modelRoles".into(), json!({}));
                }
                let roles = obj
                    .get_mut("modelRoles")
                    .and_then(|r| r.as_object_mut())
                    .unwrap();
                if let Some(ref m) = payload.model {
                    if m.is_empty() {
                        roles.remove("default");
                    } else {
                        roles.insert("default".into(), Value::String(m.clone()));
                    }
                }
                if let Some(ref f) = payload.small_model {
                    if f.is_empty() {
                        roles.remove("fast");
                    } else {
                        roles.insert("fast".into(), Value::String(f.clone()));
                    }
                }
            }
            atomic_write_secret(sp, &yaml_text(&root)?)?;
            Ok(AgentActionResult {
                ok: true,
                message: "默认模型已更新".into(),
                view: Some(get_config(agent)?),
            })
        }
        ModelConfigAgent::Opencode => unreachable!("opencode 不走 Pi 家族后端"),
    }
}

pub fn reveal_config(agent: ModelConfigAgent) -> Result<AgentActionResult, String> {
    let paths = family_paths(agent)?;
    if !paths.models.exists() {
        if let Some(parent) = paths.models.parent() {
            fs::create_dir_all(parent).ok();
            platform::open_path(parent).map_err(|e| format!("打开目录失败: {e}"))?;
            return Ok(AgentActionResult {
                ok: true,
                message: format!("已打开目录 {}", parent.to_string_lossy()),
                view: None,
            });
        }
        return Err("配置文件尚不存在".into());
    }
    platform::reveal_path(&paths.models).map_err(|e| format!("打开文件管理器失败: {e}"))?;
    Ok(AgentActionResult {
        ok: true,
        message: format!("已在文件管理器中显示 {}", paths.models.to_string_lossy()),
        view: None,
    })
}

/* ===== Tests ===== */

#[cfg(test)]
mod tests {
    use super::*;

    // 触碰 HOME 环境的测试串行化：使用跨模块共享锁（见 config::TEST_HOME_LOCK）。

    struct TempHome {
        path: PathBuf,
        prev_home: Option<std::ffi::OsString>,
        prev_path: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl TempHome {
        fn new(subdirs: &[&str]) -> Self {
            let guard = crate::config::lock_home_for_test();
            let path = std::env::temp_dir().join(format!(
                "agentbuddy-pifam-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            for d in subdirs {
                fs::create_dir_all(path.join(d)).unwrap();
            }
            let prev_home = std::env::var_os("HOME");
            std::env::set_var("HOME", &path);
            // get_config 会用 sniff::is_agent_installed(agent) 门控视图；
            // 在沙箱 PATH 里放假的 pi/omp 二进制，避免依赖开发机真实安装。
            let bin = path.join("bin");
            fs::create_dir_all(&bin).unwrap();
            fs::write(bin.join("pi"), "#!/bin/sh\nexit 0\n").unwrap();
            fs::write(bin.join("omp"), "#!/bin/sh\nexit 0\n").unwrap();
            fs::write(bin.join("omo"), "#!/bin/sh\nexit 0\n").unwrap();
            #[cfg(windows)]
            {
                fs::write(bin.join("pi.exe"), "").unwrap();
                fs::write(bin.join("omp.exe"), "").unwrap();
                fs::write(bin.join("omo.exe"), "").unwrap();
            }
            let prev_path = std::env::var_os("PATH");
            let new_path = std::env::join_paths(std::iter::once(bin.clone()).chain(
                std::env::split_paths(&prev_path.clone().unwrap_or_default()),
            ))
            .unwrap();
            std::env::set_var("PATH", &new_path);
            Self {
                path,
                prev_home,
                prev_path,
                _guard: guard,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            // 恢复原 HOME / PATH，避免污染进程内后续测试。
            match self.prev_home.take() {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            match self.prev_path.take() {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn pi_provider_model_upsert_roundtrip() {
        let h = TempHome::new(&[".pi/agent"]);
        let models = h.path.join(".pi/agent/models.json");
        fs::write(
            &models,
            r#"{"providers":{"deepseek":{"baseUrl":"https://api.deepseek.com","api":"openai-completions","apiKey":"$DEEPSEEK_API_KEY","models":[{"id":"deepseek-chat","name":"DeepSeek Chat","reasoning":false,"input":["text"],"contextWindow":64000,"maxTokens":8192,"extraKeep":{"a":1}}]}}}"#,
        )
        .unwrap();

        let view = get_config(ModelConfigAgent::Pi).unwrap();
        assert_eq!(view.agent, "pi");
        assert_eq!(view.providers.len(), 1);
        let p = &view.providers[0];
        assert_eq!(p.base_url.as_deref(), Some("https://api.deepseek.com"));
        assert!(p.has_api_key);
        assert_eq!(p.api_key_source, "config");
        let m = &p.models[0];
        assert_eq!(m.limit_context, Some(64000.0));
        assert_eq!(m.limit_output, Some(8192.0));
        assert!(m.extra_options.contains_key("extraKeep"));

        // Rename + update model
        upsert_model(
            ModelConfigAgent::Pi,
            UpsertModelPayload {
                provider_id: "deepseek".into(),
                id: "deepseek-v3".into(),
                previous_id: Some("deepseek-chat".into()),
                name: Some("DeepSeek V3".into()),
                limit_context: Some(128000.0),
                limit_input: None,
                limit_output: None,
                // Pi schema 不允许 pdf/audio/video；写入层必须收敛为 text + image。
                modalities_input: Some(vec!["text".into(), "image".into(), "pdf".into()]),
                modalities_output: None,
                reasoning: Some(true),
                tool_call: None,
                thinking_budget_tokens: None,
                reasoning_effort: None,
                text_verbosity: None,
                variants: None,
                extra_options: None,
                replace_extra_options: None,
            },
        )
        .unwrap();

        let raw: Value = serde_json::from_str(&fs::read_to_string(&models).unwrap()).unwrap();
        let m0 = &raw.pointer("/providers/deepseek/models/0").unwrap();
        assert_eq!(m0.get("id").and_then(|x| x.as_str()), Some("deepseek-v3"));
        assert_eq!(
            m0.get("contextWindow").and_then(|x| x.as_f64()),
            Some(128000.0)
        );
        assert_eq!(m0.get("input"), Some(&json!(["text", "image"])));
        // Unmodeled field preserved
        assert!(m0.pointer("/extraKeep/a").is_some());
        // apiKey untouched
        assert_eq!(
            raw.pointer("/providers/deepseek/apiKey")
                .and_then(|x| x.as_str()),
            Some("$DEEPSEEK_API_KEY")
        );
    }

    #[test]
    fn pi_auth_secret_and_defaults() {
        let h = TempHome::new(&[".pi/agent"]);
        upsert_provider(
            ModelConfigAgent::Pi,
            UpsertProviderPayload {
                id: "deepseek".into(),
                previous_id: None,
                name: None,
                package: None,
                api: Some("openai-completions".into()),
                base_url: Some("https://api.deepseek.com".into()),
                timeout: None,
                chunk_timeout: None,
                api_key: Some("sk-test".into()),
            },
        )
        .unwrap();

        let auth: Value =
            serde_json::from_str(&fs::read_to_string(h.path.join(".pi/agent/auth.json")).unwrap())
                .unwrap();
        assert_eq!(
            auth.pointer("/deepseek/type").and_then(|x| x.as_str()),
            Some("api_key")
        );
        assert_eq!(
            get_provider_secret(ModelConfigAgent::Pi, "deepseek".into()).unwrap(),
            "sk-test"
        );

        set_defaults(
            ModelConfigAgent::Pi,
            SetDefaultsPayload {
                model: Some("deepseek/deepseek-chat".into()),
                small_model: None,
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: None,
            },
        )
        .unwrap();
        let settings: Value = serde_json::from_str(
            &fs::read_to_string(h.path.join(".pi/agent/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            settings.get("defaultProvider").and_then(|x| x.as_str()),
            Some("deepseek")
        );
        assert_eq!(
            settings.get("defaultModel").and_then(|x| x.as_str()),
            Some("deepseek-chat")
        );

        let view = get_config(ModelConfigAgent::Pi).unwrap();
        assert_eq!(view.model.as_deref(), Some("deepseek/deepseek-chat"));

        // Clear secret removes auth entry
        set_provider_secret(ModelConfigAgent::Pi, "deepseek".into(), String::new()).unwrap();
        let auth: Value =
            serde_json::from_str(&fs::read_to_string(h.path.join(".pi/agent/auth.json")).unwrap())
                .unwrap();
        assert!(auth.get("deepseek").is_none());
    }

    #[cfg(unix)]
    fn assert_owner_only(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{} 应为 600 权限", path.display());
    }

    #[test]
    fn omp_model_upsert_roundtrip() {
        let h = TempHome::new(&[".omp/agent"]);
        let models = h.path.join(".omp/agent/models.yml");
        fs::write(
            &models,
            r#"
providers:
  magpie:
    api: anthropic-messages
    baseUrl: https://api.magpie.example
    apiKey: $MAGPIE_API_KEY
    compat:
      someFlag: true
    models:
      - id: magpie/opus
        name: Magpie Opus
        reasoning: true
        input: [text, image]
        contextWindow: 200000
        maxTokens: 8192
        extraKeep:
          a: 1
  openai-compat:
    api: openai-completions
    models:
      - id: gpt-5
"#,
        )
        .unwrap();

        let view = get_config(ModelConfigAgent::OhMyPi).unwrap();
        assert_eq!(view.agent, "oh-my-pi");
        assert!(view.defaults_supported);
        assert!(view.warnings.iter().any(|w| w.contains("YAML")));
        let providers: Vec<_> = view.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(providers, vec!["magpie", "openai-compat"]);
        let magpie = &view.providers[0];
        assert_eq!(magpie.api.as_deref(), Some("anthropic-messages"));
        assert_eq!(
            magpie.base_url.as_deref(),
            Some("https://api.magpie.example")
        );
        assert!(magpie.has_api_key);
        assert_eq!(magpie.api_key_source, "config");
        let m = &magpie.models[0];
        assert_eq!(m.limit_context, Some(200000.0));
        assert_eq!(m.limit_output, Some(8192.0));
        assert_eq!(m.modalities_input, vec!["text", "image"]);
        assert!(m.extra_options.contains_key("extraKeep"));

        // Rename + update：模型 id 可含 `/`，写入必须保留未知字段与 apiKey。
        upsert_model(
            ModelConfigAgent::OhMyPi,
            UpsertModelPayload {
                provider_id: "magpie".into(),
                id: "magpie-opus-4".into(),
                previous_id: Some("magpie/opus".into()),
                name: Some("Magpie Opus 4".into()),
                limit_context: Some(1000000.0),
                limit_input: None,
                limit_output: None,
                // OMP 与 Pi 一样只接受 text / text + image。
                modalities_input: Some(vec!["text".into(), "image".into(), "pdf".into()]),
                modalities_output: None,
                reasoning: Some(true),
                tool_call: None,
                thinking_budget_tokens: None,
                reasoning_effort: None,
                text_verbosity: None,
                variants: None,
                extra_options: None,
                replace_extra_options: None,
            },
        )
        .unwrap();

        let raw: Value = serde_yaml::from_str(&fs::read_to_string(&models).unwrap()).unwrap();
        let m0 = raw.pointer("/providers/magpie/models/0").unwrap();
        assert_eq!(m0.get("id").and_then(|x| x.as_str()), Some("magpie-opus-4"));
        assert_eq!(
            m0.get("contextWindow").and_then(|x| x.as_f64()),
            Some(1000000.0)
        );
        assert_eq!(m0.get("input"), Some(&json!(["text", "image"])));
        assert!(m0.pointer("/extraKeep/a").is_some());
        // provider 级未知字段与 apiKey 保留（YAML 重写不丢字段）。
        assert!(raw.pointer("/providers/magpie/compat/someFlag").is_some());
        assert_eq!(
            raw.pointer("/providers/magpie/apiKey")
                .and_then(|x| x.as_str()),
            Some("$MAGPIE_API_KEY")
        );
        #[cfg(unix)]
        assert_owner_only(&models);
    }

    #[test]
    fn omp_legacy_auth_migration_and_secret() {
        let h = TempHome::new(&[".omp/agent"]);
        let models = h.path.join(".omp/agent/models.yml");
        fs::write(
            &models,
            r#"
providers:
  alpha:
    api: anthropic-messages
    models:
      - id: alpha-1
  beta:
    auth: none
    models:
      - id: beta-1
  gamma:
    apiKey: $GAMMA_API_KEY
    models:
      - id: gamma-1
"#,
        )
        .unwrap();
        let auth_path = h.path.join(".omp/agent/auth.json");
        fs::write(
            &auth_path,
            r#"{"alpha":{"type":"api_key","key":"sk-alpha"},"beta":{"type":"api_key","key":"sk-beta"},"gamma":{"type":"api_key","key":"sk-gamma"}}"#,
        )
        .unwrap();

        let view = get_config(ModelConfigAgent::OhMyPi).unwrap();
        let alpha = view.providers.iter().find(|p| p.id == "alpha").unwrap();
        assert!(alpha.has_api_key);
        // 迁移后密钥归属 config。
        assert_eq!(alpha.api_key_source, "config");
        let beta = view.providers.iter().find(|p| p.id == "beta").unwrap();
        assert_eq!(beta.api_key_source, "auth");

        let raw: Value = serde_yaml::from_str(&fs::read_to_string(&models).unwrap()).unwrap();
        assert_eq!(
            raw.pointer("/providers/alpha/apiKey")
                .and_then(|x| x.as_str()),
            Some("sk-alpha")
        );
        assert!(raw.pointer("/providers/beta/apiKey").is_none());
        #[cfg(unix)]
        assert_owner_only(&models);

        let auth: Value = serde_json::from_str(&fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert!(auth.get("alpha").is_none());
        assert!(auth.get("beta").is_some());

        assert_eq!(
            get_provider_secret(ModelConfigAgent::OhMyPi, "alpha".into()).unwrap(),
            "sk-alpha"
        );

        // 写入密钥：落到 provider apiKey，并清理 auth.json 兼容条目。
        set_provider_secret(
            ModelConfigAgent::OhMyPi,
            "beta".into(),
            "sk-beta-new".to_string(),
        )
        .unwrap();
        let raw: Value = serde_yaml::from_str(&fs::read_to_string(&models).unwrap()).unwrap();
        assert_eq!(
            raw.pointer("/providers/beta/apiKey")
                .and_then(|x| x.as_str()),
            Some("sk-beta-new")
        );
        let auth: Value = serde_json::from_str(&fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert!(auth.get("beta").is_none());
        // 无关 provider 的旧条目保持不动。
        assert!(auth.get("gamma").is_some());
    }

    #[test]
    fn omp_defaults_model_roles() {
        let h = TempHome::new(&[".omp/agent"]);
        let models = h.path.join(".omp/agent/models.yml");
        fs::write(&models, "providers: {}\n").unwrap();
        let config = h.path.join(".omp/agent/config.yml");
        fs::write(
            &config,
            "modelRoles:\n  default: magpie/magpie-opus\n  fast: magpie/fast\ntheme: dark\n",
        )
        .unwrap();

        let view = get_config(ModelConfigAgent::OhMyPi).unwrap();
        assert_eq!(view.model.as_deref(), Some("magpie/magpie-opus"));
        assert_eq!(view.small_model.as_deref(), Some("magpie/fast"));
        assert!(view.small_model_supported);
        assert_eq!(view.small_model_key.as_deref(), Some("modelRoles.fast"));

        set_defaults(
            ModelConfigAgent::OhMyPi,
            SetDefaultsPayload {
                model: Some("magpie/magpie-opus-4".into()),
                small_model: Some("magpie/fast-2".into()),
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: None,
            },
        )
        .unwrap();
        let raw: Value = serde_yaml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            raw.pointer("/modelRoles/default").and_then(|x| x.as_str()),
            Some("magpie/magpie-opus-4")
        );
        assert_eq!(
            raw.pointer("/modelRoles/fast").and_then(|x| x.as_str()),
            Some("magpie/fast-2")
        );
        assert_eq!(raw.get("theme").and_then(|x| x.as_str()), Some("dark"));
        let view = get_config(ModelConfigAgent::OhMyPi).unwrap();
        assert_eq!(view.small_model.as_deref(), Some("magpie/fast-2"));

        set_defaults(
            ModelConfigAgent::OhMyPi,
            SetDefaultsPayload {
                model: None,
                small_model: Some(String::new()),
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: None,
            },
        )
        .unwrap();
        let raw: Value = serde_yaml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            raw.pointer("/modelRoles/default").and_then(|x| x.as_str()),
            Some("magpie/magpie-opus-4")
        );
        assert!(raw.pointer("/modelRoles/fast").is_none());
        assert_eq!(raw.get("theme").and_then(|x| x.as_str()), Some("dark"));

        set_defaults(
            ModelConfigAgent::OhMyPi,
            SetDefaultsPayload {
                model: Some(String::new()),
                small_model: None,
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: None,
            },
        )
        .unwrap();
        let raw: Value = serde_yaml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        assert!(raw.pointer("/modelRoles/default").is_none());
        assert!(raw.pointer("/modelRoles/fast").is_none());
        let view = get_config(ModelConfigAgent::OhMyPi).unwrap();
        assert!(view.model.is_none());
        assert!(view.small_model.is_none());
    }

    const OMO_FIXTURE: &str = r#"{
  // 注释用于验证读取时兼容 JSONC
  "[native]": {
    "model_profile": "magpie/opus",
    "theme": "dark",
    "categories": {
      "quick": { "model": "magpie/quick", "reasoning": "low" },
      "deep-low": { "model": "magpie/deep" }
    }
  }
}"#;

    #[test]
    fn omo_defaults_quick_category() {
        let h = TempHome::new(&[".omo/agent"]);
        let omo = h.path.join(".omo/omo.jsonc");
        fs::write(&omo, OMO_FIXTURE).unwrap();

        let view = get_config(ModelConfigAgent::Omo).unwrap();
        assert!(view.installed);
        assert!(view.small_model_supported);
        assert_eq!(view.small_model.as_deref(), Some("magpie/quick"));
        assert_eq!(
            view.small_model_key.as_deref(),
            Some("[native].categories.quick.model")
        );
        assert_eq!(view.omo_model_profile.as_deref(), Some("magpie/opus"));

        let res = set_defaults(
            ModelConfigAgent::Omo,
            SetDefaultsPayload {
                model: Some("magpie/opus-2".into()),
                small_model: Some("magpie/quick-2".into()),
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: Some(true),
            },
        )
        .unwrap();
        assert!(res.ok);
        assert!(res.message.contains("model_profile"));
        assert!(res.message.contains("categories.quick.model"));

        // 上游优先 settings.jsonc；两者都不存在时 pick_existing 也落到该文件名。
        let settings: Value =
            json5::from_str(&fs::read_to_string(h.path.join(".omo/agent/settings.jsonc")).unwrap())
                .unwrap();
        assert_eq!(
            settings.get("defaultProvider").and_then(Value::as_str),
            Some("magpie")
        );
        assert_eq!(
            settings.get("defaultModel").and_then(Value::as_str),
            Some("opus-2")
        );

        let raw: Value = json5::from_str(&fs::read_to_string(&omo).unwrap()).unwrap();
        assert_eq!(
            raw.pointer("/[native]/model_profile")
                .and_then(Value::as_str),
            Some("magpie/opus-2")
        );
        assert_eq!(
            raw.pointer("/[native]/categories/quick/model")
                .and_then(Value::as_str),
            Some("magpie/quick-2")
        );
        assert_eq!(
            raw.pointer("/[native]/categories/quick/reasoning")
                .and_then(Value::as_str),
            Some("low")
        );
        assert_eq!(
            raw.pointer("/[native]/categories/deep-low/model")
                .and_then(Value::as_str),
            Some("magpie/deep")
        );
        assert_eq!(
            raw.pointer("/[native]/theme").and_then(Value::as_str),
            Some("dark")
        );

        // 只清空 quick.model：保留同级字段与其它类别。
        set_defaults(
            ModelConfigAgent::Omo,
            SetDefaultsPayload {
                model: None,
                small_model: Some(String::new()),
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: Some(false),
            },
        )
        .unwrap();
        let raw: Value = json5::from_str(&fs::read_to_string(&omo).unwrap()).unwrap();
        assert!(raw.pointer("/[native]/categories/quick/model").is_none());
        assert_eq!(
            raw.pointer("/[native]/categories/quick/reasoning")
                .and_then(Value::as_str),
            Some("low")
        );
        assert_eq!(
            raw.pointer("/[native]/model_profile")
                .and_then(Value::as_str),
            Some("magpie/opus-2")
        );
        let view = get_config(ModelConfigAgent::Omo).unwrap();
        assert!(view.small_model.is_none());
    }

    #[test]
    fn omo_quick_prunes_empty_category() {
        let h = TempHome::new(&[".omo/agent"]);
        let omo = h.path.join(".omo/omo.jsonc");
        fs::write(
            &omo,
            r#"{ "[native]": { "categories": { "quick": { "model": "magpie/quick" } } } }"#,
        )
        .unwrap();

        set_defaults(
            ModelConfigAgent::Omo,
            SetDefaultsPayload {
                model: None,
                small_model: Some(String::new()),
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: None,
            },
        )
        .unwrap();

        let raw: Value = json5::from_str(&fs::read_to_string(&omo).unwrap()).unwrap();
        assert!(raw.pointer("/[native]/categories").is_none());
        assert!(raw.get("[native]").is_some());
    }

    #[test]
    fn omo_legacy_senpi_container() {
        let h = TempHome::new(&[".omo/agent"]);
        let omo = h.path.join(".omo/omo.jsonc");
        fs::write(
            &omo,
            r#"{ "[senpi]": { "categories": { "quick": { "model": "magpie/quick" } } } }"#,
        )
        .unwrap();

        let view = get_config(ModelConfigAgent::Omo).unwrap();
        assert_eq!(view.small_model.as_deref(), Some("magpie/quick"));
        assert_eq!(
            view.small_model_key.as_deref(),
            Some("[senpi].categories.quick.model")
        );

        set_defaults(
            ModelConfigAgent::Omo,
            SetDefaultsPayload {
                model: None,
                small_model: Some("magpie/quick-2".into()),
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: None,
            },
        )
        .unwrap();

        let raw: Value = json5::from_str(&fs::read_to_string(&omo).unwrap()).unwrap();
        assert!(raw.get("[native]").is_none());
        assert_eq!(
            raw.pointer("/[senpi]/categories/quick/model")
                .and_then(Value::as_str),
            Some("magpie/quick-2")
        );
    }

    #[test]
    fn omo_quick_creates_config_file() {
        let h = TempHome::new(&[".omo/agent"]);
        let res = set_defaults(
            ModelConfigAgent::Omo,
            SetDefaultsPayload {
                model: Some("magpie/opus".into()),
                small_model: Some("magpie/quick".into()),
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: None,
            },
        )
        .unwrap();
        assert!(res.ok);
        assert!(res.message.contains("categories.quick.model"));

        // 两个候选文件都不存在时，按空配置在标准路径创建并写入 quick。
        let omo = h.path.join(".omo/omo.jsonc");
        let raw: Value = json5::from_str(&fs::read_to_string(&omo).unwrap()).unwrap();
        assert_eq!(
            raw.pointer("/[native]/categories/quick/model")
                .and_then(Value::as_str),
            Some("magpie/quick")
        );

        let view = get_config(ModelConfigAgent::Omo).unwrap();
        assert_eq!(view.model.as_deref(), Some("magpie/opus"));
        assert_eq!(view.small_model.as_deref(), Some("magpie/quick"));
    }

    #[test]
    fn omo_aux_rejects_malformed_structures() {
        let h = TempHome::new(&[".omo/agent"]);
        let omo = h.path.join(".omo/omo.jsonc");
        fs::write(
            &omo,
            r#"{ "[native]": { "categories": { "quick": "magpie/quick" } } }"#,
        )
        .unwrap();

        let res = set_defaults(
            ModelConfigAgent::Omo,
            SetDefaultsPayload {
                model: None,
                small_model: Some("magpie/quick-2".into()),
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: None,
            },
        )
        .unwrap();
        assert!(res.message.contains("categories.quick 必须是对象"));
        let raw: Value = json5::from_str(&fs::read_to_string(&omo).unwrap()).unwrap();
        assert_eq!(
            raw.pointer("/[native]/categories/quick")
                .and_then(Value::as_str),
            Some("magpie/quick")
        );

        // harness 块本身不是对象时同样拒绝，不覆盖用户已有值。
        fs::write(&omo, r#"{ "[native]": "oops" }"#).unwrap();
        let res = set_defaults(
            ModelConfigAgent::Omo,
            SetDefaultsPayload {
                model: None,
                small_model: Some("magpie/quick".into()),
                enabled_providers: None,
                disabled_providers: None,
                sync_omo_model_profile: None,
            },
        )
        .unwrap();
        assert!(res.message.contains("[native] 必须是对象"));
        let raw: Value = json5::from_str(&fs::read_to_string(&omo).unwrap()).unwrap();
        assert_eq!(raw.get("[native]").and_then(Value::as_str), Some("oops"));
    }

    #[test]
    fn omp_auth_parse_failure_hides_providers() {
        let h = TempHome::new(&[".omp/agent"]);
        let models = h.path.join(".omp/agent/models.yml");
        fs::write(
            &models,
            "providers:\n  alpha:\n    api: anthropic-messages\n    models:\n      - id: alpha-1\n",
        )
        .unwrap();
        fs::write(h.path.join(".omp/agent/auth.json"), "not json").unwrap();

        let view = get_config(ModelConfigAgent::OhMyPi).unwrap();
        assert!(view.providers.is_empty());
        assert!(view.warnings.iter().any(|w| w.contains("迁移失败")));
    }
}
