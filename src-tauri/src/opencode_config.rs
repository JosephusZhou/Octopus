//! OpenCode 全局配置：供应商 / 模型可视化读写（通用模型配置框架的 OpenCode 后端）。
//!
//! - 配置：`~/.config/opencode/opencode.json(c)`（与 MCP 路径解析一致）
//! - 密钥：写入 `providers.<id>.settings.apiKey`；`~/.local/share/opencode/auth.json`
//!   仅作为旧配置的只读回退（OpenCode 2.x 不再读取该文件）
//! - 目录：Models.dev `https://models.dev/api.json`（精简后返回，磁盘 + 进程缓存）
//!
//! 写侧只产出 OpenCode 2.x 的原生 schema：
//! `providers.<id>.{name,package,settings,models}`、
//! `models.<id>.{limit,capabilities,settings,variants[]}`、
//! 顶层 `model` 字符串与第二槽位 `agents.title.model`。
//! 1.x 的 `provider` / `npm` / `options` / `modalities` 只在**读取**时兼容，
//! 某个 provider 被保存/删除时会把它的 1.x 残留整条清理掉，避免 `providers`
//! 覆盖 `provider` 导致的「保存成功但 OpenCode 不生效」。
//!
//! 列表 DTO **永不**回传明文 API Key；编辑时用 `get_provider_secret` 按需拉取。
//! Pi 后端见 `pi_model_config`，共用本模块的 DTO 与 IO helper。

use crate::platform;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA_URL: &str = "https://opencode.ai/config.json";
const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CATALOG_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// OpenCode 2.x 的供应商容器。
const PROVIDERS_KEY: &str = "providers";
/// OpenCode 1.x 的供应商容器；2.x 会在内存里把它迁移成 `providers`（`providers` 优先）。
const LEGACY_PROVIDERS_KEY: &str = "provider";
/// 第二模型槽位在 2.x 的落点：1.x 的 `small_model` 已废弃，等价于 title agent 的模型。
const TITLE_AGENT_MODEL_PATH: [&str; 3] = ["agents", "title", "model"];
/// 1.x 顶层第二模型槽位键，仅在读取旧配置时兼容。
const LEGACY_SMALL_MODEL_KEY: &str = "small_model";

/* ===== DTOs ===== */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelConfigView {
    /// agent 标识：opencode | pi
    pub agent: String,
    pub config_path: String,
    /// 凭据文件真实路径；前端展示时须脱敏。
    pub credential_path: String,
    pub config_exists: bool,
    pub is_jsonc: bool,
    /// Whether the agent App/CLI is installed (same rule as agent sniff).
    /// Config dir alone is **not** enough. UI treats this as the highest-priority gate.
    pub installed: bool,
    /// 该 agent 是否支持可视化编辑默认模型。
    pub defaults_supported: bool,
    /// 该 agent 是否支持可视化编辑第二模型槽位。
    pub small_model_supported: bool,
    pub model: Option<String>,
    /// 第二模型槽位的当前值；按 agent 映射到各自配置键。
    pub small_model: Option<String>,
    /// 第二模型槽位对应的配置键；无槽位时为 None。
    /// opencode=small_model，oh-my-pi=modelRoles.fast，
    /// omo=`<harness>.categories.quick.model`（harness 为 `[native]` / `native` / `[senpi]`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub small_model_key: Option<String>,
    /// OmO 专用：`omo.jsonc` 中固定 `model_profile` 的当前值（headless / desktop 会话会覆盖默认模型）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omo_model_profile: Option<String>,
    /// OmO 专用：`omo.jsonc` 的真实路径（确认提示展示用，属 b 类真实路径）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omo_profile_path: Option<String>,
    pub enabled_providers: Option<Vec<String>>,
    pub disabled_providers: Option<Vec<String>>,
    pub providers: Vec<AgentProviderView>,
    pub warnings: Vec<String>,
    /// OpenCode 专用：可一键迁移的 1.x 残留；None 表示该 Agent 不存在迁移场景
    /// （Pi 家族）或无需迁移。前端据此渲染「立即迁移」按钮。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_migration: Option<LegacyMigrationInfo>,
}

/// OpenCode 1.x → 2.x 的待迁移项（只用于提示，不含任何敏感数据）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationInfo {
    /// 仅存在于 1.x 顶层 `provider` 的供应商 id。
    pub providers: Vec<String>,
    /// 密钥只存在于旧版 `auth.json` 的供应商 id。
    pub auth_providers: Vec<String>,
}

impl LegacyMigrationInfo {
    fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.auth_providers.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderView {
    pub id: String,
    pub name: Option<String>,
    /// OpenCode 2.x `providers.<id>.package`（原样回传，可能带 `aisdk:` 前缀）。
    /// 1.x 旧配置里的 `npm` 会归一成 2.x 的 package 形式。Pi 家族恒为 None。
    pub package: Option<String>,
    /// 派生协议（`openai` / `anthropic`）：OpenCode 后端由 `package` 推导、**不落盘**；
    /// Pi 家族是落盘的 `api` 字段。同步供应商时按它选择对应侧的 Base URL。
    pub api: Option<String>,
    pub has_api_key: bool,
    /// "auth" | "config" | "both" | "none"
    pub api_key_source: String,
    pub base_url: Option<String>,
    pub set_cache_key: Option<bool>,
    pub timeout: Option<i64>,
    pub chunk_timeout: Option<i64>,
    pub whitelist: Vec<String>,
    pub blacklist: Vec<String>,
    pub models: Vec<AgentModelView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelView {
    pub id: String,
    pub name: Option<String>,
    pub limit_context: Option<f64>,
    pub limit_input: Option<f64>,
    pub limit_output: Option<f64>,
    pub modalities_input: Vec<String>,
    pub modalities_output: Vec<String>,
    pub reasoning: Option<bool>,
    pub tool_call: Option<bool>,
    pub attachment: Option<bool>,
    pub status: Option<String>,
    pub thinking_type: Option<String>,
    pub thinking_budget_tokens: Option<u64>,
    pub reasoning_effort: Option<String>,
    pub text_verbosity: Option<String>,
    pub variants: Vec<AgentVariantView>,
    /// 未建模的 options 字段（原样透传，序列化为 JSON 对象）。
    pub extra_options: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// 模型变体：对应 2.x `models.<id>.variants[].{id,settings,headers,body}`。
/// `settings` 里除 `reasoningEffort` 外的键进 `extra`，变体顶层未建模键（headers/body 等）进
/// `overlay`，两者都原样往返。1.x 的对象写法只在读取时兼容（2.x 会整条丢弃该 provider）。
pub struct AgentVariantView {
    pub id: String,
    pub reasoning_effort: Option<String>,
    pub extra: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub overlay: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogReasoningOption {
    #[serde(rename = "type")]
    pub r#type: String,
    pub values: Option<Vec<String>>,
    pub min: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsDevCatalog {
    pub fetched_at: u64,
    pub from_cache: bool,
    pub providers: Vec<CatalogProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogProvider {
    pub id: String,
    pub name: String,
    pub env: Vec<String>,
    pub npm: Option<String>,
    pub models: Vec<CatalogModelSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModelSummary {
    pub id: String,
    pub name: String,
    pub limit_context: Option<f64>,
    pub limit_input: Option<f64>,
    pub limit_output: Option<f64>,
    pub modalities_input: Vec<String>,
    pub modalities_output: Vec<String>,
    pub reasoning: bool,
    pub reasoning_options: Vec<CatalogReasoningOption>,
    pub tool_call: bool,
    pub attachment: bool,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActionResult {
    pub ok: bool,
    pub message: String,
    pub view: Option<AgentModelConfigView>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDefaultsPayload {
    /// None = 不改动；Some("") = 删除键；Some("provider/model") = 写入
    pub model: Option<String>,
    pub small_model: Option<String>,
    pub enabled_providers: Option<Option<Vec<String>>>,
    pub disabled_providers: Option<Option<Vec<String>>>,
    /// OmO 专用：是否把本次默认模型同步到 `~/.omo/omo.jsonc` 的 `model_profile`；
    /// 其它 agent 忽略该字段。
    #[serde(default)]
    pub sync_omo_model_profile: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertProviderPayload {
    /// 新建时的 id；编辑时也传当前 id
    pub id: String,
    /// 若重命名 provider key，传旧 id（仅当与 id 不同）
    pub previous_id: Option<String>,
    pub name: Option<String>,
    /// OpenCode 2.x 的 `package`；空串表示删除该键。
    pub package: Option<String>,
    /// Pi 家族的 API 格式；OpenCode 后端忽略（协议由 package 推导）。
    pub api: Option<String>,
    pub base_url: Option<String>,
    pub timeout: Option<i64>,
    pub chunk_timeout: Option<i64>,
    /// 三态：None=不改动，Some("")=清除，Some(v)=写入 `providers.<id>.settings.apiKey`
    /// 并清理旧版 `auth.json` 条目
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertModelPayload {
    pub provider_id: String,
    pub id: String,
    pub previous_id: Option<String>,
    pub name: Option<String>,
    pub limit_context: Option<f64>,
    pub limit_input: Option<f64>,
    pub limit_output: Option<f64>,
    pub modalities_input: Option<Vec<String>>,
    pub modalities_output: Option<Vec<String>>,
    pub reasoning: Option<bool>,
    pub tool_call: Option<bool>,
    pub thinking_budget_tokens: Option<u64>,
    pub reasoning_effort: Option<String>,
    pub text_verbosity: Option<String>,
    pub variants: Option<Vec<AgentVariantView>>,
    /// 合并进 options 的额外字段；传空对象表示不改 extra
    pub extra_options: Option<Map<String, Value>>,
    /// true 时用 extra_options 整体替换 options 中未知字段（删除未列出的）
    pub replace_extra_options: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeModelsResult {
    pub ok: bool,
    pub message: String,
    pub model_ids: Vec<String>,
}

/* ===== Paths ===== */

fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "无法解析用户主目录".to_string())
}

fn config_path() -> Result<(PathBuf, bool), String> {
    // Prefer existing resolution used by MCP (jsonc over json).
    let path = crate::mcp_config::resolve_mcp_path("opencode")?;
    let is_jsonc = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("jsonc"))
        .unwrap_or(false);
    Ok((path, is_jsonc))
}

fn default_config_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".config/opencode/opencode.json"))
}

fn auth_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".local/share/opencode/auth.json"))
}

pub(crate) fn display_path(path: &Path) -> String {
    platform::display_path(&path.to_string_lossy())
}

/* ===== IO helpers ===== */

pub(crate) fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("无效路径: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    use std::sync::atomic::{AtomicU64, Ordering};
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = parent.join(format!(
        ".{}.agentbuddy-{}-{}.tmp",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("tmp"),
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let mut f = fs::File::create(&tmp).map_err(|e| format!("创建临时文件失败: {e}"))?;
        f.write_all(content.as_bytes())
            .map_err(|e| format!("写入临时文件失败: {e}"))?;
        f.sync_all().ok();
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let direct = fs::write(path, content);
            let _ = fs::remove_file(&tmp);
            direct.map_err(|e2| format!("写入失败: {e} / {e2}"))
        }
    }
}

pub(crate) fn atomic_write_secret(path: &Path, content: &str) -> Result<(), String> {
    atomic_write(path, content)?;
    platform::set_owner_only_file(path);
    Ok(())
}

pub(crate) fn read_json_value(path: &Path) -> Result<Value, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    json5::from_str(&raw).map_err(|e| format!("解析 {} 失败: {e}", path.display()))
}

fn write_json_value(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| format!("序列化失败: {e}"))?;
    let text = format!("{text}\n");
    atomic_write(path, &text)
}

fn ensure_schema(root: &mut Value) {
    let obj = root.as_object_mut().expect("root must be object");
    if !obj.contains_key("$schema") {
        obj.insert("$schema".into(), Value::String(SCHEMA_URL.into()));
    }
}

fn load_or_empty_config() -> Result<(PathBuf, bool, Value, bool), String> {
    let (resolved, is_jsonc) = config_path()?;
    if resolved.exists() {
        let v = read_json_value(&resolved)?;
        let root = match v {
            Value::Object(_) => v,
            _ => return Err("OpenCode 配置根节点必须是对象".into()),
        };
        return Ok((resolved, is_jsonc, root, true));
    }
    // Prefer creating plain json when nothing exists.
    let path = default_config_path()?;
    Ok((path, false, json!({}), false))
}

fn load_auth() -> Result<Map<String, Value>, String> {
    load_auth_file(&auth_path()?)
}

fn save_auth(map: &Map<String, Value>) -> Result<(), String> {
    save_auth_file(&auth_path()?, map)
}

/// 读取任意 auth.json：`{ providerId: { type, key } }`；不存在时返回空映射。
pub(crate) fn load_auth_file(path: &Path) -> Result<Map<String, Value>, String> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let v = read_json_value(path)?;
    Ok(v.as_object().cloned().unwrap_or_default())
}

pub(crate) fn save_auth_file(path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建 auth 目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(map.clone()))
        .map_err(|e| format!("序列化 auth 失败: {e}"))?;
    atomic_write_secret(path, &format!("{text}\n"))
}

pub(crate) fn auth_has_key(auth: &Map<String, Value>, provider_id: &str) -> bool {
    auth.get(provider_id)
        .and_then(|v| v.get("key"))
        .and_then(|k| k.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

pub(crate) fn auth_get_key(auth: &Map<String, Value>, provider_id: &str) -> Option<String> {
    auth.get(provider_id)
        .and_then(|v| v.get("key"))
        .and_then(|k| k.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// provider 的 2.x `settings` 对象。
fn settings_of(provider: &Value) -> Option<&Map<String, Value>> {
    provider.get("settings").and_then(|s| s.as_object())
}

/// provider 的 1.x `options` 对象（仅读取兼容）。
fn legacy_options_of(provider: &Value) -> Option<&Map<String, Value>> {
    provider.get("options").and_then(|o| o.as_object())
}

fn map_str<'a>(map: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a str> {
    map.and_then(|m| m.get(key))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// 配置内明文密钥：2.x `settings.apiKey` 优先，1.x `options.apiKey` 回退。
fn config_get_key(provider: &Value) -> Option<String> {
    map_str(settings_of(provider), "apiKey")
        .or_else(|| map_str(legacy_options_of(provider), "apiKey"))
        .map(|s| s.to_string())
}

fn config_has_key(provider: &Value) -> bool {
    config_get_key(provider).is_some()
}

pub(crate) fn api_key_source(in_auth: bool, in_config: bool) -> String {
    match (in_auth, in_config) {
        (true, true) => "both".into(),
        (true, false) => "auth".into(),
        (false, true) => "config".into(),
        (false, false) => "none".into(),
    }
}

/// 把 1.x `npm` 包名归一成 2.x `package`：只有 AI SDK 包（`@ai-sdk/*`）需要补
/// `aisdk:` 前缀，否则 2.x 会落到原生 provider loader，配置看着正常、请求阶段才失败。
/// 其它包（`@opencode/ai/providers/*`、已带前缀的原样）不动。
fn normalize_package(raw: &str) -> String {
    let value = raw.trim();
    if value.is_empty() {
        return String::new();
    }
    if value.contains(':') {
        return value.to_string();
    }
    if value.starts_with("@ai-sdk/") {
        return format!("aisdk:{value}");
    }
    value.to_string()
}

/// 由 `package` 推导协议；`legacy_api` 只在找不到 package 时兼容 1.x 的 `api` 字段。
/// 注意：1.x 官方口径里 `api` 是 Base URL，Octopus 历史上只在其中存过协议名。
fn protocol_of(package: Option<&str>, legacy_api: Option<&str>) -> Option<String> {
    if let Some(pkg) = package.filter(|p| !p.is_empty()) {
        return Some(if pkg.contains("anthropic") {
            "anthropic".to_string()
        } else {
            "openai".to_string()
        });
    }
    let api = legacy_api?.trim();
    if api.is_empty() || api.contains("://") || api.contains('/') {
        return None;
    }
    Some(api.to_string())
}

/* ===== Parse view ===== */

pub(crate) fn str_opt(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

pub(crate) fn string_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|i| i.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn f64_opt(v: &Value, key: &str) -> Option<f64> {
    v.get(key)
        .and_then(|x| x.as_f64())
        .or_else(|| v.get(key).and_then(|x| x.as_i64()).map(|n| n as f64))
}

pub(crate) fn bool_opt(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(|x| x.as_bool())
}

fn i64_timeout_opt(options: &Value, key: &str) -> Option<i64> {
    match options.get(key) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::Bool(false)) => Some(-1), // sentinel: disabled
        _ => None,
    }
}

fn parse_model(id: &str, raw: &Value) -> AgentModelView {
    let limit = raw.get("limit").cloned().unwrap_or(Value::Null);
    // 2.x 能力：capabilities.{input,output,tools,reasoning}；1.x 的 modalities / tool_call 仅读取兼容。
    let capabilities = raw.get("capabilities").cloned().unwrap_or(Value::Null);
    let legacy_modalities = raw.get("modalities").cloned().unwrap_or(Value::Null);
    // 2.x 模型请求参数在 settings；1.x 的 options 仅读取兼容。
    let settings = raw
        .get("settings")
        .and_then(|s| s.as_object())
        .cloned()
        .unwrap_or_default();
    let legacy_options = raw
        .get("options")
        .and_then(|o| o.as_object())
        .cloned()
        .unwrap_or_default();

    let mut modalities_input = string_list(&capabilities, "input");
    if modalities_input.is_empty() {
        modalities_input = string_list(&legacy_modalities, "input");
    }
    let mut modalities_output = string_list(&capabilities, "output");
    if modalities_output.is_empty() {
        modalities_output = string_list(&legacy_modalities, "output");
    }

    // 2.x 的 reasoning 是对象（`{supported,toggle?,effort?,budget?}`）；
    // 1.x 的布尔写法只用于展示回填，写回一律归一成对象。
    let reasoning_raw = capabilities.get("reasoning");
    let reasoning = match reasoning_raw {
        Some(Value::Bool(b)) => Some(*b),
        Some(v) => bool_opt(v, "supported"),
        None => bool_opt(raw, "reasoning"),
    };
    let thinking_budget_tokens = reasoning_raw
        .and_then(|r| r.get("budget"))
        .and_then(|b| b.get("max"))
        .and_then(|b| b.as_u64())
        .or_else(|| {
            raw.pointer("/options/thinking/budgetTokens")
                .and_then(|b| b.as_u64())
        });
    let thinking_type = raw
        .pointer("/options/thinking/type")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());

    let reasoning_effort = map_str(Some(&settings), "reasoningEffort")
        .or_else(|| map_str(Some(&legacy_options), "reasoningEffort"))
        .map(|s| s.to_string());
    let text_verbosity = map_str(Some(&settings), "textVerbosity")
        .or_else(|| map_str(Some(&legacy_options), "textVerbosity"))
        .map(|s| s.to_string());

    // 已建模的 settings 键之外的字段原样进 extra_options，保存时再原样写回 settings。
    let known = ["reasoningEffort", "textVerbosity"];
    let mut extra_options = Map::new();
    for (k, v) in &settings {
        if !known.contains(&k.as_str()) {
            extra_options.insert(k.clone(), v.clone());
        }
    }
    if settings.is_empty() {
        for (k, v) in &legacy_options {
            if k != "thinking" && !known.contains(&k.as_str()) {
                extra_options.insert(k.clone(), v.clone());
            }
        }
    }

    AgentModelView {
        id: id.to_string(),
        name: str_opt(raw, "name"),
        limit_context: f64_opt(&limit, "context"),
        limit_input: f64_opt(&limit, "input"),
        limit_output: f64_opt(&limit, "output"),
        modalities_input,
        modalities_output,
        reasoning,
        tool_call: bool_opt(&capabilities, "tools")
            .or_else(|| bool_opt(raw, "tool_call"))
            .or_else(|| bool_opt(raw, "toolCall")),
        // 2.x 没有这两个字段：attachment 由 capabilities.input 表达，status 不在配置 schema 内。
        attachment: None,
        status: None,
        thinking_type,
        thinking_budget_tokens,
        reasoning_effort,
        text_verbosity,
        variants: parse_variants(raw),
        extra_options,
    }
}

/// 2.x 的 variants 是数组（`[{id,settings,headers?,body?}]`）；1.x 的对象写法只读取兼容。
fn parse_variants(raw: &Value) -> Vec<AgentVariantView> {
    let split = |settings: Option<&Map<String, Value>>| -> (Option<String>, Map<String, Value>) {
        let mut extra = Map::new();
        let mut reasoning_effort = None;
        if let Some(map) = settings {
            for (k, v) in map {
                if k == "reasoningEffort" {
                    reasoning_effort = v.as_str().map(|s| s.to_string());
                } else {
                    extra.insert(k.clone(), v.clone());
                }
            }
        }
        (reasoning_effort, extra)
    };

    match raw.get("variants") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|item| {
                let id = item.get("id")?.as_str()?.to_string();
                let (reasoning_effort, extra) =
                    split(item.get("settings").and_then(|s| s.as_object()));
                let mut overlay = Map::new();
                if let Some(obj) = item.as_object() {
                    for (k, v) in obj {
                        if k != "id" && k != "settings" {
                            overlay.insert(k.clone(), v.clone());
                        }
                    }
                }
                Some(AgentVariantView {
                    id,
                    reasoning_effort,
                    extra,
                    overlay,
                })
            })
            .collect(),
        Some(Value::Object(obj)) => obj
            .iter()
            .map(|(vid, vv)| {
                let (reasoning_effort, extra) = split(vv.as_object());
                AgentVariantView {
                    id: vid.clone(),
                    reasoning_effort,
                    extra,
                    overlay: Map::new(),
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_provider(id: &str, raw: &Value, auth: &Map<String, Value>) -> AgentProviderView {
    let settings = settings_of(raw).cloned().unwrap_or_default();
    let legacy_options = legacy_options_of(raw).cloned().unwrap_or_default();
    let in_auth = auth_has_key(auth, id);
    let in_config = config_has_key(raw);

    let package = str_opt(raw, "package").or_else(|| {
        raw.get("npm")
            .and_then(|n| n.as_str())
            .filter(|n| !n.is_empty())
            .map(normalize_package)
    });

    // baseURL：2.x settings 优先；1.x options.baseURL 次之；1.x 官方写法的 api
    // 只有在看起来像 URL 时才当作 Base URL（Octopus 历史上用它存协议名）。
    let base_url = map_str(Some(&settings), "baseURL")
        .or_else(|| map_str(Some(&settings), "baseUrl"))
        .or_else(|| map_str(Some(&legacy_options), "baseURL"))
        .or_else(|| map_str(Some(&legacy_options), "baseUrl"))
        .or_else(|| {
            raw.get("api")
                .and_then(|a| a.as_str())
                .filter(|a| a.contains("://"))
        })
        .map(|s| s.to_string());

    let timeout = i64_timeout_opt(&Value::Object(settings.clone()), "timeout")
        .or_else(|| i64_timeout_opt(&Value::Object(legacy_options.clone()), "timeout"));
    let chunk_timeout = i64_timeout_opt(&Value::Object(settings), "chunkTimeout")
        .or_else(|| i64_timeout_opt(&Value::Object(legacy_options.clone()), "chunkTimeout"));

    let models = raw
        .get("models")
        .and_then(|m| m.as_object())
        .map(|obj| {
            let mut list: Vec<_> = obj.iter().map(|(mid, mv)| parse_model(mid, mv)).collect();
            list.sort_by(|a, b| a.id.cmp(&b.id));
            list
        })
        .unwrap_or_default();

    AgentProviderView {
        id: id.to_string(),
        name: str_opt(raw, "name"),
        api: protocol_of(package.as_deref(), raw.get("api").and_then(|a| a.as_str())),
        package,
        has_api_key: in_auth || in_config,
        api_key_source: api_key_source(in_auth, in_config),
        base_url,
        set_cache_key: legacy_options.get("setCacheKey").and_then(|x| x.as_bool()),
        timeout,
        chunk_timeout,
        whitelist: string_list(raw, "whitelist"),
        blacklist: string_list(raw, "blacklist"),
        models,
    }
}

fn top_string_list(root: &Value, key: &str) -> Option<Vec<String>> {
    root.get(key).and_then(|v| {
        if v.is_null() {
            return None;
        }
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|i| i.as_str().map(|s| s.to_string()))
                .collect()
        })
    })
}

/// 读取模型引用（`model` / `agents.title.model`）：2.x 接受 `"provider/model#variant"`
/// 字符串或 `{providerID,model,variant?}` 对象，1.x 只有字符串。统一回转成展示用的
/// `provider/model`（带 variant 时拼 `#variant`）。
fn read_model_ref(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(s) = value.as_str() {
        return Some(s.to_string()).filter(|s| !s.is_empty());
    }
    let provider = value.get("providerID").and_then(|v| v.as_str())?;
    let model = value.get("model").and_then(|v| v.as_str())?;
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let variant = value
        .get("variant")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty());
    Some(match variant {
        Some(v) => format!("{provider}/{model}#{v}"),
        None => format!("{provider}/{model}"),
    })
}

/// 第二模型槽位当前值：2.x 读 `agents.title.model`，旧配置回退顶层 `small_model`。
fn read_title_agent_model(root: &Value) -> Option<String> {
    read_model_ref(
        root.pointer("/agents/title/model")
            .or_else(|| root.get(LEGACY_SMALL_MODEL_KEY)),
    )
}

/// 合并 1.x `provider` 与 2.x `providers`：2.x 同名条目优先（与 OpenCode 2.x 行为一致）。
/// 返回（按 id 排序的视图列表，仅存在于 1.x 的 id）。
fn collect_providers(
    root: &Value,
    auth: &Map<String, Value>,
) -> (Vec<AgentProviderView>, Vec<String>) {
    let mut merged: std::collections::BTreeMap<String, AgentProviderView> =
        std::collections::BTreeMap::new();
    let mut legacy_only: Vec<String> = Vec::new();

    for key in [LEGACY_PROVIDERS_KEY, PROVIDERS_KEY] {
        let Some(obj) = root.get(key).and_then(|v| v.as_object()) else {
            continue;
        };
        for (id, raw) in obj {
            if key == PROVIDERS_KEY {
                merged.insert(id.clone(), parse_provider(id, raw, auth));
                legacy_only.retain(|existing| existing != id);
            } else {
                merged.entry(id.clone()).or_insert_with(|| {
                    legacy_only.push(id.clone());
                    parse_provider(id, raw, auth)
                });
            }
        }
    }

    let providers = merged.into_values().collect();
    (providers, legacy_only)
}

/* ===== Public API ===== */

pub fn get_config() -> Result<AgentModelConfigView, String> {
    // Highest priority for the UI: is OpenCode App/CLI actually installed?
    // Config dir alone (e.g. leftover ~/.config/opencode) is not enough —
    // same rule as agent sniff (`found` requires install paths).
    let installed = crate::sniff::is_agent_installed("opencode");

    let (path, is_jsonc, root, exists) = load_or_empty_config()?;
    let credential_path = auth_path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let auth = load_auth().unwrap_or_default();
    let mut warnings = Vec::new();
    if exists && is_jsonc {
        warnings.push("当前配置为 JSONC；保存后注释与部分格式会被规范化为标准 JSON。".into());
    }

    // Not installed → still return path metadata so empty-state copy can mention it,
    // but never surface provider inventory (UI shows not-installed empty state).
    if !installed {
        return Ok(AgentModelConfigView {
            agent: "opencode".into(),
            config_path: display_path(&path),
            credential_path,
            config_exists: exists,
            is_jsonc,
            installed: false,
            defaults_supported: true,
            small_model_supported: true,
            model: None,
            small_model: None,
            small_model_key: Some("agents.title.model".into()),
            omo_model_profile: None,
            omo_profile_path: None,
            enabled_providers: None,
            disabled_providers: None,
            providers: Vec::new(),
            warnings: Vec::new(),
            legacy_migration: None,
        });
    }

    let (providers, legacy_only) = collect_providers(&root, &auth);
    // 密钥只存在于旧版 auth.json 的供应商（2.x 不再读取该文件，需要一键迁移搬进配置）。
    let auth_only: Vec<String> = providers
        .iter()
        .filter(|p| p.api_key_source == "auth")
        .map(|p| p.id.clone())
        .collect();
    if !legacy_only.is_empty() {
        warnings.push(format!(
            "检测到 {} 个 OpenCode 1.x 供应商配置（顶层 `provider`）；OpenCode 2.x 以 `providers` 为准，\
             保存或删除这些供应商时会连模型一起迁移到 `providers` 并清理旧键。",
            legacy_only.len()
        ));
    }
    // 只有 auth.json 有密钥（2.x 读不到）与两边都有密钥（可清理）分开报，避免把两种情况说成一种。
    if !auth_only.is_empty() {
        warnings.push(format!(
            "OpenCode 2.x 不再读取 auth.json（{} 个供应商的密钥只存在于该文件）；\
             点击「迁移」或保存对应供应商时会把密钥写入 `providers.<id>.settings.apiKey`。",
            auth_only.len()
        ));
    }
    let dual_auth = providers
        .iter()
        .filter(|p| p.api_key_source == "both")
        .count();
    if dual_auth > 0 {
        warnings.push(format!(
            "auth.json 里还有 {dual_auth} 个供应商的旧密钥条目（配置文件里已有一份）；\
             迁移或保存对应供应商时会清理旧条目。"
        ));
    }

    Ok(AgentModelConfigView {
        agent: "opencode".into(),
        config_path: display_path(&path),
        credential_path,
        config_exists: exists,
        is_jsonc,
        installed: true,
        defaults_supported: true,
        small_model_supported: true,
        model: read_model_ref(root.get("model")),
        small_model: read_title_agent_model(&root),
        small_model_key: Some("agents.title.model".into()),
        omo_model_profile: None,
        omo_profile_path: None,
        enabled_providers: top_string_list(&root, "enabled_providers"),
        disabled_providers: top_string_list(&root, "disabled_providers"),
        providers,
        warnings,
        legacy_migration: migration_info(legacy_only, auth_only),
    })
}

/// 组装待迁移提示：两项都为空时返回 None（前端据此隐藏「迁移」按钮）。
fn migration_info(
    providers: Vec<String>,
    auth_providers: Vec<String>,
) -> Option<LegacyMigrationInfo> {
    let info = LegacyMigrationInfo {
        providers,
        auth_providers,
    };
    (!info.is_empty()).then_some(info)
}

fn apply_optional_string_key(root: &mut Value, key: &str, value: &Option<String>) {
    if let Some(v) = value {
        let obj = root.as_object_mut().unwrap();
        if v.is_empty() {
            obj.remove(key);
        } else {
            obj.insert(key.to_string(), Value::String(v.clone()));
        }
    }
}

fn apply_optional_list_key(root: &mut Value, key: &str, value: &Option<Option<Vec<String>>>) {
    // Some(None) = clear; Some(Some(list)) = set; None = no change
    if let Some(inner) = value {
        let obj = root.as_object_mut().unwrap();
        match inner {
            None => {
                obj.remove(key);
            }
            Some(list) => {
                obj.insert(
                    key.to_string(),
                    Value::Array(list.iter().map(|s| Value::String(s.clone())).collect()),
                );
            }
        }
    }
}

pub fn set_defaults(payload: SetDefaultsPayload) -> Result<AgentActionResult, String> {
    let (path, _, mut root, _) = load_or_empty_config()?;
    if !root.is_object() {
        root = json!({});
    }
    ensure_schema(&mut root);
    apply_optional_string_key(&mut root, "model", &payload.model);
    if let Some(ref small) = payload.small_model {
        // 第二槽位在 2.x 是 `agents.title.model`；`small_model` 已被废弃，
        // 同时清掉旧键，避免 2.x 的兼容迁移把它重新注回去。
        if small.is_empty() {
            remove_title_agent_model(&mut root);
        } else {
            nested_object_mut(&mut root, &TITLE_AGENT_MODEL_PATH[..2])?.insert(
                TITLE_AGENT_MODEL_PATH[2].into(),
                Value::String(small.clone()),
            );
        }
        root.as_object_mut().unwrap().remove(LEGACY_SMALL_MODEL_KEY);
    }
    apply_optional_list_key(&mut root, "enabled_providers", &payload.enabled_providers);
    apply_optional_list_key(&mut root, "disabled_providers", &payload.disabled_providers);
    write_json_value(&path, &root)?;
    Ok(AgentActionResult {
        ok: true,
        message: "默认模型已更新".into(),
        view: Some(get_config()?),
    })
}

/// 逐层取（必要时创建）嵌套对象，返回最深一层的可变对象。
fn nested_object_mut<'a>(
    root: &'a mut Value,
    path: &[&str],
) -> Result<&'a mut Map<String, Value>, String> {
    let mut current = root;
    for key in path {
        let obj = current
            .as_object_mut()
            .ok_or_else(|| "配置根节点必须是对象".to_string())?;
        let entry = obj
            .entry((*key).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        current = entry;
    }
    current
        .as_object_mut()
        .ok_or_else(|| "节点必须是对象".to_string())
}

/// 删除 `agents.title.model`，并把变空的 `title` / `agents` 一并清掉。
fn remove_title_agent_model(root: &mut Value) {
    let agents_obj = root
        .get_mut("agents")
        .and_then(|a| a.as_object_mut())
        .and_then(|a| {
            if let Some(title) = a.get_mut("title").and_then(|t| t.as_object_mut()) {
                title.remove(TITLE_AGENT_MODEL_PATH[2]);
                if title.is_empty() {
                    a.remove("title");
                }
            }
            if a.is_empty() {
                None
            } else {
                Some(())
            }
        });
    if agents_obj.is_none() {
        root.as_object_mut().unwrap().remove("agents");
    }
}

/// `providers` 容器（按需创建）。
fn providers_object_mut(root: &mut Value) -> Result<&mut Map<String, Value>, String> {
    nested_object_mut(root, &[PROVIDERS_KEY])
}

/// 2.x `providers.<id>`（按需创建）。
fn provider_object_mut<'a>(
    root: &'a mut Value,
    id: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    let map = providers_object_mut(root)?;
    let entry = map
        .entry(id.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    entry
        .as_object_mut()
        .ok_or_else(|| format!("{PROVIDERS_KEY}.{id} 必须是对象"))
}

/// 2.x `providers.<id>.settings`（按需创建）。
fn ensure_settings_mut(provider: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let settings = provider
        .entry("settings".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !settings.is_object() {
        *settings = Value::Object(Map::new());
    }
    settings.as_object_mut().unwrap()
}

fn take_from_container(root: &mut Value, container: &str, id: &str) -> Option<Value> {
    root.get_mut(container)
        .and_then(|p| p.as_object_mut())
        .and_then(|m| m.remove(id))
}

/// 清掉变空的容器（`"provider": {}` / `"providers": {}` 这类噪音）。
fn drop_empty_container(root: &mut Value, container: &str) {
    let empty = root
        .get(container)
        .and_then(|p| p.as_object())
        .is_some_and(|m| m.is_empty());
    if empty {
        if let Some(obj) = root.as_object_mut() {
            obj.remove(container);
        }
    }
}

/// 把 1.x `provider.<id>` 的模型表按 2.x schema 写入 `providers.<id>.models`。
/// 保存与一键迁移共用，保证两处转换规则一致。
fn apply_legacy_models(
    p: &mut Map<String, Value>,
    provider_id: &str,
    view: &AgentProviderView,
    raw_models: &Map<String, Value>,
) -> Result<(), String> {
    for model in &view.models {
        let raw_model = raw_models.get(&model.id).cloned().unwrap_or(Value::Null);
        let payload = legacy_model_payload(provider_id, model, &raw_model);
        let models = p
            .entry("models".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let map = models
            .as_object_mut()
            .ok_or_else(|| format!("{PROVIDERS_KEY}.{provider_id}.models 必须是对象"))?;
        let entry = map
            .entry(model.id.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let m = entry
            .as_object_mut()
            .ok_or_else(|| format!("模型 `{provider_id}/{}` 必须是对象", model.id))?;
        apply_model_payload(m, &payload);
    }
    Ok(())
}

/// 收编 1.x `provider.<id>`（保存供应商 / 模型、写密钥、迁移都走这里）：
/// - `providers.<id>` 已存在：以 2.x 为准，只把 1.x 里唯一的密钥接续过去，再丢弃残留；
/// - `providers.<id>` 不存在：按 2.x schema 重建（含全部模型）后再删除旧键。
///
/// 关键点：旧条目里带着整张 `models` 表，直接删会让用户「保存一次就丢模型」，
/// 所以必须先转换再删。返回是否把 1.x 的明文密钥写进了配置文件。
fn adopt_legacy_provider(
    root: &mut Value,
    id: &str,
    auth: &Map<String, Value>,
) -> Result<bool, String> {
    let Some(raw) = take_from_container(root, LEGACY_PROVIDERS_KEY, id) else {
        return Ok(false);
    };
    // 1.x 的密钥在 `options.apiKey`（2.x 的 settings 已在 config_get_key 里优先）
    let legacy_key = config_get_key(&raw);
    let mut wrote_secret = false;

    if root.get(PROVIDERS_KEY).and_then(|p| p.get(id)).is_some() {
        // 2.x 条目优先，但 1.x 里唯一的密钥不能静默丢掉
        if let Some(key) = legacy_key {
            let p = provider_object_mut(root, id)?;
            let existing = map_str(p.get("settings").and_then(|s| s.as_object()), "apiKey");
            if existing.is_none() {
                ensure_settings_mut(p).insert("apiKey".into(), Value::String(key));
                wrote_secret = true;
            }
        }
    } else {
        let view = parse_provider(id, &raw, auth);
        let payload = legacy_provider_payload(&view, legacy_key);
        let raw_models = raw
            .get("models")
            .and_then(|m| m.as_object())
            .cloned()
            .unwrap_or_default();
        let p = provider_object_mut(root, id)?;
        wrote_secret = apply_provider_payload(p, &payload);
        apply_legacy_models(p, id, &view, &raw_models)?;
    }

    drop_empty_container(root, LEGACY_PROVIDERS_KEY);
    Ok(wrote_secret)
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

/// 把供应商 payload 应用到 `providers.<id>` 对象（OpenCode 2.x 写侧的唯一实现）。
/// 返回是否写入了明文密钥，调用方据此收紧配置文件权限。
fn apply_provider_payload(p: &mut Map<String, Value>, payload: &UpsertProviderPayload) -> bool {
    let mut wrote_secret = false;
    set_or_remove_str(p, "name", &payload.name);
    if let Some(ref pkg) = payload.package {
        let normalized = normalize_package(pkg);
        if normalized.is_empty() {
            p.remove("package");
        } else {
            p.insert("package".into(), Value::String(normalized));
        }
    }

    if payload.base_url.is_some()
        || payload.timeout.is_some()
        || payload.chunk_timeout.is_some()
        || payload.api_key.is_some()
    {
        let settings = ensure_settings_mut(p);
        if let Some(ref bu) = payload.base_url {
            if bu.is_empty() {
                settings.remove("baseURL");
            } else {
                settings.insert("baseURL".into(), Value::String(bu.clone()));
            }
        }
        if let Some(t) = payload.timeout {
            if t < 0 {
                settings.insert("timeout".into(), Value::Bool(false));
            } else if t == 0 {
                settings.remove("timeout");
            } else {
                settings.insert("timeout".into(), json!(t));
            }
        }
        if let Some(ct) = payload.chunk_timeout {
            if ct <= 0 {
                settings.remove("chunkTimeout");
            } else {
                settings.insert("chunkTimeout".into(), json!(ct));
            }
        }
        // OpenCode 2.x 只从配置 / 环境变量取密钥：统一写 settings.apiKey，
        // 并清掉 1.x 的 options.apiKey，避免明文双写。
        if let Some(ref key) = payload.api_key {
            if key.is_empty() {
                settings.remove("apiKey");
            } else {
                settings.insert("apiKey".into(), Value::String(key.clone()));
                wrote_secret = true;
            }
        }
        if settings.is_empty() {
            p.remove("settings");
        }
    }
    if payload.api_key.is_some() {
        if let Some(legacy_options) = p.get_mut("options").and_then(|o| o.as_object_mut()) {
            legacy_options.remove("apiKey");
            if legacy_options.is_empty() {
                p.remove("options");
            }
        }
    }
    wrote_secret
}

pub fn upsert_provider(payload: UpsertProviderPayload) -> Result<AgentActionResult, String> {
    let id = payload.id.trim().to_string();
    if id.is_empty() {
        return Err("供应商 ID 不能为空".into());
    }
    if id.contains('/') || id.contains(' ') {
        return Err("供应商 ID 不能包含空格或 /".into());
    }

    let (path, _, mut root, _) = load_or_empty_config()?;
    if !root.is_object() {
        root = json!({});
    }
    ensure_schema(&mut root);

    // Rename: move previous_id → id（2.x providers 与 1.x provider 两处都搬）
    if let Some(prev) = payload
        .previous_id
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && *s != id)
    {
        let map = providers_object_mut(&mut root)?;
        if let Some(old) = map.remove(&prev) {
            if map.contains_key(&id) {
                return Err(format!("供应商 `{id}` 已存在，无法重命名"));
            }
            map.insert(id.clone(), old);
        }
        if let Some(legacy) = root
            .get_mut(LEGACY_PROVIDERS_KEY)
            .and_then(|p| p.as_object_mut())
        {
            if let Some(old) = legacy.remove(&prev) {
                if legacy.contains_key(&id) {
                    return Err(format!("供应商 `{id}` 已存在，无法重命名"));
                }
                legacy.insert(id.clone(), old);
            }
        }
        // Move auth.json entry too (legacy 只读回退，仍保持一致)
        let mut auth = load_auth()?;
        if let Some(a) = auth.remove(&prev) {
            auth.insert(id.clone(), a);
            save_auth(&auth)?;
        }
    }

    // 先把 1.x 同名条目收编成 2.x（含它的模型表），再在它之上应用本次改动：
    // 2.x 的 `providers` 会覆盖 `provider`，旧条目留着只会让用户“保存成功但不生效”。
    // auth 只用于收编时的密钥来源标记，读失败不该阻断保存。
    let legacy_auth = load_auth().unwrap_or_default();
    let adopted_secret = adopt_legacy_provider(&mut root, &id, &legacy_auth)?;
    let wrote_secret = {
        let p = provider_object_mut(&mut root, &id)?;
        apply_provider_payload(p, &payload)
    } || adopted_secret;

    // 2.x 不再读取 auth.json：管理过的密钥统一以 settings.apiKey 为准，旧条目清掉。
    if payload.api_key.is_some() {
        let mut auth = load_auth()?;
        if auth.remove(&id).is_some() {
            save_auth(&auth)?;
        }
    }

    write_json_value(&path, &root)?;
    if wrote_secret {
        // 密钥进了 opencode.json，收紧权限（默认 644 会让同机其它用户可读）。
        platform::set_owner_only_file(&path);
    }
    Ok(AgentActionResult {
        ok: true,
        message: format!("供应商 `{id}` 已保存"),
        view: Some(get_config()?),
    })
}

pub fn delete_provider(
    provider_id: String,
    delete_auth: bool,
) -> Result<AgentActionResult, String> {
    let id = provider_id.trim().to_string();
    if id.is_empty() {
        return Err("供应商 ID 不能为空".into());
    }
    let (path, _, mut root, exists) = load_or_empty_config()?;
    if !exists {
        return Err("配置文件不存在".into());
    }

    let removed_v2 = take_from_container(&mut root, PROVIDERS_KEY, &id).is_some();
    let removed_legacy = take_from_container(&mut root, LEGACY_PROVIDERS_KEY, &id).is_some();
    if !removed_v2 && !removed_legacy {
        return Err(format!("未找到供应商 `{id}`"));
    }
    // 清掉变空的容器，避免留下 `"providers": {}` / `"provider": {}` 噪音
    drop_empty_container(&mut root, PROVIDERS_KEY);
    drop_empty_container(&mut root, LEGACY_PROVIDERS_KEY);

    write_json_value(&path, &root)?;
    if delete_auth {
        let mut auth = load_auth()?;
        if auth.remove(&id).is_some() {
            save_auth(&auth)?;
        }
    }
    Ok(AgentActionResult {
        ok: true,
        message: format!("已删除供应商 `{id}`"),
        view: Some(get_config()?),
    })
}

fn set_limit_field(limit: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    match value {
        None => {}
        Some(v) if v <= 0.0 => {
            limit.remove(key);
        }
        Some(v) => {
            limit.insert(key.to_string(), json!(v));
        }
    }
}

/// 把模型 payload 应用到 `providers.<id>.models.<mid>` 对象（OpenCode 2.x 写侧的唯一实现）。
/// 保存模型与 1.x 一键迁移共用，避免两处 schema 归一逻辑漂移。
fn apply_model_payload(m: &mut Map<String, Value>, payload: &UpsertModelPayload) {
    if let Some(ref n) = payload.name {
        if n.is_empty() {
            m.remove("name");
        } else {
            m.insert("name".into(), Value::String(n.clone()));
        }
    }

    // limit
    if payload.limit_context.is_some()
        || payload.limit_input.is_some()
        || payload.limit_output.is_some()
    {
        let limit_val = m
            .entry("limit".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !limit_val.is_object() {
            *limit_val = Value::Object(Map::new());
        }
        let limit = limit_val.as_object_mut().unwrap();
        set_limit_field(limit, "context", payload.limit_context);
        set_limit_field(limit, "input", payload.limit_input);
        set_limit_field(limit, "output", payload.limit_output);
        if limit.is_empty() {
            m.remove("limit");
        }
    }

    // capabilities：2.x 的能力模型（1.x 的 modalities / tool_call 不再写入）
    let need_capabilities = payload.modalities_input.is_some()
        || payload.modalities_output.is_some()
        || payload.tool_call.is_some()
        || payload.reasoning.is_some()
        || payload.thinking_budget_tokens.is_some();
    if need_capabilities {
        let cap_val = m
            .entry("capabilities".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !cap_val.is_object() {
            *cap_val = Value::Object(Map::new());
        }
        let cap = cap_val.as_object_mut().unwrap();

        for (key, value) in [
            ("input", payload.modalities_input.as_ref()),
            ("output", payload.modalities_output.as_ref()),
        ] {
            let Some(list) = value else { continue };
            if list.is_empty() {
                cap.remove(key);
            } else {
                cap.insert(
                    key.to_string(),
                    Value::Array(list.iter().map(|s| Value::String(s.clone())).collect()),
                );
            }
        }
        if let Some(tc) = payload.tool_call {
            cap.insert("tools".into(), Value::Bool(tc));
        }

        // reasoning：2.x 只接受 `{supported:false}` 或
        // `{supported:true, toggle?, effort?: string[], budget?:{min,max}}`；
        // 写成布尔会让 2.x 静默丢弃整个 provider，所以这里必须归一成对象。
        let budget = payload.thinking_budget_tokens;
        if payload.reasoning.is_some() || budget.is_some() {
            let mut reasoning = cap
                .get("reasoning")
                .and_then(|r| r.as_object())
                .cloned()
                .unwrap_or_default();
            let supported = payload.reasoning.unwrap_or(true) || budget.unwrap_or(0) > 0;
            if !supported {
                // `supported:false` 分支不允许其它字段
                cap.insert("reasoning".into(), json!({ "supported": false }));
            } else {
                reasoning.insert("supported".into(), Value::Bool(true));
                match budget {
                    Some(0) => {
                        reasoning.remove("budget");
                    }
                    Some(v) => {
                        let mut b = reasoning
                            .get("budget")
                            .and_then(|b| b.as_object())
                            .cloned()
                            .unwrap_or_default();
                        b.insert("max".into(), json!(v));
                        reasoning.insert("budget".into(), Value::Object(b));
                    }
                    None => {}
                }
                cap.insert("reasoning".into(), Value::Object(reasoning));
            }
        }

        if cap.is_empty() {
            m.remove("capabilities");
        }
    }

    // settings：2.x 模型请求参数（1.x 的 options 不再写入）
    let need_settings = payload.reasoning_effort.is_some()
        || payload.text_verbosity.is_some()
        || payload.extra_options.is_some();
    if need_settings {
        let st_val = m
            .entry("settings".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !st_val.is_object() {
            *st_val = Value::Object(Map::new());
        }
        let st = st_val.as_object_mut().unwrap();

        if let Some(ref re) = payload.reasoning_effort {
            if re.is_empty() {
                st.remove("reasoningEffort");
            } else {
                st.insert("reasoningEffort".into(), Value::String(re.clone()));
            }
        }
        if let Some(ref tv) = payload.text_verbosity {
            if tv.is_empty() {
                st.remove("textVerbosity");
            } else {
                st.insert("textVerbosity".into(), Value::String(tv.clone()));
            }
        }
        if let Some(ref extra) = payload.extra_options {
            let replace = payload.replace_extra_options.unwrap_or(false);
            if replace {
                // 保留已建模键，其余未知字段整体替换
                let keep_keys = ["reasoningEffort", "textVerbosity"];
                let retained: Map<String, Value> = st
                    .iter()
                    .filter(|(k, _)| keep_keys.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                *st = retained;
            }
            for (k, v) in extra {
                st.insert(k.clone(), v.clone());
            }
        }
        if st.is_empty() {
            m.remove("settings");
        }
    }

    // variants：2.x 是数组 `[{id, settings?, headers?, body?}]`
    if let Some(ref variants) = payload.variants {
        if variants.is_empty() {
            m.remove("variants");
        } else {
            let mut list = Vec::with_capacity(variants.len());
            for v in variants {
                let mut item = Map::new();
                item.insert("id".into(), Value::String(v.id.clone()));
                let mut settings = Map::new();
                if let Some(ref re) = v.reasoning_effort {
                    if !re.is_empty() {
                        settings.insert("reasoningEffort".into(), Value::String(re.clone()));
                    }
                }
                for (k, val) in &v.extra {
                    settings.insert(k.clone(), val.clone());
                }
                if !settings.is_empty() {
                    item.insert("settings".into(), Value::Object(settings));
                }
                for (k, val) in &v.overlay {
                    item.insert(k.clone(), val.clone());
                }
                list.push(Value::Object(item));
            }
            m.insert("variants".into(), Value::Array(list));
        }
    }
}

pub fn upsert_model(payload: UpsertModelPayload) -> Result<AgentActionResult, String> {
    let pid = payload.provider_id.trim().to_string();
    let mid = payload.id.trim().to_string();
    if pid.is_empty() || mid.is_empty() {
        return Err("供应商 ID 与模型 ID 均不能为空".into());
    }

    let (path, _, mut root, _) = load_or_empty_config()?;
    if !root.is_object() {
        root = json!({});
    }
    ensure_schema(&mut root);

    // 先把 1.x 同名条目收编成 2.x（含它的模型表），再写入本次模型改动；
    // 否则「保存一个模型」会把旧条目里其余模型一起删掉。
    let legacy_auth = load_auth().unwrap_or_default();
    let adopted_secret = adopt_legacy_provider(&mut root, &pid, &legacy_auth)?;

    // Rename model key
    if let Some(prev) = payload
        .previous_id
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && *s != mid)
    {
        let p = provider_object_mut(&mut root, &pid)?;
        let models = p
            .entry("models".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let map = models
            .as_object_mut()
            .ok_or_else(|| "models 必须是对象".to_string())?;
        if let Some(old) = map.remove(&prev) {
            if map.contains_key(&mid) {
                return Err(format!("模型 `{mid}` 已存在"));
            }
            map.insert(mid.clone(), old);
        }
    }

    {
        let p = provider_object_mut(&mut root, &pid)?;
        let models = p
            .entry("models".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let map = models
            .as_object_mut()
            .ok_or_else(|| "models 必须是对象".to_string())?;
        let model_val = map
            .entry(mid.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let m = model_val
            .as_object_mut()
            .ok_or_else(|| format!("model `{mid}` 必须是对象"))?;
        apply_model_payload(m, &payload);
    }

    write_json_value(&path, &root)?;
    if adopted_secret {
        // 收编时把 1.x 的明文密钥搬进了配置，同样需要收紧文件权限。
        platform::set_owner_only_file(&path);
    }
    Ok(AgentActionResult {
        ok: true,
        message: format!("模型 `{pid}/{mid}` 已保存"),
        view: Some(get_config()?),
    })
}

pub fn delete_model(provider_id: String, model_id: String) -> Result<AgentActionResult, String> {
    let pid = provider_id.trim().to_string();
    let mid = model_id.trim().to_string();
    let (path, _, mut root, exists) = load_or_empty_config()?;
    if !exists {
        return Err("配置文件不存在".into());
    }
    let take_model = |root: &mut Value, container: &str| -> bool {
        root.get_mut(container)
            .and_then(|p| p.as_object_mut())
            .and_then(|pm| pm.get_mut(&pid))
            .and_then(|p| p.as_object_mut())
            .and_then(|p| p.get_mut("models"))
            .and_then(|m| m.as_object_mut())
            .and_then(|m| m.remove(&mid))
            .is_some()
    };
    // 两个容器都要清：2.x 优先只影响读取，1.x 同名残留留着会「诈尸」。
    let removed_v2 = take_model(&mut root, PROVIDERS_KEY);
    let removed_legacy = take_model(&mut root, LEGACY_PROVIDERS_KEY);
    if !removed_v2 && !removed_legacy {
        return Err(format!("未找到模型 `{pid}/{mid}`"));
    }
    drop_empty_container(&mut root, PROVIDERS_KEY);
    drop_empty_container(&mut root, LEGACY_PROVIDERS_KEY);
    write_json_value(&path, &root)?;
    Ok(AgentActionResult {
        ok: true,
        message: format!("已删除模型 `{pid}/{mid}`"),
        view: Some(get_config()?),
    })
}

/// 1.x 供应商视图 → 2.x 写侧 payload。`setCacheKey` / `whitelist` / `blacklist`
/// 是 1.x 专有字段，2.x schema 里没有对应位置，迁移时丢弃。
fn legacy_provider_payload(
    view: &AgentProviderView,
    legacy_config_key: Option<String>,
) -> UpsertProviderPayload {
    UpsertProviderPayload {
        id: view.id.clone(),
        previous_id: None,
        name: view.name.clone(),
        package: view.package.clone(),
        api: None,
        base_url: view.base_url.clone(),
        timeout: view.timeout,
        chunk_timeout: view.chunk_timeout,
        api_key: legacy_config_key,
    }
}

/// 1.x 模型视图 → 2.x 写侧 payload。1.x 的 `attachment` 在 2.x 由
/// `capabilities.input` 表达，为 `true` 时补 `image`，避免迁移后图片能力丢失。
fn legacy_model_payload(
    provider_id: &str,
    model: &AgentModelView,
    raw: &Value,
) -> UpsertModelPayload {
    let mut modalities_input = model.modalities_input.clone();
    if bool_opt(raw, "attachment") == Some(true) && !modalities_input.iter().any(|m| m == "image") {
        modalities_input.push("image".into());
    }
    UpsertModelPayload {
        provider_id: provider_id.to_string(),
        id: model.id.clone(),
        previous_id: None,
        name: model.name.clone(),
        limit_context: model.limit_context,
        limit_input: model.limit_input,
        limit_output: model.limit_output,
        modalities_input: Some(modalities_input),
        modalities_output: Some(model.modalities_output.clone()),
        reasoning: model.reasoning,
        tool_call: model.tool_call,
        // 1.x 遗留字段：2.x 用 capabilities 表达，不再写回
        thinking_budget_tokens: model.thinking_budget_tokens,
        reasoning_effort: model.reasoning_effort.clone(),
        text_verbosity: model.text_verbosity.clone(),
        variants: Some(model.variants.clone()),
        extra_options: Some(model.extra_options.clone()),
        replace_extra_options: Some(true),
    }
}

/// 一键迁移 OpenCode 1.x 残留到 2.x（配置页黄色提示块的「迁移」按钮）。
///
/// - 转换与保存路径共用 `adopt_legacy_provider`，不会两套规则漂移；
/// - 只存在于顶层 `provider` 的供应商按 2.x schema 重建到 `providers.<id>`（含全部模型）；
/// - `providers` 里已有同名条目时以 2.x 为准（与读取侧一致），但会把 1.x 里唯一的密钥接续过去；
/// - 密钥统一收敛到 `providers.<id>.settings.apiKey`（含只存在于旧版 `auth.json` 的），
///   旧位置删除；写入明文密钥时收紧配置文件权限；
/// - 最后删除空掉的 `provider` 容器，全程单次原子写入。
///
/// 幂等：没有可迁移项时直接返回当前视图，不写盘。1.x 专有字段（`setCacheKey` /
/// `whitelist` / `blacklist` / 模型的 `status` / `cost` 等）在 2.x schema 里没有
/// 对应位置，迁移后不再保留。
pub fn migrate_legacy_config() -> Result<AgentActionResult, String> {
    let (path, _, mut root, exists) = load_or_empty_config()?;
    if !root.is_object() {
        root = json!({});
    }
    let mut auth = load_auth().unwrap_or_default();
    let legacy_ids: Vec<String> = if exists {
        root.get(LEGACY_PROVIDERS_KEY)
            .and_then(|p| p.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut migrated = 0usize; // 1.x → 2.x 重建的供应商数
    let mut cleaned = 0usize; // 2.x 已有同名条目，只清理 1.x 残留的数量
    let mut moved_keys = 0usize; // 从 auth.json 搬进配置的密钥数
    let mut wrote_secret = false;

    for id in &legacy_ids {
        // 2.x 已有同名条目 → 本次只算「清理残留」；否则算「重建」
        let already_v2 = root
            .get(PROVIDERS_KEY)
            .and_then(|p| p.get(id.as_str()))
            .is_some();
        // 与保存路径共用同一份 1.x → 2.x 转换（含模型表与密钥接续）。
        wrote_secret |= adopt_legacy_provider(&mut root, id, &auth)?;
        if already_v2 {
            cleaned += 1;
        } else {
            migrated += 1;
        }
    }

    // 密钥收敛：2.x 只从配置取密钥，1.x `options.apiKey` 与旧版 auth.json 都要搬过来。
    let mut auth_changed = false;
    let provider_ids: Vec<String> = root
        .get(PROVIDERS_KEY)
        .and_then(|p| p.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for id in provider_ids {
        let legacy_key = root
            .get(PROVIDERS_KEY)
            .and_then(|p| p.get(id.as_str()))
            .and_then(|p| map_str(legacy_options_of(p), "apiKey"))
            .map(|s| s.to_string());
        let auth_key = auth_get_key(&auth, &id);
        if legacy_key.is_none() && auth_key.is_none() {
            continue;
        }
        let used_auth_key = legacy_key.is_none() && auth_key.is_some();
        let inserted = {
            let p = provider_object_mut(&mut root, &id)?;
            let existing = map_str(p.get("settings").and_then(|s| s.as_object()), "apiKey")
                .map(|s| s.to_string());
            let to_write = if existing.is_some() {
                None
            } else {
                legacy_key.or(auth_key)
            };
            if let Some(ref key) = to_write {
                ensure_settings_mut(p).insert("apiKey".into(), Value::String(key.clone()));
            }
            // 1.x `options.apiKey` 与 2.x `settings.apiKey` 双写没有意义，清掉旧位置。
            if let Some(options) = p.get_mut("options").and_then(|o| o.as_object_mut()) {
                options.remove("apiKey");
                if options.is_empty() {
                    p.remove("options");
                }
            }
            to_write.is_some()
        };
        wrote_secret |= inserted;
        if used_auth_key && inserted {
            moved_keys += 1;
        }
        // 2.x 不再读取 auth.json：迁移后旧条目一律清掉。
        if auth.remove(&id).is_some() {
            auth_changed = true;
        }
    }

    // 清掉变空的 1.x 容器，避免留下 `"provider": {}` 噪音。
    drop_empty_container(&mut root, LEGACY_PROVIDERS_KEY);

    if !legacy_ids.is_empty() || auth_changed {
        let text = serde_json::to_string_pretty(&root).map_err(|e| format!("序列化失败: {e}"))?;
        let text = format!("{text}\n");
        if wrote_secret {
            // 密钥进了 opencode.json，收紧权限（默认 644 会让同机其它用户可读）。
            atomic_write_secret(&path, &text)?;
        } else {
            write_json_value(&path, &root)?;
        }
        if auth_changed {
            save_auth(&auth)?;
        }
    }

    let message = if legacy_ids.is_empty() && !auth_changed {
        "没有可迁移的 OpenCode 1.x 配置".to_string()
    } else {
        let mut parts = Vec::new();
        if migrated > 0 {
            parts.push(format!("{migrated} 个 1.x 供应商"));
        }
        if cleaned > 0 {
            parts.push(format!("{cleaned} 个重复的 1.x 残留"));
        }
        if moved_keys > 0 {
            parts.push(format!("{moved_keys} 个 auth.json 密钥"));
        }
        if parts.is_empty() {
            "OpenCode 配置已整理".to_string()
        } else {
            format!("已迁移 {}", parts.join("、"))
        }
    };

    Ok(AgentActionResult {
        ok: true,
        message,
        view: Some(get_config()?),
    })
}

pub fn get_provider_secret(provider_id: String) -> Result<String, String> {
    let id = provider_id.trim().to_string();
    let (_, _, root, _) = load_or_empty_config()?;
    // 2.x 唯一生效的位置：providers.<id>.settings.apiKey（旧配置回退 options.apiKey）
    for container in [PROVIDERS_KEY, LEGACY_PROVIDERS_KEY] {
        if let Some(p) = root.get(container).and_then(|p| p.get(&id)) {
            if let Some(k) = config_get_key(p) {
                return Ok(k);
            }
        }
    }
    // 旧版 auth.json：OpenCode 2.x 已不读取，仅作迁移期的只读回退
    let auth = load_auth()?;
    if let Some(k) = auth_get_key(&auth, &id) {
        return Ok(k);
    }
    Ok(String::new())
}

pub fn set_provider_secret(
    provider_id: String,
    api_key: String,
) -> Result<AgentActionResult, String> {
    upsert_provider(UpsertProviderPayload {
        id: provider_id,
        previous_id: None,
        name: None,
        package: None,
        api: None,
        base_url: None,
        timeout: None,
        chunk_timeout: None,
        api_key: Some(api_key),
    })
}

pub fn reveal_config() -> Result<AgentActionResult, String> {
    let (path, _, _, exists) = load_or_empty_config()?;
    if !exists {
        if let Some(parent) = path.parent() {
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
    platform::reveal_path(&path).map_err(|e| format!("打开文件管理器失败: {e}"))?;
    Ok(AgentActionResult {
        ok: true,
        message: format!("已在文件管理器中显示 {}", path.to_string_lossy()),
        view: None,
    })
}

/* ===== Models.dev catalog ===== */

struct CatalogCache {
    catalog: ModelsDevCatalog,
}

static CATALOG_CACHE: Mutex<Option<CatalogCache>> = Mutex::new(None);
/// 防止启动预热与页面首次打开同时触发重复网络请求。
static CATALOG_FETCH_LOCK: Mutex<()> = Mutex::new(());

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn catalog_cache_is_fresh(cached_at: Option<u64>, now: u64) -> bool {
    let Some(cached_at) = cached_at else {
        return false;
    };
    cached_at <= now && now - cached_at <= CATALOG_TTL.as_secs()
}

fn parse_catalog_json(raw: &Value) -> Result<ModelsDevCatalog, String> {
    let obj = raw
        .as_object()
        .ok_or_else(|| "models.dev 响应必须是对象".to_string())?;
    let mut providers = Vec::new();
    for (pid, pv) in obj {
        let name = pv
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or(pid)
            .to_string();
        let env = pv
            .get("env")
            .and_then(|e| e.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let npm = pv
            .get("npm")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string());
        let mut models = Vec::new();
        if let Some(mobj) = pv.get("models").and_then(|m| m.as_object()) {
            for (mid, mv) in mobj {
                let limit = mv.get("limit").cloned().unwrap_or(Value::Null);
                let modalities = mv.get("modalities").cloned().unwrap_or(Value::Null);
                let reasoning_options = mv
                    .get("reasoning_options")
                    .and_then(|r| r.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| {
                                let t = item.get("type")?.as_str()?.to_string();
                                let values = item.get("values").and_then(|v| {
                                    v.as_array().map(|a| {
                                        a.iter()
                                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                            .collect()
                                    })
                                });
                                let min = item.get("min").and_then(|m| m.as_u64());
                                Some(CatalogReasoningOption {
                                    r#type: t,
                                    values,
                                    min,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                models.push(CatalogModelSummary {
                    id: mid.clone(),
                    name: mv
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or(mid)
                        .to_string(),
                    limit_context: f64_opt(&limit, "context"),
                    limit_input: f64_opt(&limit, "input"),
                    limit_output: f64_opt(&limit, "output"),
                    modalities_input: string_list(&modalities, "input"),
                    modalities_output: string_list(&modalities, "output"),
                    reasoning: bool_opt(mv, "reasoning").unwrap_or(false),
                    reasoning_options,
                    tool_call: bool_opt(mv, "tool_call").unwrap_or(false),
                    attachment: bool_opt(mv, "attachment").unwrap_or(false),
                    status: str_opt(mv, "status"),
                });
            }
            models.sort_by(|a, b| a.id.cmp(&b.id));
        }
        providers.push(CatalogProvider {
            id: pid.clone(),
            name,
            env,
            npm,
            models,
        });
    }
    providers.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(ModelsDevCatalog {
        fetched_at: now_unix(),
        from_cache: false,
        providers,
    })
}

fn build_http_client() -> Result<reqwest::blocking::Client, String> {
    let builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("AgentBuddy/0.1 (opencode-config)");
    crate::http_client::apply_proxy(builder)?
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

fn catalog_cache_file() -> Result<PathBuf, String> {
    Ok(crate::config::app_dir()?.join("Models.dev.json"))
}

fn load_file_cache() -> Option<ModelsDevCatalog> {
    let primary = catalog_cache_file().ok()?;
    let is_legacy = !primary.is_file();
    let path = if !is_legacy {
        primary
    } else {
        // 兼容旧版本的缓存位置；成功读取后会写入新位置。
        crate::config::app_dir()
            .ok()?
            .join("cache")
            .join("models-dev.json")
    };
    let raw = fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    // Stored as already-slim catalog
    let mut cat: ModelsDevCatalog = serde_json::from_value(v).ok()?;
    cat.from_cache = true;
    if is_legacy {
        let _ = save_file_cache(&cat);
    }
    Some(cat)
}

fn save_file_cache(cat: &ModelsDevCatalog) -> Result<(), String> {
    let path = catalog_cache_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建 Models.dev 缓存目录失败: {e}"))?;
    }
    let text =
        serde_json::to_string(cat).map_err(|e| format!("序列化 Models.dev 缓存失败: {e}"))?;
    atomic_write(&path, &text)
}

pub fn fetch_models_dev_catalog(force: bool) -> Result<ModelsDevCatalog, String> {
    let _fetch_guard = CATALOG_FETCH_LOCK
        .lock()
        .map_err(|_| "Models.dev 目录锁已损坏".to_string())?;
    if !force {
        let cached_at = crate::config::load_models_dev_cached_at()?;
        let fresh = catalog_cache_is_fresh(cached_at, now_unix());
        if fresh {
            if let Ok(guard) = CATALOG_CACHE.lock() {
                if let Some(c) = guard.as_ref() {
                    let mut cat = c.catalog.clone();
                    cat.from_cache = true;
                    return Ok(cat);
                }
            }
            if let Some(cat) = load_file_cache() {
                if let Ok(mut guard) = CATALOG_CACHE.lock() {
                    *guard = Some(CatalogCache {
                        catalog: cat.clone(),
                    });
                }
                return Ok(cat);
            }
        }
    }

    let client = build_http_client()?;
    let resp = client
        .get(MODELS_DEV_URL)
        .send()
        .map_err(|e| format!("请求 models.dev 失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("models.dev 返回 HTTP {}", resp.status()));
    }
    let raw: Value = resp
        .json()
        .map_err(|e| format!("解析 models.dev JSON 失败: {e}"))?;
    let mut catalog = parse_catalog_json(&raw)?;
    catalog.from_cache = false;
    // 只有目录文件安全写入后，才更新配置中的缓存时间。
    save_file_cache(&catalog)?;
    crate::config::save_models_dev_cached_at(catalog.fetched_at)?;
    if let Ok(mut guard) = CATALOG_CACHE.lock() {
        *guard = Some(CatalogCache {
            catalog: catalog.clone(),
        });
    }
    Ok(catalog)
}

pub fn probe_models_endpoint(base_url: String) -> Result<ProbeModelsResult, String> {
    let base = base_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err("baseURL 不能为空".into());
    }
    let url = if base.ends_with("/models") {
        base
    } else {
        format!("{base}/models")
    };
    let client = build_http_client()?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("探测失败: {e}"))?;
    if !resp.status().is_success() {
        return Ok(ProbeModelsResult {
            ok: false,
            message: format!("HTTP {}", resp.status()),
            model_ids: vec![],
        });
    }
    let v: Value = resp.json().map_err(|e| format!("解析响应失败: {e}"))?;
    let mut ids = Vec::new();
    if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
        for item in arr {
            if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                ids.push(id.to_string());
            }
        }
    } else if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                ids.push(id.to_string());
            }
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ProbeModelsResult {
        ok: true,
        message: format!("发现 {} 个模型", ids.len()),
        model_ids: ids,
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
        fn new() -> Self {
            let guard = crate::config::lock_home_for_test();
            let path = std::env::temp_dir().join(format!(
                "agentbuddy-oc-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(path.join(".config/opencode")).unwrap();
            fs::create_dir_all(path.join(".local/share/opencode")).unwrap();
            // dirs::home_dir does not honor HOME on all platforms the same way —
            // we set HOME for path helpers that use dirs::home_dir.
            let prev_home = std::env::var_os("HOME");
            std::env::set_var("HOME", &path);
            // get_config 会用 sniff::is_agent_installed("opencode") 门控视图；
            // 在沙箱 PATH 里放一个假 opencode 二进制，避免依赖开发机真实安装。
            let bin = path.join("bin");
            fs::create_dir_all(&bin).unwrap();
            fs::write(bin.join("opencode"), "#!/bin/sh\nexit 0\n").unwrap();
            #[cfg(windows)]
            fs::write(bin.join("opencode.exe"), "").unwrap();
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

        fn config_file(&self) -> PathBuf {
            self.path.join(".config/opencode/opencode.json")
        }

        fn auth_file(&self) -> PathBuf {
            self.path.join(".local/share/opencode/auth.json")
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

    fn load_root(h: &TempHome) -> Value {
        serde_json::from_str(&fs::read_to_string(h.config_file()).unwrap()).unwrap()
    }

    fn view() -> AgentModelConfigView {
        get_config().unwrap()
    }

    #[test]
    fn empty_config_view() {
        let _h = TempHome::new();
        let v = view();
        assert!(!v.config_exists);
        assert!(v.providers.is_empty());
        assert!(v.model.is_none());
    }

    #[test]
    fn reads_v2_providers_and_reports_v1_only_ids() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "providers": {
                    "magpie": {
                        "name": "magpie",
                        "package": "aisdk:@ai-sdk/openai-compatible",
                        "settings": { "baseURL": "http://127.0.0.1:3425/v1", "apiKey": "sk-native" },
                        "models": {
                            "m": { "name": "M", "limit": { "context": 1000, "input": 900, "output": 100 },
                                   "capabilities": { "tools": true, "input": ["text","image"], "output": ["text"] } }
                        }
                    }
                },
                "provider": {
                    "router": {
                        "name": "路由聚合",
                        "npm": "@ai-sdk/openai",
                        "options": { "baseURL": "http://127.0.0.1:16888/v1" },
                        "models": { "m2": { "name": "M2", "modalities": { "input": ["text"], "output": ["text"] } } }
                    }
                },
                "model": "magpie/m"
            })
            .to_string(),
        )
        .unwrap();

        let v = view();
        let ids: Vec<_> = v.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["magpie", "router"]);

        // 2.x 条目按原样读出
        let magpie = &v.providers[0];
        assert_eq!(
            magpie.package.as_deref(),
            Some("aisdk:@ai-sdk/openai-compatible")
        );
        assert_eq!(magpie.base_url.as_deref(), Some("http://127.0.0.1:3425/v1"));
        assert_eq!(magpie.api_key_source, "config");
        assert_eq!(magpie.models[0].modalities_input, vec!["text", "image"]);
        assert_eq!(magpie.models[0].tool_call, Some(true));

        // 1.x 条目兼容读出：npm 归一为 2.x package、options.baseURL 提升为 Base URL
        let router = &v.providers[1];
        assert_eq!(router.package.as_deref(), Some("aisdk:@ai-sdk/openai"));
        assert_eq!(
            router.base_url.as_deref(),
            Some("http://127.0.0.1:16888/v1")
        );
        // 1.x 只有 modalities：input 提升为 modalities_input，tool_call 为 None
        assert_eq!(router.models[0].modalities_input, vec!["text"]);

        assert!(v.warnings.iter().any(|w| w.contains("1.x")));
        // 待迁移项：只有 1.x 独有条目
        let info = v.legacy_migration.as_ref().expect("应上报 1.x 待迁移项");
        assert_eq!(info.providers, vec!["router"]);
        assert!(info.auth_providers.is_empty());
        assert_eq!(v.model.as_deref(), Some("magpie/m"));
    }

    #[test]
    fn v2_wins_over_legacy_for_same_id() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": { "p": { "name": "legacy", "npm": "@ai-sdk/openai", "options": { "baseURL": "http://legacy/v1" } } },
                "providers": { "p": { "name": "native", "package": "aisdk:@ai-sdk/openai", "settings": { "baseURL": "http://native/v1" } } }
            })
            .to_string(),
        )
        .unwrap();
        let v = view();
        assert_eq!(v.providers.len(), 1);
        assert_eq!(v.providers[0].name.as_deref(), Some("native"));
        assert_eq!(v.providers[0].base_url.as_deref(), Some("http://native/v1"));
        // 同 id 被覆盖，不再算 1.x 残留
        assert!(!v.warnings.iter().any(|w| w.contains("1.x")));
        assert!(v.legacy_migration.is_none());
    }

    #[test]
    fn small_model_reads_agents_title_model() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "providers": {},
                "model": "a/b",
                "agents": { "title": { "model": "a/c" } }
            })
            .to_string(),
        )
        .unwrap();
        let v = view();
        assert_eq!(v.model.as_deref(), Some("a/b"));
        assert_eq!(v.small_model.as_deref(), Some("a/c"));
        assert_eq!(v.small_model_key.as_deref(), Some("agents.title.model"));
    }

    #[test]
    fn legacy_small_model_still_readable() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({ "model": "a/b", "small_model": "a/c" }).to_string(),
        )
        .unwrap();
        let v = view();
        assert_eq!(v.small_model.as_deref(), Some("a/c"));
    }

    #[test]
    fn set_defaults_writes_agents_title_model_and_clears_legacy_key() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({ "model": "old/p", "small_model": "old/s", "agents": { "title": { "model": "old/s" } } })
                .to_string(),
        )
        .unwrap();

        set_defaults(SetDefaultsPayload {
            model: Some("new/p".into()),
            small_model: Some("new/s".into()),
            enabled_providers: None,
            disabled_providers: None,
            sync_omo_model_profile: None,
        })
        .unwrap();

        let raw = load_root(&h);
        assert!(raw.get(LEGACY_SMALL_MODEL_KEY).is_none());
        assert_eq!(
            raw.pointer("/agents/title/model").and_then(|x| x.as_str()),
            Some("new/s")
        );
        assert_eq!(
            raw.pointer("/model").and_then(|x| x.as_str()),
            Some("new/p")
        );

        // 清空第二槽位：连带清理变空的 agents.title
        set_defaults(SetDefaultsPayload {
            model: None,
            small_model: Some(String::new()),
            enabled_providers: None,
            disabled_providers: None,
            sync_omo_model_profile: None,
        })
        .unwrap();
        let raw = load_root(&h);
        assert!(raw.get("agents").is_none());
    }

    #[test]
    fn upsert_provider_writes_v2_shape() {
        let h = TempHome::new();
        upsert_provider(UpsertProviderPayload {
            id: "ollama".into(),
            previous_id: None,
            name: Some("Ollama".into()),
            package: Some("@ai-sdk/openai-compatible".into()),
            api: None,
            base_url: Some("http://localhost:11434/v1".into()),
            timeout: Some(600000),
            chunk_timeout: Some(30000),
            api_key: Some("sk-secret".into()),
        })
        .unwrap();

        let raw = load_root(&h);
        // 顶层只允许 2.x 容器；绝不出现 1.x 键
        assert!(raw.get(PROVIDERS_KEY).is_some());
        assert!(raw.get(LEGACY_PROVIDERS_KEY).is_none());
        let p = raw.pointer("/providers/ollama").unwrap();
        assert_eq!(
            p.pointer("/package").and_then(|x| x.as_str()),
            Some("aisdk:@ai-sdk/openai-compatible"),
            "AI SDK 包必须补 aisdk: 前缀"
        );
        assert_eq!(
            p.pointer("/settings/baseURL").and_then(|x| x.as_str()),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(
            p.pointer("/settings/timeout").and_then(|x| x.as_i64()),
            Some(600000)
        );
        assert!(p.pointer("/settings/apiKey").is_some());
        assert!(p.pointer("/options").is_none(), "1.x options 不得写回");
        assert!(p.pointer("/npm").is_none(), "1.x npm 不得写回");
        // auth.json 不再承担密钥
        assert!(!h.auth_file().exists());

        let v = view();
        let p = v.providers.iter().find(|p| p.id == "ollama").unwrap();
        assert_eq!(p.api_key_source, "config");
        assert_eq!(p.timeout, Some(600000));
        assert_eq!(p.chunk_timeout, Some(30000));
    }

    #[test]
    fn secret_saved_to_config_and_permissions_tightened() {
        let h = TempHome::new();
        set_provider_secret("demo".into(), "sk-test-secret".into()).unwrap();

        let raw = load_root(&h);
        assert_eq!(
            raw.pointer("/providers/demo/settings/apiKey")
                .and_then(|x| x.as_str()),
            Some("sk-test-secret")
        );
        assert!(!fs::read_to_string(h.auth_file())
            .unwrap_or_default()
            .contains("sk-test-secret"));

        let v = view();
        let p = v.providers.iter().find(|p| p.id == "demo").unwrap();
        assert!(p.has_api_key);
        assert_eq!(p.api_key_source, "config");
        assert_eq!(
            get_provider_secret("demo".into()).unwrap(),
            "sk-test-secret"
        );

        // 清空 → settings.apiKey 删除
        set_provider_secret("demo".into(), "".into()).unwrap();
        let raw = load_root(&h);
        assert!(raw.pointer("/providers/demo/settings/apiKey").is_none());
        assert!(
            !view()
                .providers
                .iter()
                .find(|p| p.id == "demo")
                .unwrap()
                .has_api_key
        );
    }

    #[test]
    fn legacy_auth_key_readable_and_migrated_on_save() {
        let h = TempHome::new();
        fs::write(
            h.auth_file(),
            r#"{"oldprov": {"type": "api", "key": "sk-legacy"}}"#,
        )
        .unwrap();
        fs::write(
            h.config_file(),
            json!({
                "providers": {
                    "oldprov": { "name": "Old", "package": "aisdk:@ai-sdk/openai", "settings": { "baseURL": "http://x/v1" } }
                }
            })
            .to_string(),
        )
        .unwrap();

        // 只读回退可见
        assert_eq!(get_provider_secret("oldprov".into()).unwrap(), "sk-legacy");
        let v = view();
        assert!(v.warnings.iter().any(|w| w.contains("auth.json")));

        // 保存时密钥迁到 settings.apiKey，auth.json 条目删除
        upsert_provider(UpsertProviderPayload {
            id: "oldprov".into(),
            previous_id: None,
            name: Some("Migrated".into()),
            package: Some("aisdk:@ai-sdk/openai".into()),
            api: None,
            base_url: Some("http://x/v1".into()),
            timeout: None,
            chunk_timeout: None,
            api_key: Some("sk-legacy".into()),
        })
        .unwrap();

        let raw = load_root(&h);
        assert_eq!(
            raw.pointer("/providers/oldprov/settings/apiKey")
                .and_then(|x| x.as_str()),
            Some("sk-legacy")
        );
        let auth: Value =
            serde_json::from_str(&fs::read_to_string(h.auth_file()).unwrap()).unwrap();
        assert!(auth.get("oldprov").is_none());
    }

    #[test]
    fn saving_v1_provider_migrates_it_into_v2() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": {
                    "router": {
                        "name": "路由聚合",
                        "npm": "@ai-sdk/openai",
                        "options": { "baseURL": "http://127.0.0.1:16888/v1", "apiKey": "sk-legacy" },
                        "models": {
                            "m1": { "name": "M1", "modalities": { "input": ["text"], "output": ["text"] } },
                            "m2": { "name": "M2" }
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        upsert_provider(UpsertProviderPayload {
            id: "router".into(),
            previous_id: None,
            name: Some("路由聚合".into()),
            package: Some("aisdk:@ai-sdk/openai".into()),
            api: None,
            base_url: Some("http://127.0.0.1:16888/v1".into()),
            timeout: None,
            chunk_timeout: None,
            api_key: None,
        })
        .unwrap();

        let raw = load_root(&h);
        assert!(
            raw.get(LEGACY_PROVIDERS_KEY).is_none(),
            "同名 1.x 条目必须被清理"
        );
        assert!(raw.pointer("/providers/router").is_some());
        // 保存不得丢掉 1.x 条目里的模型表与密钥（曾经的回归点）
        assert!(
            raw.pointer("/providers/router/models/m1").is_some(),
            "保存供应商必须保留原模型"
        );
        assert!(raw.pointer("/providers/router/models/m2").is_some());
        assert_eq!(
            raw.pointer("/providers/router/settings/apiKey")
                .and_then(|x| x.as_str()),
            Some("sk-legacy")
        );
        assert!(view().warnings.iter().all(|w| !w.contains("1.x")));
    }

    #[test]
    fn saving_one_model_of_v1_provider_keeps_the_others() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": {
                    "local": {
                        "name": "Local",
                        "npm": "@ai-sdk/openai-compatible",
                        "options": { "baseURL": "http://127.0.0.1:8080/v1" },
                        "models": {
                            "keep": { "name": "Keep" },
                            "edit": { "name": "Edit" }
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        upsert_model(UpsertModelPayload {
            provider_id: "local".into(),
            id: "edit".into(),
            previous_id: None,
            name: Some("Edited".into()),
            limit_context: None,
            limit_input: None,
            limit_output: None,
            modalities_input: Some(vec!["text".into()]),
            modalities_output: Some(vec!["text".into()]),
            reasoning: None,
            tool_call: None,
            thinking_budget_tokens: None,
            reasoning_effort: None,
            text_verbosity: None,
            variants: None,
            extra_options: None,
            replace_extra_options: None,
        })
        .unwrap();

        let raw = load_root(&h);
        assert!(raw.get(LEGACY_PROVIDERS_KEY).is_none());
        // 只改一个模型，其余模型与供应商字段必须完好
        assert!(raw.pointer("/providers/local/models/keep").is_some());
        assert_eq!(
            raw.pointer("/providers/local/models/edit/name")
                .and_then(|x| x.as_str()),
            Some("Edited")
        );
        assert_eq!(
            raw.pointer("/providers/local/package")
                .and_then(|x| x.as_str()),
            Some("aisdk:@ai-sdk/openai-compatible")
        );
        assert_eq!(
            raw.pointer("/providers/local/settings/baseURL")
                .and_then(|x| x.as_str()),
            Some("http://127.0.0.1:8080/v1")
        );
    }

    #[test]
    fn setting_secret_on_v1_provider_keeps_models() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": {
                    "local": { "npm": "@ai-sdk/openai", "options": { "baseURL": "http://x/v1" },
                               "models": { "m": { "name": "M" } } }
                }
            })
            .to_string(),
        )
        .unwrap();

        set_provider_secret("local".into(), "sk-new".into()).unwrap();

        let raw = load_root(&h);
        assert!(raw.pointer("/providers/local/models/m").is_some());
        assert_eq!(
            raw.pointer("/providers/local/settings/apiKey")
                .and_then(|x| x.as_str()),
            Some("sk-new")
        );
    }

    #[test]
    fn migrate_legacy_keeps_v1_key_when_v2_entry_already_exists() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": {
                    "p": { "name": "legacy", "npm": "@ai-sdk/openai", "options": { "baseURL": "http://legacy/v1", "apiKey": "sk-legacy" } }
                },
                "providers": {
                    "p": { "name": "native", "package": "aisdk:@ai-sdk/openai", "settings": { "baseURL": "http://native/v1" } }
                }
            })
            .to_string(),
        )
        .unwrap();

        migrate_legacy_config().unwrap();

        let raw = load_root(&h);
        assert!(raw.get(LEGACY_PROVIDERS_KEY).is_none());
        // 2.x 条目优先，但 1.x 里唯一的密钥必须接续过来
        assert_eq!(
            raw.pointer("/providers/p/name").and_then(|x| x.as_str()),
            Some("native")
        );
        assert_eq!(
            raw.pointer("/providers/p/settings/apiKey")
                .and_then(|x| x.as_str()),
            Some("sk-legacy")
        );
    }

    #[test]
    fn dual_auth_warning_is_reported_separately() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "providers": {
                    "dup": { "name": "Dup", "package": "aisdk:@ai-sdk/openai",
                             "settings": { "baseURL": "http://x/v1", "apiKey": "sk-config" } }
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            h.auth_file(),
            r#"{"dup": {"type": "api", "key": "sk-auth"}}"#,
        )
        .unwrap();

        let v = view();
        assert_eq!(v.providers[0].api_key_source, "both");
        // 两边都有密钥 ≠ 密钥只在 auth.json，文案不能混用
        assert!(
            v.warnings.iter().any(|w| w.contains("旧密钥条目")),
            "{:?}",
            v.warnings
        );
        assert!(!v.warnings.iter().any(|w| w.contains("只存在于该文件")));
        // 两边都有密钥时没有需要迁移的 auth.json 项
        assert!(v.legacy_migration.is_none());
    }

    #[test]
    fn delete_provider_cleans_both_containers() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": { "a": { "models": { "m1": {"name":"M1"}, "m2": {"name":"M2"} } }, "b": { "models": { "n": {} } } },
                "providers": { "b": { "name": "B", "package": "aisdk:@ai-sdk/openai" } }
            })
            .to_string(),
        )
        .unwrap();

        // v2 删除：provider 里的同名残留一并清掉
        delete_provider("b".into(), true).unwrap();
        let raw = load_root(&h);
        assert!(raw.pointer("/providers/b").is_none());
        assert!(raw.pointer("/provider/b").is_none());

        // legacy 删除：容器为空后整体移除
        delete_provider("a".into(), false).unwrap();
        let raw = load_root(&h);
        assert!(raw.get(LEGACY_PROVIDERS_KEY).is_none());
        assert!(view().providers.is_empty());

        // 不存在的 id 报错
        assert!(delete_provider("zzz".into(), false).is_err());
    }

    #[test]
    fn upsert_model_writes_v2_shape_and_preserves_unrelated_keys() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "$schema": SCHEMA_URL,
                "mcp": { "demo": { "type": "local", "command": ["echo"] } },
                "providers": {
                    "local": {
                        "package": "aisdk:@ai-sdk/openai-compatible",
                        "name": "Local",
                        "settings": { "baseURL": "http://127.0.0.1:8080/v1" },
                        "models": {
                            "qwen": {
                                "name": "Qwen",
                                "limit": { "context": 128000, "output": 65536 },
                                "capabilities": { "input": ["text"], "output": ["text"] }
                            }
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        upsert_model(UpsertModelPayload {
            provider_id: "local".into(),
            id: "qwen".into(),
            previous_id: None,
            name: Some("Qwen Coder".into()),
            limit_context: Some(200000.0),
            limit_input: None,
            limit_output: Some(80000.0),
            modalities_input: Some(vec!["text".into(), "image".into()]),
            modalities_output: Some(vec!["text".into()]),
            reasoning: Some(true),
            tool_call: None,
            thinking_budget_tokens: Some(4096),
            reasoning_effort: Some("high".into()),
            text_verbosity: None,
            variants: None,
            extra_options: None,
            replace_extra_options: None,
        })
        .unwrap();

        let raw = load_root(&h);
        assert!(raw.get("mcp").is_some(), "mcp 必须保留");
        assert!(raw.get(LEGACY_PROVIDERS_KEY).is_none());
        let m = raw.pointer("/providers/local/models/qwen").unwrap();
        assert_eq!(
            m.pointer("/limit/context").and_then(|x| x.as_f64()),
            Some(200000.0)
        );
        // capabilities：modality 数组 + reasoning 对象（绝不能是布尔）
        assert_eq!(
            m.pointer("/capabilities/input")
                .and_then(|x| x.as_array())
                .map(|a| a.len()),
            Some(2)
        );
        let reasoning = m.pointer("/capabilities/reasoning").unwrap();
        assert_eq!(
            reasoning.pointer("/supported").and_then(|x| x.as_bool()),
            Some(true)
        );
        assert_eq!(
            reasoning.pointer("/budget/max").and_then(|x| x.as_u64()),
            Some(4096)
        );
        // settings：reasoningEffort 在模型 settings，而非 options
        assert_eq!(
            m.pointer("/settings/reasoningEffort")
                .and_then(|x| x.as_str()),
            Some("high")
        );
        assert!(m.pointer("/options").is_none(), "1.x options 不得写回");
        assert!(
            m.pointer("/modalities").is_none(),
            "1.x modalities 不得写回"
        );

        let v = view();
        let m = &v.providers[0].models[0];
        assert_eq!(m.limit_context, Some(200000.0));
        assert!(m.modalities_input.contains(&"image".into()));
        assert_eq!(m.reasoning_effort.as_deref(), Some("high"));
        assert!(m.reasoning == Some(true));
    }

    #[test]
    fn upsert_model_writes_variants_as_array() {
        let _h = TempHome::new();
        upsert_provider(UpsertProviderPayload {
            id: "p".into(),
            previous_id: None,
            name: None,
            package: Some("aisdk:@ai-sdk/openai".into()),
            api: None,
            base_url: None,
            timeout: None,
            chunk_timeout: None,
            api_key: None,
        })
        .unwrap();
        upsert_model(UpsertModelPayload {
            provider_id: "p".into(),
            id: "m".into(),
            previous_id: None,
            name: Some("M".into()),
            limit_context: None,
            limit_input: None,
            limit_output: None,
            modalities_input: None,
            modalities_output: None,
            reasoning: None,
            tool_call: None,
            thinking_budget_tokens: None,
            reasoning_effort: None,
            text_verbosity: None,
            variants: Some(vec![
                AgentVariantView {
                    id: "high".into(),
                    reasoning_effort: Some("high".into()),
                    extra: Map::new(),
                    overlay: Map::new(),
                },
                AgentVariantView {
                    id: "low".into(),
                    reasoning_effort: Some("low".into()),
                    extra: Map::new(),
                    overlay: [("body".to_string(), json!({ "store": false }))]
                        .into_iter()
                        .collect(),
                },
            ]),
            extra_options: None,
            replace_extra_options: None,
        })
        .unwrap();

        let h_dir = std::env::var("HOME").unwrap();
        let raw: Value = serde_json::from_str(
            &fs::read_to_string(PathBuf::from(h_dir).join(".config/opencode/opencode.json"))
                .unwrap(),
        )
        .unwrap();
        let variants = raw
            .pointer("/providers/p/models/m/variants")
            .and_then(|x| x.as_array())
            .expect("variants 必须是数组（对象写法会被 OpenCode 2.x 静默丢弃）");
        assert_eq!(variants.len(), 2);
        assert_eq!(
            variants[0].pointer("/id").and_then(|x| x.as_str()),
            Some("high")
        );
        assert_eq!(
            variants[0]
                .pointer("/settings/reasoningEffort")
                .and_then(|x| x.as_str()),
            Some("high")
        );
        assert_eq!(
            variants[1].pointer("/body/store").and_then(|x| x.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn reasoning_false_writes_supported_false_object() {
        let _h = TempHome::new();
        upsert_provider(UpsertProviderPayload {
            id: "p".into(),
            previous_id: None,
            name: None,
            package: Some("aisdk:@ai-sdk/openai".into()),
            api: None,
            base_url: None,
            timeout: None,
            chunk_timeout: None,
            api_key: None,
        })
        .unwrap();
        upsert_model(UpsertModelPayload {
            provider_id: "p".into(),
            id: "m".into(),
            previous_id: None,
            name: Some("M".into()),
            limit_context: None,
            limit_input: None,
            limit_output: None,
            modalities_input: None,
            modalities_output: None,
            reasoning: Some(false),
            tool_call: None,
            thinking_budget_tokens: None,
            reasoning_effort: None,
            text_verbosity: None,
            variants: None,
            extra_options: None,
            replace_extra_options: None,
        })
        .unwrap();

        let h_dir = std::env::var("HOME").unwrap();
        let raw: Value = serde_json::from_str(
            &fs::read_to_string(PathBuf::from(h_dir).join(".config/opencode/opencode.json"))
                .unwrap(),
        )
        .unwrap();
        let reasoning = raw
            .pointer("/providers/p/models/m/capabilities/reasoning")
            .unwrap();
        assert_eq!(
            reasoning.as_object().unwrap().len(),
            1,
            "supported:false 分支不允许其它字段"
        );
        assert_eq!(
            reasoning.pointer("/supported").and_then(|x| x.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn delete_model_works_on_both_containers() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": { "a": { "models": { "m1": {"name":"M1"} } } },
                "providers": { "b": { "package": "aisdk:@ai-sdk/openai", "models": { "m2": {"name":"M2"} } } }
            })
            .to_string(),
        )
        .unwrap();

        delete_model("a".into(), "m1".into()).unwrap();
        delete_model("b".into(), "m2".into()).unwrap();
        let raw = load_root(&h);
        assert!(raw.pointer("/provider/a/models/m1").is_none());
        assert!(raw.pointer("/providers/b/models/m2").is_none());
        assert!(delete_model("b".into(), "zzz".into()).is_err());

        // 两个容器同名模型：一次删除必须两处都清，否则 1.x 残留会「诈尸」
        fs::write(
            h.config_file(),
            json!({
                "provider": { "p": { "models": { "m": { "name": "V1" } } } },
                "providers": { "p": { "package": "aisdk:@ai-sdk/openai", "models": { "m": { "name": "V2" } } } }
            })
            .to_string(),
        )
        .unwrap();
        delete_model("p".into(), "m".into()).unwrap();
        let raw = load_root(&h);
        assert!(raw.pointer("/providers/p/models/m").is_none());
        assert!(raw.pointer("/provider/p/models/m").is_none());
    }

    #[test]
    fn jsonc_config_roundtrip() {
        let h = TempHome::new();
        let jsonc = h.path.join(".config/opencode/opencode.jsonc");
        fs::write(
            &jsonc,
            r#"{
              // comment
              "model": "a/b",
              "providers": {}
            }"#,
        )
        .unwrap();
        let v = view();
        assert!(v.config_exists);
        assert!(v.is_jsonc);
        assert_eq!(v.model.as_deref(), Some("a/b"));
        assert!(!v.warnings.is_empty());

        set_defaults(SetDefaultsPayload {
            model: Some("local/qwen".into()),
            small_model: Some("local/tiny".into()),
            enabled_providers: None,
            disabled_providers: None,
            sync_omo_model_profile: None,
        })
        .unwrap();
        let v = view();
        assert_eq!(v.model.as_deref(), Some("local/qwen"));
        assert_eq!(v.small_model.as_deref(), Some("local/tiny"));
    }

    #[test]
    fn parse_catalog_fixture() {
        let raw = json!({
            "anthropic": {
                "id": "anthropic",
                "name": "Anthropic",
                "env": ["ANTHROPIC_API_KEY"],
                "npm": "@ai-sdk/anthropic",
                "models": {
                    "claude-x": {
                        "id": "claude-x",
                        "name": "Claude X",
                        "reasoning": true,
                        "reasoning_options": [
                            { "type": "effort", "values": ["low", "high"] },
                            { "type": "budget_tokens", "min": 1024 }
                        ],
                        "tool_call": true,
                        "attachment": true,
                        "modalities": { "input": ["text", "image"], "output": ["text"] },
                        "limit": { "context": 200000, "output": 64000 }
                    }
                }
            }
        });
        let cat = parse_catalog_json(&raw).unwrap();
        assert_eq!(cat.providers.len(), 1);
        let m = &cat.providers[0].models[0];
        assert_eq!(m.limit_context, Some(200000.0));
        assert_eq!(m.modalities_input, vec!["text", "image"]);
        assert_eq!(m.reasoning_options.len(), 2);
    }

    #[test]
    fn catalog_cache_ttl_is_7_days_and_rejects_future_timestamps() {
        let now = 1_000_000;
        assert!(catalog_cache_is_fresh(Some(now), now));
        assert!(catalog_cache_is_fresh(Some(now - 7 * 24 * 60 * 60), now));
        assert!(!catalog_cache_is_fresh(
            Some(now - 7 * 24 * 60 * 60 - 1),
            now
        ));
        assert!(!catalog_cache_is_fresh(Some(now + 1), now));
        assert!(!catalog_cache_is_fresh(None, now));
    }

    #[test]
    fn migrate_legacy_rebuilds_v1_provider_in_v2_shape() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": {
                    "router": {
                        "name": "路由聚合",
                        "npm": "@ai-sdk/openai",
                        "options": {
                            "baseURL": "http://127.0.0.1:16888/v1",
                            "apiKey": "sk-legacy",
                            "timeout": 60000,
                            "setCacheKey": true
                        },
                        "models": {
                            "gpt": {
                                "name": "GPT",
                                "attachment": true,
                                "reasoning": true,
                                "tool_call": true,
                                "modalities": { "input": ["text"], "output": ["text"] },
                                "limit": { "context": 1000, "output": 100 },
                                "options": {
                                    "reasoningEffort": "medium",
                                    "textVerbosity": "low",
                                    "thinking": { "type": "enabled", "budgetTokens": 2048 }
                                },
                                "variants": { "high": { "reasoningEffort": "high" } }
                            }
                        }
                    }
                },
                "model": "router/gpt"
            })
            .to_string(),
        )
        .unwrap();

        // 迁移前：get_config 上报待迁移项，前端据此渲染「迁移」按钮
        let before = view();
        let info = before
            .legacy_migration
            .as_ref()
            .expect("应上报 1.x 待迁移项");
        assert_eq!(info.providers, vec!["router"]);
        assert!(info.auth_providers.is_empty());

        let res = migrate_legacy_config().unwrap();
        assert!(res.ok);
        assert!(res.message.contains("1 个 1.x 供应商"), "{}", res.message);

        let raw = load_root(&h);
        assert!(
            raw.get(LEGACY_PROVIDERS_KEY).is_none(),
            "1.x 容器必须被清空移除"
        );
        assert!(raw.pointer("/provider").is_none());
        // 供应商：npm → package（归一 aisdk: 前缀）、options → settings
        assert_eq!(
            raw.pointer("/providers/router/package")
                .and_then(|x| x.as_str()),
            Some("aisdk:@ai-sdk/openai")
        );
        assert_eq!(
            raw.pointer("/providers/router/settings/baseURL")
                .and_then(|x| x.as_str()),
            Some("http://127.0.0.1:16888/v1")
        );
        assert_eq!(
            raw.pointer("/providers/router/settings/apiKey")
                .and_then(|x| x.as_str()),
            Some("sk-legacy")
        );
        assert_eq!(
            raw.pointer("/providers/router/settings/timeout")
                .and_then(|x| x.as_i64()),
            Some(60000)
        );
        // 1.x 专有字段不保留（setCacheKey / options / modalities / tool_call / attachment）
        assert!(raw.pointer("/providers/router/options").is_none());
        let model = raw.pointer("/providers/router/models/gpt").unwrap();
        assert!(model.get("options").is_none());
        assert!(model.get("modalities").is_none());
        assert!(model.get("tool_call").is_none());
        assert!(model.get("attachment").is_none());
        // 模型：limit / capabilities / settings / variants（数组）
        assert_eq!(
            raw.pointer("/providers/router/models/gpt/limit/context")
                .and_then(|x| x.as_f64()),
            Some(1000.0)
        );
        assert_eq!(
            raw.pointer("/providers/router/models/gpt/capabilities/input"),
            Some(&json!(["text", "image"]))
        );
        assert_eq!(
            raw.pointer("/providers/router/models/gpt/capabilities/tools")
                .and_then(|x| x.as_bool()),
            Some(true)
        );
        assert_eq!(
            raw.pointer("/providers/router/models/gpt/capabilities/reasoning/supported")
                .and_then(|x| x.as_bool()),
            Some(true)
        );
        assert_eq!(
            raw.pointer("/providers/router/models/gpt/capabilities/reasoning/budget/max")
                .and_then(|x| x.as_u64()),
            Some(2048)
        );
        assert_eq!(
            raw.pointer("/providers/router/models/gpt/settings/reasoningEffort")
                .and_then(|x| x.as_str()),
            Some("medium")
        );
        assert_eq!(
            raw.pointer("/providers/router/models/gpt/settings/textVerbosity")
                .and_then(|x| x.as_str()),
            Some("low")
        );
        assert_eq!(
            raw.pointer("/providers/router/models/gpt/variants/0/id")
                .and_then(|x| x.as_str()),
            Some("high")
        );
        assert_eq!(
            raw.pointer("/providers/router/models/gpt/variants/0/settings/reasoningEffort")
                .and_then(|x| x.as_str()),
            Some("high")
        );

        // 迁移后视图：待迁移项清空、1.x 警告消失、密钥来源变成配置文件
        let after = res.view.expect("迁移后应返回刷新后的视图");
        assert!(after.legacy_migration.is_none());
        assert!(after.warnings.iter().all(|w| !w.contains("1.x")));
        let provider = after
            .providers
            .iter()
            .find(|p| p.id == "router")
            .expect("迁移后供应商可见");
        assert_eq!(provider.api_key_source, "config");
        assert_eq!(provider.models[0].modalities_input, vec!["text", "image"]);
        assert_eq!(after.model.as_deref(), Some("router/gpt"));
    }

    #[test]
    fn migrate_legacy_drops_duplicate_id_and_keeps_v2_entry() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": {
                    "p": { "name": "legacy", "npm": "@ai-sdk/openai", "options": { "baseURL": "http://legacy/v1" } }
                },
                "providers": {
                    "p": { "name": "native", "package": "aisdk:@ai-sdk/openai", "settings": { "baseURL": "http://native/v1" } }
                }
            })
            .to_string(),
        )
        .unwrap();

        let res = migrate_legacy_config().unwrap();
        assert!(res.message.contains("重复"), "{}", res.message);

        let raw = load_root(&h);
        assert!(raw.get(LEGACY_PROVIDERS_KEY).is_none());
        assert_eq!(
            raw.pointer("/providers/p/name").and_then(|x| x.as_str()),
            Some("native")
        );
        assert_eq!(
            raw.pointer("/providers/p/settings/baseURL")
                .and_then(|x| x.as_str()),
            Some("http://native/v1")
        );
        let v = view();
        assert_eq!(v.providers.len(), 1);
        assert!(v.legacy_migration.is_none());
    }

    #[test]
    fn migrate_legacy_moves_auth_only_key_into_config() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "providers": {
                    "oldprov": {
                        "name": "Old",
                        "package": "aisdk:@ai-sdk/openai",
                        "settings": { "baseURL": "http://x/v1" }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            h.auth_file(),
            r#"{"oldprov": {"type": "api", "key": "sk-legacy"}}"#,
        )
        .unwrap();

        let info = view().legacy_migration.expect("auth.json 待迁移项");
        assert!(info.providers.is_empty());
        assert_eq!(info.auth_providers, vec!["oldprov"]);

        let res = migrate_legacy_config().unwrap();
        assert!(res.message.contains("auth.json"), "{}", res.message);

        let raw = load_root(&h);
        assert_eq!(
            raw.pointer("/providers/oldprov/settings/apiKey")
                .and_then(|x| x.as_str()),
            Some("sk-legacy")
        );
        let auth: Value =
            serde_json::from_str(&fs::read_to_string(h.auth_file()).unwrap()).unwrap();
        assert!(auth.get("oldprov").is_none());

        let after = res.view.expect("迁移后应返回刷新后的视图");
        assert!(after.legacy_migration.is_none());
        assert!(after.warnings.iter().all(|w| !w.contains("auth.json")));
        let p = after.providers.iter().find(|p| p.id == "oldprov").unwrap();
        assert_eq!(p.api_key_source, "config");
        assert_eq!(get_provider_secret("oldprov".into()).unwrap(), "sk-legacy");
    }

    #[test]
    fn migrate_legacy_is_idempotent() {
        let h = TempHome::new();
        fs::write(
            h.config_file(),
            json!({
                "provider": { "p": { "name": "P", "npm": "@ai-sdk/openai", "options": { "baseURL": "http://x/v1" } } }
            })
            .to_string(),
        )
        .unwrap();

        migrate_legacy_config().unwrap();
        let after_first = fs::read_to_string(h.config_file()).unwrap();
        let res = migrate_legacy_config().unwrap();
        assert_eq!(res.message, "没有可迁移的 OpenCode 1.x 配置");
        assert_eq!(fs::read_to_string(h.config_file()).unwrap(), after_first);
        assert!(res.view.unwrap().legacy_migration.is_none());
    }

    #[test]
    fn migrate_legacy_without_config_file_is_noop() {
        let _h = TempHome::new();
        let res = migrate_legacy_config().unwrap();
        assert_eq!(res.message, "没有可迁移的 OpenCode 1.x 配置");
        assert!(res.view.unwrap().providers.is_empty());
    }

    #[test]
    fn normalize_package_prefixes_ai_sdk() {
        assert_eq!(normalize_package("@ai-sdk/openai"), "aisdk:@ai-sdk/openai");
        assert_eq!(
            normalize_package("aisdk:@ai-sdk/openai"),
            "aisdk:@ai-sdk/openai"
        );
        assert_eq!(
            normalize_package("@opencode-ai/plugin"),
            "@opencode-ai/plugin"
        );
        assert_eq!(normalize_package(""), "");
    }

    #[test]
    fn model_ref_reader_accepts_string_and_object() {
        assert_eq!(
            read_model_ref(Some(&json!("prov/model#high"))).as_deref(),
            Some("prov/model#high")
        );
        assert_eq!(
            read_model_ref(Some(&json!({"providerID":"prov","model":"model"}))).as_deref(),
            Some("prov/model")
        );
        assert_eq!(
            read_model_ref(Some(
                &json!({"providerID":"prov","model":"model","variant":"high"})
            ))
            .as_deref(),
            Some("prov/model#high")
        );
        assert!(read_model_ref(None).is_none());
        assert!(read_model_ref(Some(&json!(null))).is_none());
    }
}
