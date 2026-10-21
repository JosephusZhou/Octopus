//! Request handlers — entry points for Axum routes.
//!
//! The aggregated endpoint always serves the supported API formats (Claude
//! messages, Codex responses, OpenAI Chat Completions) while the server is
//! running. Each handler authenticates the request against the configured API
//! key list, then delegates to the forwarder and returns the upstream response
//! (with SSE passthrough for streaming).
//!
//! Every handler also writes a `LogEntry` to the shared `LogStore` so the
//! UI's "进出日志" list can show recent traffic.

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::time::Instant;

use super::forwarder;
use super::log::{
    body_preview_json, redact_json_for_log, redact_json_text_for_log, sanitize_header_value,
    sanitize_url_for_log, truncate_for_preview, InboundProtocol, LogEntry, LogEntryFull,
    BODY_PREVIEW_MAX_BYTES,
};
use super::router::AppState;
use super::RouteGroup;

/// Extract the Bearer token from the Authorization header.
/// Returns None if the header is missing or not a Bearer token.
fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    let token = auth.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Authenticate a request against the configured API key list.
/// Returns Ok(()) if the token matches any key, or if no key is configured
/// (no-auth mode). Returns Err(Response) with 401 otherwise.
fn authenticate(headers: &HeaderMap, api_keys: &[String]) -> Result<(), Response> {
    if api_keys.is_empty() {
        // No API key configured — open access
        return Ok(());
    }
    match extract_bearer_token(headers) {
        Some(token) if api_keys.iter().any(|k| *k == token) => Ok(()),
        _ => Err(error_response(
            StatusCode::UNAUTHORIZED,
            "无效或缺失的 API Key",
        )),
    }
}

/// Handler for Claude Code messages endpoint: POST /v1/messages
pub async fn handle_claude_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    handle_with_log(
        state,
        RouteGroup::ClaudeCode,
        InboundProtocol::ClaudeMessages,
        headers,
        body,
        false,
    )
    .await
}

/// Handler for Claude Code token counting: POST /v1/messages/count_tokens.
pub async fn handle_claude_count_tokens(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    handle_with_log(
        state,
        RouteGroup::ClaudeCode,
        InboundProtocol::ClaudeMessages,
        headers,
        body,
        true,
    )
    .await
}

/// Handler for Codex Responses API: POST /v1/responses
/// Codex CLI uses the Responses API format, not Chat Completions.
pub async fn handle_codex_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    handle_with_log(
        state,
        RouteGroup::Codex,
        InboundProtocol::CodexResponses,
        headers,
        body,
        false,
    )
    .await
}

/// Handler for OpenAI Chat Completions: POST /v1/chat/completions
/// OpenAI SDK 等 Chat Completions 客户端 → CC→CC 同协议透传（不做格式转换）。
pub async fn handle_openai_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    handle_with_log(
        state,
        RouteGroup::OpenAiChat,
        InboundProtocol::OpenAiChatCompletions,
        headers,
        body,
        false,
    )
    .await
}

/// Common handler body shared by all inbound POST endpoints.
///
/// Responsibilities:
/// 1. Authenticate against the configured API key list.
/// 2. Run the forwarder (which handles cloaking, failover, and passthrough).
/// 3. Write a LogEntry to the shared LogStore regardless of success/failure
///    so the UI can see the full inbound → outbound lifecycle.
async fn handle_with_log(
    state: AppState,
    group: RouteGroup,
    protocol: InboundProtocol,
    headers: HeaderMap,
    body: serde_json::Value,
    count_tokens: bool,
) -> Response {
    let started = Instant::now();
    let config = state.config.read().await.clone();

    // Re-derive the inbound path from headers (axum's matched_path is in the
    // router; we keep a stable string here instead of plumbing it through).
    let inbound_path = match protocol {
        InboundProtocol::ClaudeMessages if count_tokens => "/v1/messages/count_tokens",
        InboundProtocol::ClaudeMessages => "/v1/messages",
        InboundProtocol::CodexResponses => "/v1/responses",
        InboundProtocol::OpenAiChatCompletions => "/v1/chat/completions",
        InboundProtocol::OpenAiModelsList => "/v1/models",
    }
    .to_string();

    // Capture inbound headers (sanitized).
    let inbound_headers: Vec<[String; 2]> = headers
        .iter()
        .filter_map(|(k, v)| {
            let name = k.as_str().to_string();
            let value = v.to_str().ok()?.to_string();
            Some([name, sanitize_header_value(k.as_str(), &value)])
        })
        .collect();

    // Inbound body: 内存保留 64KB 预览；全量脱敏 JSON 单独传给磁盘日志链路。
    let full_inbound_body = redact_json_for_log(&body);
    let raw_body_str = serde_json::to_string(&full_inbound_body).unwrap_or_default();
    let (inbound_body, inbound_body_truncated) =
        body_preview_json(raw_body_str.as_bytes(), BODY_PREVIEW_MAX_BYTES);
    let inbound_model = body
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let is_stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if let Err(resp) = authenticate(&headers, &config.api_keys) {
        // Log auth failures too — they're useful for diagnosing "client can't
        // even reach the proxy" issues.
        let entry = LogEntry {
            id: 0,
            timestamp_ms: chrono_millis(),
            protocol,
            inbound_method: "POST".to_string(),
            inbound_path: inbound_path.clone(),
            inbound_headers: inbound_headers.clone(),
            inbound_body: inbound_body.clone(),
            inbound_body_truncated,
            inbound_model: inbound_model.clone(),
            upstream_model: None,
            provider_id: None,
            provider_name: None,
            upstream_url: None,
            upstream_status: Some(resp.status().as_u16()),
            upstream_headers: Vec::new(),
            upstream_body: None,
            upstream_body_truncated: false,
            stream: is_stream,
            duration_ms: started.elapsed().as_millis() as u64,
            success: false,
            error: Some("未通过 API Key 鉴权".to_string()),
        };
        state
            .log_store
            .push(
                entry,
                LogEntryFull {
                    inbound_body: Some(full_inbound_body.clone()),
                    upstream_body: None,
                },
            )
            .await;
        return resp;
    }

    let forward_result = forwarder::forward(
        group,
        body,
        &headers,
        &config,
        &state.provider_router,
        count_tokens,
    )
    .await;

    match forward_result {
        Ok(result) => {
            let upstream_status = Some(result.response.status().as_u16());
            let upstream_headers = collect_response_headers(&result.response);
            // 上游响应体：磁盘全量日志保留完整脱敏文本；内存 entry 只留 64KB 预览。
            let upstream_full = if is_stream {
                None
            } else if result.body_preview.is_empty() {
                Some(String::new())
            } else {
                Some(redact_json_text_for_log(&String::from_utf8_lossy(
                    &result.body_preview,
                )))
            };
            let (upstream_body, upstream_body_truncated) = match &upstream_full {
                Some(full) => truncate_for_preview(full.as_bytes(), BODY_PREVIEW_MAX_BYTES),
                None => (String::new(), false),
            };
            let upstream_body_str = upstream_full.as_ref().map(|_| upstream_body);
            let entry = LogEntry {
                id: 0,
                timestamp_ms: chrono_millis(),
                protocol,
                inbound_method: "POST".to_string(),
                inbound_path,
                inbound_headers,
                inbound_body,
                inbound_body_truncated,
                inbound_model,
                upstream_model: result.effective_model,
                provider_id: Some(result.provider_id),
                provider_name: Some(result.provider_name),
                upstream_url: Some(sanitize_url_for_log(&result.upstream_url)),
                upstream_status,
                upstream_headers,
                upstream_body: upstream_body_str,
                upstream_body_truncated,
                stream: is_stream,
                duration_ms: started.elapsed().as_millis() as u64,
                success: result.response.status().is_success(),
                error: None,
            };
            // 流式 + 开启完整记录：保留 meta 与入站全量，供流结束后补写磁盘完整行。
            // 注意：真实 id 由 push 内部分配，此处 clone 的 entry.id 仍为 0，
            // 必须在 push 返回后用返回值回填，否则补写行以 id=0 落盘、详情按
            // 列表条目的真实 id 永远读不到完整流式响应。
            let stream_recorder = if is_stream {
                result
                    .stream_body_rx
                    .map(|rx| (entry.clone(), full_inbound_body.clone(), rx))
            } else {
                None
            };
            let id = state
                .log_store
                .push(
                    entry,
                    LogEntryFull {
                        inbound_body: Some(full_inbound_body),
                        upstream_body: upstream_full,
                    },
                )
                .await;
            if let Some((mut meta, inbound_full, rx)) = stream_recorder {
                meta.id = id;
                spawn_stream_body_recorder(
                    state.log_store.clone(),
                    meta,
                    inbound_full,
                    rx,
                    std::time::Duration::from_secs(config.stream_idle_timeout),
                );
            }
            result.response
        }
        Err(err) => {
            let entry = LogEntry {
                id: 0,
                timestamp_ms: chrono_millis(),
                protocol,
                inbound_method: "POST".to_string(),
                inbound_path,
                inbound_headers,
                inbound_body,
                inbound_body_truncated,
                inbound_model,
                upstream_model: None,
                provider_id: None,
                provider_name: None,
                upstream_url: None,
                upstream_status: None,
                upstream_headers: Vec::new(),
                upstream_body: None,
                upstream_body_truncated: false,
                stream: is_stream,
                duration_ms: started.elapsed().as_millis() as u64,
                success: false,
                error: Some(error_label(&err)),
            };
            state
                .log_store
                .push(
                    entry,
                    LogEntryFull {
                        inbound_body: Some(full_inbound_body),
                        upstream_body: None,
                    },
                )
                .await;
            forward_error_to_response(err)
        }
    }
}

/// 流式请求（开启完整记录）的响应体收集与落盘。
///
/// tee 收集端随转发流结束（正常完成或中断）自动关闭；此时把收集到的完整
/// 脱敏文本追加为**同 id** 的磁盘行（详情读取取最后一个匹配行，即最完整的一行）。
/// 转发本身不受影响——收集只做复制，不阻塞也不缓冲给客户端。
///
/// `idle_timeout`：连续 `idle_timeout` 无新 chunk（如 Claude 长时间思考不吐
/// token）时提前收尾写盘，已收集部分标记为截断；转发流继续照常向客户端输出。
fn spawn_stream_body_recorder(
    log_store: super::log::LogStore,
    meta: LogEntry,
    inbound_full: serde_json::Value,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<bytes::Bytes>,
    idle_timeout: std::time::Duration,
) {
    tokio::spawn(async move {
        let mut collected: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut idle_closed = false;
        loop {
            let chunk = match tokio::time::timeout(idle_timeout, rx.recv()).await {
                Ok(Some(chunk)) => chunk,
                // 流结束：发送端全部 drop（正常完成或中断）
                Ok(None) => break,
                // 闲置超时：提前收尾（转发流不受影响，后续 chunk 不再收集）
                Err(_) => {
                    idle_closed = true;
                    break;
                }
            };
            if collected.len() >= super::log::DISK_BODY_MAX_BYTES {
                truncated = true;
            } else {
                let remaining = super::log::DISK_BODY_MAX_BYTES - collected.len();
                collected.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
        }
        // 流结束 / 闲置收尾：能收集多少写多少（16MB 保护上限内）
        let full_text = redact_json_text_for_log(&String::from_utf8_lossy(&collected));
        let mut disk = super::log::build_disk_entry(
            &meta,
            super::log::LogEntryFull {
                inbound_body: Some(inbound_full),
                upstream_body: Some(full_text),
            },
        );
        disk.upstream_body_truncated = truncated || idle_closed || disk.upstream_body_truncated;
        if let Some(file) = log_store.file() {
            let _ = tokio::task::spawn_blocking(move || file.write_entry(&disk)).await;
        }
    });
}

fn error_label(err: &forwarder::ForwardError) -> String {
    match err {
        forwarder::ForwardError::NoAvailableProvider(Some(model)) => {
            format!("没有可用的供应商支持模型 {model}")
        }
        forwarder::ForwardError::NoAvailableProvider(None) => "没有可用的供应商".to_string(),
        forwarder::ForwardError::AllProvidersFailed => "所有供应商均请求失败".to_string(),
        forwarder::ForwardError::CloakingError(msg) => format!("伪装处理失败: {msg}"),
        forwarder::ForwardError::RequestError(msg) => msg.clone(),
    }
}

fn forward_error_to_response(err: forwarder::ForwardError) -> Response {
    match err {
        forwarder::ForwardError::NoAvailableProvider(Some(model)) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("没有可用的供应商支持模型 {model}"),
        ),
        forwarder::ForwardError::NoAvailableProvider(None) => {
            error_response(StatusCode::SERVICE_UNAVAILABLE, "没有可用的供应商")
        }
        forwarder::ForwardError::AllProvidersFailed => {
            error_response(StatusCode::BAD_GATEWAY, "所有供应商均请求失败")
        }
        forwarder::ForwardError::CloakingError(msg) => {
            error_response(StatusCode::BAD_REQUEST, &msg)
        }
        forwarder::ForwardError::RequestError(msg) => error_response(StatusCode::BAD_GATEWAY, &msg),
    }
}

fn collect_response_headers(resp: &Response) -> Vec<[String; 2]> {
    resp.headers()
        .iter()
        .filter_map(|(k, v)| {
            let name = k.as_str().to_string();
            let value = v.to_str().ok()?.to_string();
            Some([name.clone(), sanitize_header_value(&name, &value)])
        })
        .collect()
}

fn chrono_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Handler for listing models: GET /v1/models
///
/// Authenticates against the API key list and returns the union of all
/// checked providers' custom model lists (`ai_providers.custom_models_json`)。
/// **不**向任何 provider 的远端 /v1/models 发起请求——配置侧的自定义模型列表
/// 即为对外暴露的全部模型。
///
/// The request is logged through the shared `LogStore` so the UI's
/// 进出日志 list shows model-list calls alongside the POST endpoints —
/// `InboundProtocol::OpenAiModelsList` distinguishes them.
pub async fn handle_list_models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let started = Instant::now();
    let config = state.config.read().await.clone();

    // Sanitize inbound headers (mirrors handle_with_log so the UI's
    // debug view shows the same Authorization / x-api-key masking).
    let inbound_headers: Vec<[String; 2]> = headers
        .iter()
        .filter_map(|(k, v)| {
            let name = k.as_str().to_string();
            let value = v.to_str().ok()?.to_string();
            Some([name, sanitize_header_value(k.as_str(), &value)])
        })
        .collect();
    let inbound_path = "/v1/models".to_string();

    if let Err(resp) = authenticate(&headers, &config.api_keys) {
        let entry = LogEntry {
            id: 0,
            timestamp_ms: chrono_millis(),
            protocol: InboundProtocol::OpenAiModelsList,
            inbound_method: "GET".to_string(),
            inbound_path,
            inbound_headers,
            inbound_body: None,
            inbound_body_truncated: false,
            inbound_model: None,
            upstream_model: None,
            provider_id: None,
            provider_name: None,
            upstream_url: None,
            upstream_status: Some(resp.status().as_u16()),
            upstream_headers: Vec::new(),
            upstream_body: None,
            upstream_body_truncated: false,
            stream: false,
            duration_ms: started.elapsed().as_millis() as u64,
            success: false,
            error: Some("未通过 API Key 鉴权".to_string()),
        };
        state.log_store.push(entry, LogEntryFull::none()).await;
        return resp;
    }

    // 取所有启用 provider 的 customModels 并集：纯内存读取，无网络 I/O。
    let ids = state.provider_router.get_enabled_model_ids().await;

    // Build the response
    let data: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "object": "model",
                "owned_by": "route-aggregation",
            })
        })
        .collect();

    let body = serde_json::json!({
        "object": "list",
        "data": data,
    });
    let body_str = body.to_string();
    let (upstream_body, upstream_body_truncated) =
        truncate_for_preview(body_str.as_bytes(), BODY_PREVIEW_MAX_BYTES);

    let entry = LogEntry {
        id: 0,
        timestamp_ms: chrono_millis(),
        protocol: InboundProtocol::OpenAiModelsList,
        inbound_method: "GET".to_string(),
        inbound_path,
        inbound_headers,
        inbound_body: None,
        inbound_body_truncated: false,
        inbound_model: None,
        upstream_model: None,
        provider_id: None,
        provider_name: None,
        upstream_url: None,
        upstream_status: Some(StatusCode::OK.as_u16()),
        upstream_headers: Vec::new(),
        upstream_body: Some(upstream_body),
        upstream_body_truncated,
        stream: false,
        duration_ms: started.elapsed().as_millis() as u64,
        success: true,
        error: None,
    };
    state
        .log_store
        .push(
            entry,
            LogEntryFull {
                inbound_body: None,
                upstream_body: Some(body_str),
            },
        )
        .await;

    (StatusCode::OK, Json(body)).into_response()
}

fn error_response(status: StatusCode, message: &str) -> Response {
    // D5（2026-09-22）：本地代理错误统一输出 OpenAI error schema
    // （{ error: { message, type, param, code } }），OpenAI SDK 客户端可直接
    // 解析；type 按状态码映射到 OpenAI 语义。
    let body = serde_json::json!({
        "error": {
            "message": message,
            "type": error_type_for_status(status),
            "param": null,
            "code": null,
        }
    });
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Map an HTTP status to the OpenAI error `type` vocabulary.
fn error_type_for_status(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "invalid_request_error",
        429 => "rate_limit_error",
        500..=599 => "server_error",
        _ => "api_error",
    }
}
