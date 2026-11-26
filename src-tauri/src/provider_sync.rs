//! Provider sources and native configuration reconciliation. No remote discovery.
use crate::{
    agent_model_config::ModelConfigAgent, ai_provider, db, opencode_config as oc,
    pi_model_config as pi,
};
use serde::Serialize;
use std::{
    collections::HashSet,
    sync::{Mutex, MutexGuard},
};
pub const ROUTE_ID: &str = "__route_aggregation__";
static WRITE_LOCK: Mutex<()> = Mutex::new(());
pub fn lock() -> Result<MutexGuard<'static, ()>, String> {
    // Poison only means a synced writer panicked while holding the guard. This mutex
    // orders writes instead of protecting invariants, so recover and keep retrying.
    Ok(WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceModel {
    pub id: String,
    pub original: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub id: String,
    pub name: String,
    pub models: Vec<SourceModel>,
}
pub struct Source {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub key: String,
    pub models: Vec<SourceModel>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub target: String,
    pub ok: bool,
    pub message: String,
}
pub fn protocol(api: Option<&str>) -> Result<&'static str, String> {
    match api {
        Some("anthropic" | "anthropic-messages") => Ok("anthropic"),
        Some("openai-responses") => Ok("openai_responses"),
        Some("openai" | "openai-completions" | "openai-chat-completions") => Ok("openai_chat"),
        _ => Err("当前协议不支持绑定供应商".into()),
    }
}
fn models_of(raw: &str) -> Result<Vec<SourceModel>, String> {
    let entries: Vec<ai_provider::CustomModel> =
        serde_json::from_str(raw).map_err(|_| "供应商模型列表格式无效".to_string())?;
    let mut seen = HashSet::new();
    Ok(entries
        .into_iter()
        .filter_map(|m| {
            let id = if m.alias_id.trim().is_empty() {
                m.model.trim()
            } else {
                m.alias_id.trim()
            }
            .to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                None
            } else {
                Some(SourceModel {
                    id,
                    original: m.model.trim().to_string(),
                })
            }
        })
        .collect())
}
pub fn resolve(id: &str, api: &str, secret: bool) -> Result<Source, String> {
    if !["anthropic", "openai_chat", "openai_responses"].contains(&api) {
        return Err("不支持的来源协议".into());
    }
    if id == ROUTE_ID {
        let config = crate::route_aggregation::config::load_config()?;
        let toggles = db::load_provider_route_toggles()?;
        let mut seen = HashSet::new();
        let mut models = Vec::new();
        for row in db::load_ai_provider_rows()? {
            if !ai_provider::provider_types_contain(&row.provider_type, api)
                || toggles
                    .iter()
                    .any(|t| t.provider_id == row.id && !t.enabled)
            {
                continue;
            }
            for model in models_of(&row.custom_models_json)? {
                if seen.insert(model.id.clone()) {
                    models.push(model);
                }
            }
        }
        let base = format!("http://{}:{}", config.listen_address, config.listen_port);
        return Ok(Source {
            id: id.into(),
            name: "路由聚合".into(),
            base_url: if api == "anthropic" {
                base
            } else {
                format!("{base}/v1")
            },
            key: if secret {
                config.api_keys.first().cloned().unwrap_or_default()
            } else {
                String::new()
            },
            models,
        });
    }
    if db::provider_sync_states()?
        .iter()
        .any(|(s, d, _, _)| s == id && *d)
    {
        return Err("供应商正在删除".into());
    }
    let row = db::get_ai_provider_row(id)?.ok_or("供应商不存在")?;
    if !ai_provider::provider_types_contain(&row.provider_type, api) {
        return Err("供应商不支持当前 API 协议".into());
    }
    let key = if secret && !row.api_key_cipher.is_empty() {
        ai_provider::get_provider_secret(id.into())?
    } else {
        String::new()
    };
    Ok(Source {
        id: id.into(),
        name: row.name,
        base_url: if api == "anthropic" {
            row.base_url
        } else {
            row.openai_base_url
        },
        key,
        models: models_of(&row.custom_models_json)?,
    })
}
pub fn source_view(id: &str, api: &str) -> Result<SourceView, String> {
    let s = resolve(id, api, false)?;
    Ok(SourceView {
        id: s.id,
        name: s.name,
        models: s.models,
    })
}
pub fn get_agent(agent: ModelConfigAgent) -> Result<oc::AgentModelConfigView, String> {
    let mut view = match agent {
        ModelConfigAgent::Opencode => oc::get_config()?,
        a => pi::get_config(a)?,
    };
    for p in &mut view.providers {
        match db::agent_target_source(agent.id(), &p.id) {
            Ok(s) => {
                p.source_id = s;
                if p.source_id.is_some() && p.models.is_empty() {
                    view.warnings.push(format!(
                        "供应商 {} 的绑定模型列表为空，请配置来源模型",
                        p.id
                    ));
                }
            }
            Err(e) => view.warnings.push(e),
        }
    }
    Ok(view)
}
fn agent_sync_view(agent: ModelConfigAgent) -> Result<oc::AgentModelConfigView, String> {
    match agent {
        ModelConfigAgent::Opencode => oc::get_config_for_sync(),
        a => pi::get_config_for_sync(a),
    }
}
pub fn write_provider(
    agent: ModelConfigAgent,
    p: oc::UpsertProviderPayload,
) -> Result<oc::AgentActionResult, String> {
    match agent {
        ModelConfigAgent::Opencode => oc::upsert_provider(p),
        a => pi::upsert_provider(a, p),
    }
}
/// Match a model against the local Models.dev cache with the same merging rule as the
/// model editor's "从 Models.dev 补全" button: identity fields come from the strict
/// provider hit (falling back to the first hit), while limits and modalities are the
/// union across every provider that lists this model id. Aggregator entries often carry
/// a model without any limit data, so bailing out on multiple hits would silently drop
/// context/output numbers and modality chips during auto-import.
pub fn catalog_model(provider: &str, model: &str) -> Option<oc::CatalogModelSummary> {
    let catalog = oc::local_catalog()?;
    if model.is_empty() {
        return None;
    }
    let matches: Vec<(&str, &oc::CatalogModelSummary)> = catalog
        .providers
        .iter()
        .filter_map(|p| {
            p.models
                .iter()
                .find(|m| m.id == model)
                .map(|m| (p.id.as_str(), m))
        })
        .take(65)
        .collect();
    if matches.is_empty() {
        return None;
    }
    let identity = matches
        .iter()
        .find(|(id, _)| *id == provider)
        .map_or(matches[0].1, |(_, m)| *m);
    let first_number =
        |get: fn(&oc::CatalogModelSummary) -> Option<f64>| matches.iter().find_map(|(_, m)| get(m));
    Some(oc::CatalogModelSummary {
        id: identity.id.clone(),
        name: identity.name.clone(),
        limit_context: first_number(|m| m.limit_context),
        limit_input: first_number(|m| m.limit_input),
        limit_output: first_number(|m| m.limit_output),
        modalities_input: union_strings(matches.iter().map(|(_, m)| &m.modalities_input)),
        modalities_output: union_strings(matches.iter().map(|(_, m)| &m.modalities_output)),
        reasoning: identity.reasoning,
        reasoning_options: identity.reasoning_options.clone(),
        tool_call: identity.tool_call,
        attachment: identity.attachment,
        status: identity.status.clone(),
    })
}

/// Deduplicated union that keeps first-seen order, ignoring empty entries.
fn union_strings<'a>(lists: impl Iterator<Item = &'a Vec<String>>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for list in lists {
        for value in list {
            if seen.insert(value.clone()) {
                out.push(value.clone());
            }
        }
    }
    out
}
fn repair_slot(slot: Option<&str>, provider: &str, models: &[SourceModel]) -> Option<String> {
    let model = slot?.strip_prefix(&format!("{provider}/"))?;
    let base_model = model.split_once('#').map(|(id, _)| id).unwrap_or(model);
    if models.iter().any(|m| m.id == model || m.id == base_model) {
        return None;
    }
    Some(
        models
            .first()
            .map(|m| format!("{provider}/{}", m.id))
            .unwrap_or_default(),
    )
}
fn sync_agent(
    id: &str,
    agent: ModelConfigAgent,
    target: &str,
    deleting: bool,
) -> Result<(), String> {
    let view = agent_sync_view(agent)?;
    let provider = view.providers.iter().find(|p| p.id == target);
    let source = if deleting {
        None
    } else {
        let provider = provider
            .ok_or_else(|| format!("{}/{target} 配置不存在，等待恢复或解绑", agent.id()))?;
        Some(resolve(id, protocol(provider.api.as_deref())?, true)?)
    };
    let models = source.as_ref().map(|s| s.models.as_slice()).unwrap_or(&[]);
    if let Some(ref s) = source {
        let payload = serde_json::from_value(
            serde_json::json!({"id":target,"name":s.name,"baseUrl":s.base_url,"apiKey":s.key}),
        )
        .map_err(|e| e.to_string())?;
        write_provider(agent, payload)?;
        match agent {
            ModelConfigAgent::Opencode => oc::sync_model_set(target, models)?,
            a => pi::sync_model_set(a, target, models)?,
        }
    }
    let main = repair_slot(view.model.as_deref(), target, models);
    let small = repair_slot(view.small_model.as_deref(), target, models);
    let profile = repair_slot(view.omo_model_profile.as_deref(), target, models);
    if main.is_some() || small.is_some() {
        let payload = oc::SetDefaultsPayload {
            model: main,
            small_model: small,
            enabled_providers: None,
            disabled_providers: None,
            sync_omo_model_profile: None,
        };
        match agent {
            ModelConfigAgent::Opencode => {
                oc::set_defaults(payload)?;
            }
            a => {
                pi::set_defaults(a, payload)?;
            }
        }
    }
    if let Some(value) = profile {
        pi::sync_omo_profile(&value)?;
    }
    if deleting {
        match agent {
            ModelConfigAgent::Opencode => {
                oc::delete_provider(target.into(), true)?;
            }
            a => {
                pi::delete_provider(a, target.into(), true)?;
            }
        }
        db::clear_agent_target_binding(agent.id(), target)?;
    }
    Ok(())
}
fn item(target: String, result: Result<(), String>) -> SyncResult {
    match result {
        Ok(()) => SyncResult {
            target,
            ok: true,
            message: "已同步".into(),
        },
        Err(message) => SyncResult {
            target,
            ok: false,
            message,
        },
    }
}
fn reconcile_target(
    source: &str,
    target: &str,
    operation: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let (result, files) = crate::config_sync_io::reconcile(operation);
    for (path, status) in files {
        db::record_sync_file(source, target, &path.to_string_lossy(), &status)?;
    }
    if result.is_ok() {
        db::clear_target_file_errors(source, target)?;
    }
    result
}
fn complete_binding_intent(
    source: &str,
    agent: ModelConfigAgent,
    target: &str,
    raw: &str,
) -> Result<(), String> {
    let mut payload: oc::UpsertProviderPayload =
        serde_json::from_str(raw).map_err(|_| "绑定恢复记录格式无效".to_string())?;
    let api = if agent == ModelConfigAgent::Opencode {
        payload.package.as_deref().map(|p| {
            if p.contains("anthropic") {
                "anthropic"
            } else {
                "openai"
            }
        })
    } else {
        payload.api.as_deref()
    };
    let source = resolve(source, protocol(api)?, true)?;
    payload.name = Some(source.name);
    payload.base_url = Some(source.base_url);
    payload.api_key = Some(source.key);
    write_provider(agent, payload)?;
    db::finish_binding_intent(agent.id(), target)
}
pub fn run_source(id: &str) -> Result<Vec<SyncResult>, String> {
    let deleting = db::provider_sync_states()?
        .iter()
        .any(|(s, d, _, _)| s == id && *d);
    let mut results = Vec::new();
    let mut failed_intents = HashSet::new();
    if !deleting {
        for (agent, target, raw) in db::binding_intents(id)? {
            let label = format!("{agent}/{target}");
            let result = ModelConfigAgent::parse(&agent).and_then(|a| {
                reconcile_target(id, &label, || complete_binding_intent(id, a, &target, &raw))
            });
            if result.is_err() {
                failed_intents.insert((agent, target));
                results.push(item(label, result));
            }
        }
    }
    for env in db::load_claude_envs_by_provider(id)? {
        let result = if std::path::Path::new(&env.config_dir).is_dir() {
            if deleting {
                reconcile_target(id, &format!("Claude/{}", env.name), || {
                    crate::claude_env::sync_bound_source(&env.config_dir, None)
                })
            } else {
                resolve(id, "anthropic", true).and_then(|s| {
                    reconcile_target(id, &format!("Claude/{}", env.name), || {
                        crate::claude_env::sync_bound_source(&env.config_dir, Some(&s))
                    })
                })
            }
        } else if deleting {
            Ok(())
        } else {
            Err("配置目录未初始化，等待初始化后同步".into())
        };
        results.push(item(format!("Claude/{}", env.name), result));
    }
    for env in db::load_codex_envs_by_provider(id)? {
        let result = if std::path::Path::new(&env.config_dir).is_dir() {
            if deleting {
                reconcile_target(id, &format!("Codex/{}", env.name), || {
                    crate::codex_env::sync_bound_source(&env.config_dir, None)
                })
            } else {
                resolve(id, "openai_responses", true).and_then(|s| {
                    reconcile_target(id, &format!("Codex/{}", env.name), || {
                        crate::codex_env::sync_bound_source(&env.config_dir, Some(&s))
                    })
                })
            }
        } else if deleting {
            Ok(())
        } else {
            Err("配置目录未初始化，等待初始化后同步".into())
        };
        results.push(item(format!("Codex/{}", env.name), result));
    }
    for b in db::load_agent_provider_bindings(id)? {
        if failed_intents.contains(&(b.agent_type.clone(), b.agent_provider_id.clone())) {
            continue;
        }
        results.push(item(
            format!("{}/{}", b.agent_type, b.agent_provider_id),
            ModelConfigAgent::parse(&b.agent_type).and_then(|a| {
                reconcile_target(
                    id,
                    &format!("{}/{}", b.agent_type, b.agent_provider_id),
                    || sync_agent(id, a, &b.agent_provider_id, deleting),
                )
            }),
        ));
    }
    let error = results
        .iter()
        .filter(|r| !r.ok)
        .map(|r| format!("{}：{}", r.target, r.message))
        .collect::<Vec<_>>()
        .join("；");
    if deleting && error.is_empty() {
        db::delete_ai_provider_row(id)?;
    } else {
        db::finish_provider_sync(id, &error)?;
    }
    Ok(results)
}
pub fn run_pending() -> Result<Vec<SyncResult>, String> {
    let mut results = Vec::new();
    for (id, _, pending, _) in db::provider_sync_states()? {
        if pending && db::provider_sync_due(&id)? {
            match run_source(&id) {
                Ok(items) => results.extend(items),
                Err(error) => {
                    db::finish_provider_sync(&id, &error)?;
                    results.push(item(id, Err(error)));
                }
            }
        }
    }
    Ok(results)
}
pub fn validate_change(id: &str, types: &[String]) -> Result<(), String> {
    if !db::load_claude_envs_by_provider(id)?.is_empty() && !types.iter().any(|t| t == "anthropic")
    {
        return Err("供应商仍被 Claude 环境引用，不能移除 Anthropic 协议".into());
    }
    if !db::load_codex_envs_by_provider(id)?.is_empty()
        && !types.iter().any(|t| t == "openai_responses")
    {
        return Err("供应商仍被 Codex 环境引用，不能移除 Responses 协议".into());
    }
    for b in db::load_agent_provider_bindings(id)? {
        let view = agent_sync_view(ModelConfigAgent::parse(&b.agent_type)?)?;
        let p = view
            .providers
            .iter()
            .find(|p| p.id == b.agent_provider_id)
            .ok_or("绑定的 Agent 供应商不存在，请先解除绑定")?;
        if !types
            .iter()
            .any(|t| t == protocol(p.api.as_deref()).unwrap_or("unsupported"))
        {
            return Err(format!(
                "供应商仍被 {}/{} 引用，不能移除该协议",
                b.agent_type, b.agent_provider_id
            ));
        }
    }
    Ok(())
}
pub fn impact(id: &str) -> Result<Vec<String>, String> {
    let mut items = Vec::new();
    for e in db::load_claude_envs_by_provider(id)? {
        items.push(format!(
            "Claude 环境「{}」：{}（保留环境，清除连接和模型）",
            e.name, e.config_dir
        ));
    }
    for e in db::load_codex_envs_by_provider(id)? {
        items.push(format!(
            "Codex 环境「{}」：{}（保留环境，清除连接和模型）",
            e.name, e.config_dir
        ));
    }
    for b in db::load_agent_provider_bindings(id)? {
        let view = get_agent(ModelConfigAgent::parse(&b.agent_type)?)?;
        items.push(format!(
            "{}/{}：{}（删除供应商块）",
            b.agent_type, b.agent_provider_id, view.config_path
        ));
    }
    for binding in db::load_agent_provider_bindings(ROUTE_ID)? {
        let view = agent_sync_view(ModelConfigAgent::parse(&binding.agent_type)?)?;
        items.push(format!(
            "路由绑定 {}/{}：{}（重新计算模型，不删除路由供应商块）",
            binding.agent_type, binding.agent_provider_id, view.config_path
        ));
    }
    for env in db::load_claude_envs_by_provider(ROUTE_ID)? {
        items.push(format!(
            "路由绑定 Claude/{}：{}（重新校验模型）",
            env.name, env.config_dir
        ));
    }
    for env in db::load_codex_envs_by_provider(ROUTE_ID)? {
        items.push(format!(
            "路由绑定 Codex/{}：{}（重新校验模型）",
            env.name, env.config_dir
        ));
    }
    items.push("路由聚合将移除此供应商并重新计算绑定模型".into());
    Ok(items)
}
pub fn save_agent_provider(
    agent: ModelConfigAgent,
    mut payload: oc::UpsertProviderPayload,
    source_id: Option<String>,
) -> Result<oc::AgentActionResult, String> {
    let _guard = lock()?;
    let old = payload
        .previous_id
        .clone()
        .unwrap_or_else(|| payload.id.clone());
    let existing = match db::agent_target_source(agent.id(), &old) {
        Ok(value) => value,
        Err(error) if source_id.is_none() => return Err(error),
        Err(_) => None,
    };
    let id = source_id.or(existing).filter(|s| !s.is_empty());
    if let Some(ref id) = id {
        let api = if agent == ModelConfigAgent::Opencode {
            payload.package.as_deref().map(|p| {
                if p.contains("anthropic") {
                    "anthropic"
                } else {
                    "openai"
                }
            })
        } else {
            payload.api.as_deref()
        };
        let view = get_agent(agent)?;
        let old_api = view
            .providers
            .iter()
            .find(|p| p.id == old)
            .and_then(|p| p.api.as_deref());
        let s = resolve(id, protocol(api.or(old_api))?, true)?;
        if agent == ModelConfigAgent::Opencode && payload.package.is_none() {
            payload.package = view
                .providers
                .iter()
                .find(|p| p.id == old)
                .and_then(|p| p.package.clone());
        }
        if agent != ModelConfigAgent::Opencode && payload.api.is_none() {
            payload.api = old_api.map(str::to_string);
        }
        payload.name = Some(s.name);
        payload.base_url = Some(s.base_url);
        payload.api_key = Some(s.key);
    }
    let target = payload.id.trim().to_string();
    if target.is_empty() || target.contains('/') || target.contains(' ') {
        return Err("供应商 ID 不能为空或包含空格、/".into());
    }
    if old != target
        && agent_sync_view(agent)?
            .providers
            .iter()
            .any(|p| p.id == target)
    {
        return Err("目标供应商 ID 已存在，不能覆盖".into());
    }
    let mut result;
    if let Some(id) = id {
        // Do not persist the key, even encrypted source credentials are resolved at replay time.
        let safe = serde_json::json!({"id": target, "previousId": payload.previous_id, "package": payload.package, "api": payload.api, "timeout": payload.timeout, "chunkTimeout": payload.chunk_timeout});
        db::begin_binding_intent(&id, agent.id(), &target, &old, &safe.to_string())?;
        let items = run_source(&id)?;
        let failures = items
            .iter()
            .filter(|r| !r.ok)
            .map(|r| format!("{}：{}", r.target, r.message))
            .collect::<Vec<_>>();
        result = oc::AgentActionResult {
            ok: true,
            message: if failures.is_empty() {
                "供应商已绑定，全部模型已自动同步".into()
            } else {
                format!(
                    "绑定已保存，部分文件同步失败（将自动恢复）：{}",
                    failures.join("；")
                )
            },
            view: None,
        };
    } else {
        result = write_provider(agent, payload)?;
        if old != target {
            db::clear_agent_target_binding(agent.id(), &old)?;
        }
        db::clear_agent_target_binding(agent.id(), &target)?;
    }
    match get_agent(agent) {
        Ok(view) => result.view = Some(view),
        Err(_) => result
            .message
            .push_str("；目标配置无法读取，修复文件后后台将继续同步"),
    };
    Ok(result)
}
fn matching_sources(base: &str, api: &str) -> Result<Vec<String>, String> {
    let norm = |s: &str| s.trim().trim_end_matches('/').to_string();
    let base = norm(base);
    if base.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = db::load_ai_provider_rows()?
        .into_iter()
        .filter(|p| {
            ai_provider::provider_types_contain(&p.provider_type, api)
                && norm(if api == "anthropic" {
                    &p.base_url
                } else {
                    &p.openai_base_url
                }) == base
        })
        .map(|p| p.id)
        .collect::<Vec<_>>();
    if resolve(ROUTE_ID, api, false)
        .map(|s| norm(&s.base_url) == base)
        .unwrap_or(false)
    {
        ids.push(ROUTE_ID.into());
    }
    Ok(ids)
}
fn migrate_claude(mut env: crate::claude_env::ClaudeEnvironmentRow) -> Result<(), String> {
    let target = format!("claude:{}", env.id);
    if db::binding_migrated(&target)? {
        return Ok(());
    }
    if env.provider_id.is_empty() {
        let path = std::path::Path::new(&env.config_dir).join("settings.json");
        if !path.exists() {
            return Ok(());
        }
        let root: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .map_err(|_| format!("读取迁移配置失败：{}", path.display()))?,
        )
        .map_err(|_| format!("迁移配置格式无效：{}", path.display()))?;
        if !root.is_object() {
            return Err(format!("迁移配置根节点不是对象：{}", path.display()));
        }
        let ids = matching_sources(
            root.pointer("/env/ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            "anthropic",
        )?;
        if ids.len() == 1 {
            env.provider_id = ids[0].clone();
            db::upsert_claude_environment_row(&env)?;
        } else if ids.len() > 1 {
            return Err(format!(
                "{target} 匹配多个供应商，请在环境编辑中明确选择来源"
            ));
        }
    }
    if !env.provider_id.is_empty() {
        db::mark_provider_sync(&env.provider_id, false)?;
    }
    db::mark_binding_migrated(&target)
}
fn migrate_codex(mut env: crate::codex_env::CodexEnvironmentRow) -> Result<(), String> {
    let target = format!("codex:{}", env.id);
    if db::binding_migrated(&target)? {
        return Ok(());
    }
    if env.provider_id.is_empty() {
        let path = std::path::Path::new(&env.config_dir).join("config.toml");
        if !path.exists() {
            return Ok(());
        }
        let doc: toml_edit::DocumentMut = std::fs::read_to_string(&path)
            .map_err(|_| format!("读取迁移配置失败：{}", path.display()))?
            .parse()
            .map_err(|_| format!("迁移配置格式无效：{}", path.display()))?;
        let active = doc.get("model_provider").and_then(|v| v.as_str());
        let base = active
            .and_then(|id| {
                doc.get("model_providers")
                    .and_then(|v| v.get(id))
                    .and_then(|v| v.get("base_url"))
                    .and_then(|v| v.as_str())
            })
            .or_else(|| doc.get("openai_base_url").and_then(|v| v.as_str()))
            .unwrap_or_default();
        let ids = matching_sources(base, "openai_responses")?;
        if ids.len() == 1 {
            env.provider_id = ids[0].clone();
            db::upsert_codex_environment_row(&env)?;
        } else if ids.len() > 1 {
            return Err(format!(
                "{target} 匹配多个供应商，请在环境编辑中明确选择来源"
            ));
        }
    }
    if !env.provider_id.is_empty()
        && env.provider_id != crate::codex_env::OFFICIAL_OAUTH_PROVIDER_ID
    {
        db::mark_provider_sync(&env.provider_id, false)?;
    }
    db::mark_binding_migrated(&target)
}
fn migrate_agent(agent: ModelConfigAgent) -> Result<(), String> {
    let view = agent_sync_view(agent)?;
    let mut errors = Vec::new();
    for provider in view.providers {
        let target = format!("agent:{}:{}", agent.id(), provider.id);
        let result = (|| -> Result<(), String> {
            if db::binding_migrated(&target)? {
                return Ok(());
            }
            let existing = db::agent_target_source(agent.id(), &provider.id)?;
            let source = if existing.is_some() {
                existing
            } else if let Ok(api) = protocol(provider.api.as_deref()) {
                let ids = matching_sources(provider.base_url.as_deref().unwrap_or_default(), api)?;
                if ids.len() == 1 {
                    Some(ids[0].clone())
                } else if ids.len() > 1 {
                    return Err(format!("{target} 匹配多个供应商，请重新选择绑定"));
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(id) = source {
                db::upsert_agent_provider_binding(&id, agent.id(), &provider.id)?;
                db::mark_provider_sync(&id, false)?;
            }
            db::mark_binding_migrated(&target)
        })();
        if let Err(error) = result {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}
/// Every target is isolated: a malformed file must not prevent healthy targets
/// from binding or applying their already queued source updates.
pub fn migrate_bindings() -> Result<(), String> {
    let mut errors = Vec::new();
    for env in db::load_claude_environment_rows()? {
        if let Err(error) = migrate_claude(env) {
            errors.push(error);
        }
    }
    for env in db::load_codex_environment_rows()? {
        if let Err(error) = migrate_codex(env) {
            errors.push(error);
        }
    }
    for agent in [
        ModelConfigAgent::Opencode,
        ModelConfigAgent::Pi,
        ModelConfigAgent::OhMyPi,
        ModelConfigAgent::Omo,
    ] {
        if let Err(error) = migrate_agent(agent) {
            errors.push(format!("{}：{error}", agent.id()));
        }
    }
    // Persist only human-readable failures, never parsed settings or credentials.
    db::set_binding_migration_error(&errors.join("；"))?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    struct Home {
        path: PathBuf,
        old: Option<std::ffi::OsString>,
        old_path: Option<std::ffi::OsString>,
        _guard: MutexGuard<'static, ()>,
    }
    impl Home {
        fn new() -> Self {
            let guard = crate::config::lock_home_for_test();
            let path =
                std::env::temp_dir().join(format!("octopus-source-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            let old = std::env::var_os("HOME");
            std::env::set_var("HOME", &path);
            let old_path = std::env::var_os("PATH");
            let bin = path.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            for agent in ["pi", "omp", "omo", "opencode"] {
                let executable = bin.join(if cfg!(windows) {
                    format!("{agent}.exe")
                } else {
                    agent.to_string()
                });
                std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
            }
            std::env::set_var("PATH", &bin);
            Self {
                path,
                old,
                old_path,
                _guard: guard,
            }
        }
        fn write(&self, relative: &str, value: Value) {
            let path = self.path.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        }
        fn read(&self, relative: &str) -> Value {
            serde_json::from_slice(&std::fs::read(self.path.join(relative)).unwrap()).unwrap()
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            if let Some(ref old) = self.old {
                std::env::set_var("HOME", old);
            } else {
                std::env::remove_var("HOME");
            }
            if let Some(ref old) = self.old_path {
                std::env::set_var("PATH", old);
            } else {
                std::env::remove_var("PATH");
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    fn payload(id: &str, models: &[&str]) -> ai_provider::AiProviderUpsertPayload {
        serde_json::from_value(json!({"id":id,"name":id,"providerTypes":["anthropic","openai_chat","openai_responses"],"baseUrl":"https://example.invalid","openaiBaseUrl":"https://example.invalid/v1","apiKey":"test-placeholder","customModels":models.iter().map(|m|json!({"model":m,"aliasId":""})).collect::<Vec<_>>()})).unwrap()
    }
    fn source(models: &[&str]) -> Source {
        Source {
            id: "a".into(),
            name: "a".into(),
            base_url: "https://example.invalid".into(),
            key: "test-placeholder".into(),
            models: models
                .iter()
                .map(|m| SourceModel {
                    id: (*m).into(),
                    original: (*m).into(),
                })
                .collect(),
        }
    }
    #[test]
    fn aliases_dedup_order_invalid_list_and_defaults() {
        let list=models_of(r#"[{"model":"original","aliasId":" alias "},{"model":"other","aliasId":"alias"},{"model":" second ","aliasId":""}]"#).unwrap();
        assert_eq!(
            list.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["alias", "second"]
        );
        assert!(models_of("broken").is_err());
        assert_eq!(
            repair_slot(Some("a/removed"), "a", &list),
            Some("a/alias".into())
        );
        assert_eq!(repair_slot(Some("a/alias"), "a", &list), None);
        assert_eq!(repair_slot(Some("b/removed"), "a", &list), None);
        assert_eq!(
            repair_slot(Some("a/removed"), "a", &[]),
            Some(String::new())
        );
    }
    #[test]
    fn pi_models_reconcile_preserving_parameters_and_other_providers() {
        let home = Home::new();
        ai_provider::upsert_provider(payload("a", &["one", "two"])).unwrap();
        home.write(".pi/agent/models.json",json!({"unrelated":true,"providers":{"a":{"api":"openai-completions","models":[{"id":"one","name":"custom","contextWindow":12345,"unknown":{"keep":true}},{"id":"local"}]},"b":{"api":"openai-completions","models":[{"id":"untouched"}]}}}));
        home.write(
            ".pi/agent/settings.json",
            json!({"defaultProvider":"a","defaultModel":"local","theme":"dark"}),
        );
        save_agent_provider(
            ModelConfigAgent::Pi,
            serde_json::from_value(json!({"id":"a","api":"openai-completions"})).unwrap(),
            Some("a".into()),
        )
        .unwrap();
        assert_eq!(
            get_agent(ModelConfigAgent::Pi).unwrap().model.as_deref(),
            Some("a/one")
        );
        let root = home.read(".pi/agent/models.json");
        assert_eq!(
            root.pointer("/providers/a/models/0/contextWindow"),
            Some(&json!(12345))
        );
        assert_eq!(
            root.pointer("/providers/a/models/0/unknown/keep"),
            Some(&json!(true))
        );
        assert_eq!(
            root.pointer("/providers/b/models/0/id"),
            Some(&json!("untouched"))
        );
        ai_provider::upsert_provider(payload("a", &["two"])).unwrap();
        let view = get_agent(ModelConfigAgent::Pi).unwrap();
        assert_eq!(view.model.as_deref(), Some("a/two"));
        assert_eq!(
            view.providers
                .iter()
                .find(|p| p.id == "a")
                .unwrap()
                .models
                .len(),
            1
        );
        ai_provider::upsert_provider(payload("a", &[])).unwrap();
        let view = get_agent(ModelConfigAgent::Pi).unwrap();
        assert!(view.model.is_none());
        assert!(view
            .providers
            .iter()
            .find(|p| p.id == "a")
            .unwrap()
            .models
            .is_empty());
        db::clear_agent_target_binding("pi", "a").unwrap();
        assert!(db::binding_migrated("agent:pi:a").unwrap());
    }
    #[test]
    fn claude_default_tiers_independent_empty_and_corruption() {
        let home = Home::new();
        home.write("claude/settings.json",json!({"env":{"ANTHROPIC_MODEL":"gone","ANTHROPIC_DEFAULT_OPUS_MODEL":"second","ANTHROPIC_DEFAULT_HAIKU_MODEL":"removed"},"permissions":{"allow":["read"]}}));
        let dir = home.path.join("claude");
        let dir = dir.to_str().unwrap();
        crate::claude_env::sync_bound_source(dir, Some(&source(&["first", "second"]))).unwrap();
        let root = home.read("claude/settings.json");
        assert_eq!(root.pointer("/env/ANTHROPIC_MODEL"), Some(&json!("first")));
        assert_eq!(
            root.pointer("/env/ANTHROPIC_DEFAULT_OPUS_MODEL"),
            Some(&json!("second"))
        );
        assert_eq!(
            root.pointer("/env/ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            Some(&json!("first"))
        );
        assert_eq!(root.pointer("/permissions/allow/0"), Some(&json!("read")));
        crate::claude_env::sync_bound_source(dir, Some(&source(&[]))).unwrap();
        assert!(home
            .read("claude/settings.json")
            .pointer("/env/ANTHROPIC_MODEL")
            .is_none());
        std::fs::write(home.path.join("claude/settings.json"), "broken").unwrap();
        assert!(crate::claude_env::sync_bound_source(dir, Some(&source(&["first"]))).is_err());
        assert_eq!(
            std::fs::read_to_string(home.path.join("claude/settings.json")).unwrap(),
            "broken"
        );
    }
    #[test]
    fn codex_preserves_custom_provider_and_unrelated_settings() {
        let home = Home::new();
        let dir = home.path.join("codex");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"),"model = 'gone'\nmodel_provider = 'custom'\nmodel_reasoning_effort = 'high'\n[model_providers.custom]\nbase_url = 'https://old.invalid/v1'\nwire_api = 'responses'\n").unwrap();
        crate::codex_env::sync_bound_source(dir.to_str().unwrap(), Some(&source(&["first"])))
            .unwrap();
        let doc: toml_edit::DocumentMut = std::fs::read_to_string(dir.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(doc["model"].as_str(), Some("first"));
        assert_eq!(doc["model_provider"].as_str(), Some("custom"));
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("high"));
        assert_eq!(
            doc["model_providers"]["custom"]["wire_api"].as_str(),
            Some("responses")
        );
    }
    #[test]
    fn route_protocol_sets_and_unique_bindings() {
        let _home = Home::new();
        ai_provider::upsert_provider(payload("a", &["one", "two"])).unwrap();
        let mut b = payload("b", &["two", "three"]);
        b.provider_types = vec!["openai_chat".into()];
        ai_provider::upsert_provider(b).unwrap();
        assert_eq!(
            resolve(ROUTE_ID, "openai_chat", false)
                .unwrap()
                .models
                .len(),
            3
        );
        assert_eq!(
            resolve(ROUTE_ID, "anthropic", false).unwrap().models.len(),
            2
        );
        db::upsert_provider_route_toggle("a", false, 0).unwrap();
        assert!(resolve(ROUTE_ID, "anthropic", false)
            .unwrap()
            .models
            .is_empty());
        db::upsert_agent_provider_binding("a", "pi", "target").unwrap();
        db::upsert_agent_provider_binding("b", "pi", "target").unwrap();
        assert_eq!(
            db::agent_target_source("pi", "target").unwrap().as_deref(),
            Some("b")
        );
        assert!(db::load_agent_provider_bindings("a").unwrap().is_empty());
    }
    #[test]
    fn deletion_and_failed_config_retry_are_persistent() {
        let home = Home::new();
        ai_provider::upsert_provider(payload("a", &["one"])).unwrap();
        home.write(
            ".pi/agent/models.json",
            json!({"providers":{"a":{"api":"openai-completions","models":[{"id":"one"}]}}}),
        );
        db::upsert_agent_provider_binding("a", "pi", "a").unwrap();
        std::fs::write(home.path.join(".pi/agent/models.json"), "broken").unwrap();
        let result = ai_provider::delete_provider("a".into()).unwrap();
        assert!(result.sync_results.iter().any(|r| !r.ok));
        assert!(db::load_ai_provider_rows().unwrap().is_empty());
        assert!(db::get_ai_provider_row("a").unwrap().is_some());
        assert!(db::provider_sync_states()
            .unwrap()
            .iter()
            .any(|(id, d, p, _)| id == "a" && *d && *p));
        home.write(".pi/agent/models.json",json!({"providers":{"a":{"api":"openai-completions","models":[{"id":"one"}]},"other":{"api":"openai-completions","models":[]}}}));
        let _guard = lock().unwrap();
        let result = run_source("a").unwrap();
        assert!(result.iter().all(|r| r.ok));
        assert!(db::get_ai_provider_row("a").unwrap().is_none());
        assert!(home
            .read(".pi/agent/models.json")
            .pointer("/providers/other")
            .is_some());
    }
    #[test]
    fn opencode_and_omp_batch_native_shapes_preserve_fields() {
        let home = Home::new();
        let models = source(&["one", "new"]).models;
        home.write(".config/opencode/opencode.json", json!({"providers":{"a":{"package":"aisdk:@ai-sdk/openai","models":{"one":{"name":"keep","limit":{"context":12345},"extra":true},"deleted":{}}}},"theme":"dark"}));
        oc::sync_model_set("a", &models).unwrap();
        let root = home.read(".config/opencode/opencode.json");
        assert_eq!(
            root.pointer("/providers/a/models/one/extra"),
            Some(&json!(true))
        );
        assert!(root.pointer("/providers/a/models/deleted").is_none());
        assert!(root.pointer("/providers/a/models/new").is_some());
        assert_eq!(root.get("theme"), Some(&json!("dark")));
        let path = home.path.join(".omp/agent/models.yml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "providers:\n  a:\n    api: openai-completions\n    models:\n      - id: one\n        contextWindow: 12345\n        custom: true\n      - id: deleted\n").unwrap();
        pi::sync_model_set(ModelConfigAgent::OhMyPi, "a", &models).unwrap();
        let root: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            root.pointer("/providers/a/models/0/custom"),
            Some(&json!(true))
        );
        assert_eq!(
            root.pointer("/providers/a/models/1/id"),
            Some(&json!("new"))
        );
    }
    #[test]
    fn migration_unique_address_and_explicit_unbind_remains_manual() {
        let home = Home::new();
        ai_provider::upsert_provider(payload("source", &["one"])).unwrap();
        home.write(".pi/agent/models.json", json!({"providers":{"local":{"api":"openai-completions","baseUrl":"https://example.invalid/v1/","models":[{"id":"old"}]}}}));
        let _guard = lock().unwrap();
        migrate_bindings().unwrap();
        assert_eq!(
            db::agent_target_source("pi", "local").unwrap().as_deref(),
            Some("source")
        );
        db::clear_agent_target_binding("pi", "local").unwrap();
        migrate_bindings().unwrap();
        assert!(db::agent_target_source("pi", "local").unwrap().is_none());
    }
    #[test]
    fn initial_binding_journal_recovers_without_persisted_credentials() {
        let home = Home::new();
        ai_provider::upsert_provider(payload("source", &["one", "two"])).unwrap();
        let safe = json!({"id":"local","api":"openai-completions"}).to_string();
        // Simulate a process exiting immediately after committing intent, before any native file write.
        db::begin_binding_intent("source", "pi", "local", "local", &safe).unwrap();
        assert!(!home.path.join(".pi/agent/models.json").exists());
        let intents = db::binding_intents("source").unwrap();
        assert_eq!(intents.len(), 1);
        assert!(!intents[0].2.contains("apiKey"));
        assert!(!intents[0].2.contains("test-placeholder"));
        let _guard = lock().unwrap();
        let results = run_source("source").unwrap();
        assert!(results.iter().all(|r| r.ok), "{:?}", results);
        assert!(db::binding_intents("source").unwrap().is_empty());
        let root = home.read(".pi/agent/models.json");
        assert_eq!(
            root.pointer("/providers/local/models/0/id"),
            Some(&json!("one"))
        );
        assert_eq!(
            root.pointer("/providers/local/models/1/id"),
            Some(&json!("two"))
        );
    }
    #[test]
    fn failed_initial_write_retains_intent_then_recovers() {
        let home = Home::new();
        ai_provider::upsert_provider(payload("source", &["one"])).unwrap();
        std::fs::create_dir_all(home.path.join(".pi/agent/models.json")).unwrap();
        let safe = json!({"id":"local","api":"openai-completions"}).to_string();
        db::begin_binding_intent("source", "pi", "local", "local", &safe).unwrap();
        let _guard = lock().unwrap();
        assert!(run_source("source").unwrap().iter().any(|r| !r.ok));
        assert_eq!(db::binding_intents("source").unwrap().len(), 1);
        std::fs::remove_dir(home.path.join(".pi/agent/models.json")).unwrap();
        assert!(run_source("source").unwrap().iter().all(|r| r.ok));
        assert!(db::binding_intents("source").unwrap().is_empty());
    }
    #[test]
    fn unbinding_cancels_pending_initial_binding() {
        let home = Home::new();
        ai_provider::upsert_provider(payload("source", &["one"])).unwrap();
        db::begin_binding_intent(
            "source",
            "pi",
            "local",
            "local",
            &json!({"id":"local","api":"openai-completions"}).to_string(),
        )
        .unwrap();
        db::clear_agent_target_binding("pi", "local").unwrap();
        assert!(db::binding_intents("source").unwrap().is_empty());
        let _guard = lock().unwrap();
        run_source("source").unwrap();
        assert!(!home.path.join(".pi/agent/models.json").exists());
    }
    #[test]
    fn damaged_migration_target_does_not_block_healthy_agent() {
        let home = Home::new();
        ai_provider::upsert_provider(payload("source", &["one"])).unwrap();
        home.write(".config/opencode/opencode.json", json!({"providers":{"healthy":{"package":"aisdk:@ai-sdk/openai","settings":{"baseURL":"https://example.invalid/v1"},"models":{}}}}));
        std::fs::create_dir_all(home.path.join(".pi/agent")).unwrap();
        std::fs::write(home.path.join(".pi/agent/models.json"), "broken").unwrap();
        let _guard = lock().unwrap();
        assert!(migrate_bindings().is_err());
        assert_eq!(
            db::agent_target_source("opencode", "healthy")
                .unwrap()
                .as_deref(),
            Some("source")
        );
        assert!(!db::binding_migration_error().unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(home.path.join(".pi/agent/models.json")).unwrap(),
            "broken"
        );
    }
    #[test]
    fn successful_file_steps_recorded_even_if_later_step_fails() {
        let home = Home::new();
        let first = home.path.join("first.json");
        let second = home.path.join("second.json");
        std::fs::write(&first, "{}").unwrap();
        std::fs::create_dir(&second).unwrap();
        let result = reconcile_target("source", "target", || {
            crate::config_sync_io::observe(&first)?;
            oc::atomic_write(&first, "{\"updated\":true}")?;
            crate::config_sync_io::observe(&second)?;
            oc::atomic_write(&second, "{}")
        });
        assert!(result.is_err());
        assert_eq!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(&first).unwrap()).unwrap()
                ["updated"],
            true
        );
        // The first file is a completed step, not a pending failure.
        assert!(db::failed_sync_files()
            .unwrap()
            .iter()
            .all(|(_, path, _)| path != &first.to_string_lossy()));
    }
    #[test]
    fn delete_retry_cleans_credentials_after_provider_block_already_removed() {
        let home = Home::new();
        ai_provider::upsert_provider(payload("source", &["one"])).unwrap();
        home.write(
            ".pi/agent/models.json",
            json!({"providers":{"local":{"api":"openai-completions","models":[{"id":"one"}]}}}),
        );
        home.write(".pi/agent/auth.json", json!({"local":{"type":"api_key","key":"test-placeholder"},"other":{"type":"api_key","key":"other-placeholder"}}));
        db::upsert_agent_provider_binding("source", "pi", "local").unwrap();
        // Simulate a deletion step that removed the block but failed before auth cleanup.
        home.write(
            ".pi/agent/models.json",
            json!({"providers":{"unrelated":{"api":"openai-completions","models":[]}}}),
        );
        db::mark_provider_sync("source", true).unwrap();
        let _guard = lock().unwrap();
        let results = run_source("source").unwrap();
        assert!(results.iter().all(|r| r.ok), "{:?}", results);
        let auth = home.read(".pi/agent/auth.json");
        assert!(auth.get("local").is_none());
        assert!(auth.get("other").is_some());
        assert!(db::get_ai_provider_row("source").unwrap().is_none());
    }
    #[test]
    fn recovering_completed_model_step_does_not_rewrite_file() {
        let home = Home::new();
        ai_provider::upsert_provider(payload("source", &["one"])).unwrap();
        let config =
            serde_json::from_value(json!({"id":"local","api":"openai-completions"})).unwrap();
        save_agent_provider(ModelConfigAgent::Pi, config, Some("source".into())).unwrap();
        let path = home.path.join(".pi/agent/models.json");
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let _guard = lock().unwrap();
        assert!(run_source("source").unwrap().iter().all(|r| r.ok));
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
    }
    #[test]
    fn restored_route_pool_drops_deleted_models_and_old_credentials() {
        let _home = Home::new();
        ai_provider::upsert_provider(payload("source", &["one", "deleted"])).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let state = crate::route_aggregation::RouteAggregationState::new(Default::default());
            assert!(state.refresh_restored_pools().await.is_empty());
            assert!(state
                .provider_router
                .get_enabled_model_ids()
                .await
                .contains(&"deleted".into()));
            ai_provider::upsert_provider(payload("source", &["one"])).unwrap();
            assert!(state.refresh_restored_pools().await.is_empty());
            assert_eq!(
                state.provider_router.get_enabled_model_ids().await,
                vec!["one"]
            );
            for group in crate::route_aggregation::RouteGroup::ALL {
                state.provider_router.clear_pool(group).await;
            }
            assert!(state
                .provider_router
                .get_enabled_model_ids()
                .await
                .is_empty());
        });
    }
    /// 回归：反向同步必须覆盖默认环境（旧实现按 `is_default = 0` 查找会静默跳过它），
    /// 目录未初始化的环境只挂待办不写盘也不报硬错，且仍有效的模型与无关键不得被覆盖。
    #[test]
    fn source_sync_covers_default_envs_and_defers_uninitialized_dirs() {
        struct EnvGuard {
            claude: Vec<String>,
            codex: Vec<String>,
            provider: String,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for id in &self.claude {
                    let _ = db::delete_claude_environment_row(id);
                }
                for id in &self.codex {
                    let _ = db::delete_codex_environment_row(id);
                }
                let _ = db::delete_ai_provider_row(&self.provider);
            }
        }

        let home = Home::new();
        let provider_id = format!("sync-envs-{}", uuid::Uuid::new_v4());
        ai_provider::upsert_provider(payload(&provider_id, &["claude-old", "gpt-old"])).unwrap();

        let claude_dirs = [
            home.path.join("claude-default"),
            home.path.join("claude-work"),
        ];
        for dir in &claude_dirs {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join("settings.json"),
                r#"{"env":{"ANTHROPIC_BASE_URL":"https://old.example","ANTHROPIC_MODEL":"claude-old","ANTHROPIC_DEFAULT_OPUS_MODEL":"claude-gone","KEEP_ME":"yes"}}"#,
            )
            .unwrap();
        }
        let claude_missing = home.path.join("claude-missing");
        let codex_dirs = [
            home.path.join("codex-default"),
            home.path.join("codex-work"),
        ];
        for dir in &codex_dirs {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join("config.toml"),
                "model = \"gpt-old\"\nmodel_reasoning_effort = \"high\"\n",
            )
            .unwrap();
        }
        let codex_missing = home.path.join("codex-missing");

        let mut guard = EnvGuard {
            claude: Vec::new(),
            codex: Vec::new(),
            provider: provider_id.clone(),
        };
        for (index, dir) in claude_dirs
            .iter()
            .chain(std::iter::once(&claude_missing))
            .enumerate()
        {
            let id = format!("{provider_id}-claude-{index}");
            db::upsert_claude_environment_row(&crate::claude_env::ClaudeEnvironmentRow {
                id: id.clone(),
                name: format!("claude-env-{index}"),
                slug: id.clone(),
                config_dir: dir.to_string_lossy().to_string(),
                alias_name: id.clone(),
                is_default: index == 0,
                source: "managed".into(),
                notes: String::new(),
                alias_installed: false,
                provider_id: provider_id.clone(),
                created_at: 0,
                updated_at: 0,
            })
            .unwrap();
            guard.claude.push(id);
        }
        for (index, dir) in codex_dirs
            .iter()
            .chain(std::iter::once(&codex_missing))
            .enumerate()
        {
            let id = format!("{provider_id}-codex-{index}");
            db::upsert_codex_environment_row(&crate::codex_env::CodexEnvironmentRow {
                id: id.clone(),
                name: format!("codex-env-{index}"),
                slug: id.clone(),
                config_dir: dir.to_string_lossy().to_string(),
                alias_name: id.clone(),
                is_default: index == 0,
                source: "managed".into(),
                notes: String::new(),
                alias_installed: false,
                provider_id: provider_id.clone(),
                created_at: 0,
                updated_at: 0,
            })
            .unwrap();
            guard.codex.push(id);
        }

        let _sync = lock().unwrap();
        let results = run_source(&provider_id).unwrap();
        let ok: Vec<&str> = results
            .iter()
            .filter(|r| r.ok)
            .map(|r| r.target.as_str())
            .collect();
        let pending: Vec<&str> = results
            .iter()
            .filter(|r| !r.ok)
            .map(|r| r.target.as_str())
            .collect();
        // 默认环境（index 0）必须与非默认环境一起被同步。
        for label in [
            "Claude/claude-env-0",
            "Claude/claude-env-1",
            "Codex/codex-env-0",
            "Codex/codex-env-1",
        ] {
            assert!(ok.contains(&label), "缺少成功同步项 {label}：{results:?}");
        }
        assert_eq!(
            pending,
            vec!["Claude/claude-env-2", "Codex/codex-env-2"],
            "只有目录未初始化的环境应挂待办"
        );
        for item in results.iter().filter(|r| !r.ok) {
            assert!(
                item.message.contains("等待初始化后同步"),
                "{:?}",
                item.message
            );
        }
        assert!(!claude_missing.join("settings.json").exists());
        assert!(!codex_missing.join("config.toml").exists());

        for dir in &claude_dirs {
            let raw: Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                    .unwrap();
            assert_eq!(raw["env"]["ANTHROPIC_BASE_URL"], "https://example.invalid");
            assert_eq!(
                raw["env"]["ANTHROPIC_MODEL"], "claude-old",
                "仍有效的主模型不得改写"
            );
            assert_eq!(raw["env"]["KEEP_ME"], "yes", "无关 env 键必须保留");
            assert_eq!(
                raw["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "claude-old",
                "失效档位回退到来源第一项"
            );
            assert_eq!(
                crate::ai_provider::get_provider_secret(provider_id.clone()).unwrap(),
                "test-placeholder"
            );
        }
        for dir in &codex_dirs {
            let doc: toml_edit::DocumentMut = std::fs::read_to_string(dir.join("config.toml"))
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(
                doc["model"].as_str(),
                Some("gpt-old"),
                "仍有效的顶层模型不得改写"
            );
            assert_eq!(doc["model_reasoning_effort"].as_str(), Some("high"));
            assert!(doc["openai_base_url"].as_str() == Some("https://example.invalid/v1"));
        }

        // 环境全部解除关联后：只剩该供应商自身，零副作用。
        for id in std::mem::take(&mut guard.claude) {
            db::delete_claude_environment_row(&id).unwrap();
        }
        for id in std::mem::take(&mut guard.codex) {
            db::delete_codex_environment_row(&id).unwrap();
        }
        let results = run_source(&provider_id).unwrap();
        assert!(
            results.iter().all(|r| r.ok
                && !r.target.starts_with("Claude/")
                && !r.target.starts_with("Codex/")),
            "{results:?}"
        );
    }
    fn catalog_fixture() -> oc::ModelsDevCatalog {
        let entry = |id: &str,
                     name: &str,
                     ctx: Option<f64>,
                     inp: Option<f64>,
                     out: Option<f64>,
                     input: &[&str],
                     output: &[&str]| oc::CatalogModelSummary {
            id: id.into(),
            name: name.into(),
            limit_context: ctx,
            limit_input: inp,
            limit_output: out,
            modalities_input: input.iter().map(|v| (*v).to_string()).collect(),
            modalities_output: output.iter().map(|v| (*v).to_string()).collect(),
            reasoning: false,
            reasoning_options: Vec::new(),
            tool_call: false,
            attachment: false,
            status: None,
        };
        oc::ModelsDevCatalog {
            fetched_at: 0,
            from_cache: true,
            providers: vec![
                // 聚合入口：只收录模型，不带任何 limit/模态数据。
                oc::CatalogProvider {
                    id: "router".into(),
                    name: "Router".into(),
                    env: vec![],
                    npm: None,
                    models: vec![entry("m1", "M One", None, None, None, &[], &[])],
                },
                // 真正的原厂条目：上下文 + 输入模态。
                oc::CatalogProvider {
                    id: "origin".into(),
                    name: "Origin".into(),
                    env: vec![],
                    npm: None,
                    models: vec![entry(
                        "m1",
                        "Origin M1",
                        Some(200_000.0),
                        Some(120_000.0),
                        None,
                        &["text", "image"],
                        &["text"],
                    )],
                },
                // 另一家中转：只补输出上限与 pdf 能力。
                oc::CatalogProvider {
                    id: "other".into(),
                    name: "Other".into(),
                    env: vec![],
                    npm: None,
                    models: vec![entry(
                        "m1",
                        "Other M1",
                        None,
                        None,
                        Some(8_192.0),
                        &["pdf"],
                        &["text"],
                    )],
                },
            ],
        }
    }

    /// 回归：自动导入必须和「从 Models.dev 补全」用同一套合并规则。
    /// 同名模型在目录里出现十几次（聚合条目不带数据）时，不能因为「命中多条」而放弃补全。
    #[test]
    fn catalog_merge_matches_the_manual_completion_rule() {
        let _home = Home::new();
        oc::set_catalog_cache_for_tests(Some(catalog_fixture()));
        let hit = catalog_model("router", "m1").expect("聚合入口命中同名模型");
        assert_eq!(
            hit.name,
            "Router M1".replace("Router M1", "M One"),
            "身份字段取严格匹配条目"
        );
        assert_eq!(hit.limit_context, Some(200_000.0));
        assert_eq!(hit.limit_input, Some(120_000.0));
        assert_eq!(hit.limit_output, Some(8_192.0), "输出上限来自另一家命中");
        assert_eq!(hit.modalities_input, vec!["text", "image", "pdf"]);
        assert_eq!(hit.modalities_output, vec!["text"]);
        assert!(catalog_model("router", "absent").is_none());
        oc::set_catalog_cache_for_tests(None);
    }

    /// 回归：绑定导入的新模型要带上目录里的上下文/输出上限与模态，
    /// OpenCode 写 limit/capabilities，Pi 家族写 contextWindow/maxTokens/input。
    #[test]
    fn bound_import_fills_parameters_from_local_catalog() {
        let home = Home::new();
        oc::set_catalog_cache_for_tests(Some(catalog_fixture()));
        ai_provider::upsert_provider(payload("cat", &["m1"])).unwrap();

        let opencode_payload: oc::UpsertProviderPayload =
            serde_json::from_value(json!({"id":"routed","package":"aisdk:@ai-sdk/openai"}))
                .unwrap();
        save_agent_provider(
            ModelConfigAgent::Opencode,
            opencode_payload,
            Some("cat".into()),
        )
        .unwrap();
        let raw = home.read(".config/opencode/opencode.json");
        let m = &raw["providers"]["routed"]["models"]["m1"];
        assert_eq!(m["limit"]["context"], 200_000.0);
        assert_eq!(m["limit"]["input"], 120_000.0);
        assert_eq!(m["limit"]["output"], 8_192.0);
        assert_eq!(m["capabilities"]["input"], json!(["text", "image", "pdf"]));
        assert_eq!(m["capabilities"]["output"], json!(["text"]));

        let pi_payload: oc::UpsertProviderPayload =
            serde_json::from_value(json!({"id":"routed","api":"openai-responses"})).unwrap();
        save_agent_provider(ModelConfigAgent::Pi, pi_payload, Some("cat".into())).unwrap();
        let raw = home.read(".pi/agent/models.json");
        let m = &raw["providers"]["routed"]["models"][0];
        assert_eq!(m["contextWindow"], 200_000.0);
        assert_eq!(m["maxTokens"], 8_192.0);
        // Pi schema 只认 text / text+image：pdf 不会写坏配置，但目录命中仍会补上限。
        assert_eq!(m["input"], json!(["text", "image"]));
        oc::set_catalog_cache_for_tests(None);
    }
}
