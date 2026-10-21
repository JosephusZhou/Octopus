//! In-memory request log for the route aggregation proxy.
//!
//! Logs every inbound request and the corresponding upstream response so the
//! frontend can show a list of recent traffic for debugging (e.g. diagnosing
//! 404s caused by missing route handlers). Entries are stored in a bounded
//! ring buffer (default capacity 2000) and discarded FIFO when full — the goal
//! is operational visibility, not audit, so we keep it in memory only and
//! don't persist across restarts.

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::RwLock;

use super::logfile::LogFile;
use super::types::RouteGroup;

/// Default ring buffer capacity. Overridable via [`LogStore::with_capacity`].
pub const DEFAULT_LOG_CAP: usize = 2000;

/// Maximum body bytes captured per request/response side. Large enough to see
/// headers + the first message of a typical multi-turn OpenAI Responses request
/// (system prompts can stack past 8 KB). 64 KB still keeps each entry well
/// under a megabyte even with 2000 entries in the ring buffer.
pub const BODY_PREVIEW_MAX_BYTES: usize = 64 * 1024;

/// 磁盘全量日志的单侧 body 上限（入站请求 / 上游响应各一）。
///
/// 正常对话 body 只有几十 KB ~ 几 MB，全量落盘不受影响；仅当出现异常超大
/// body（客户端异常、上传超大内容等）时截断到前 16MB 并置 truncated 标志，
/// 防止日志文件被单条记录瞬间撑爆。
pub const DISK_BODY_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Inbound protocol of a logged request. Phase 5+：路由聚合只保留两种入站
/// 协议（Claude Messages + Codex Responses），与 RouteGroup 一一对应。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InboundProtocol {
    ClaudeMessages,
    CodexResponses,
    OpenAiChatCompletions,
    #[serde(rename = "openaiModelsList")]
    OpenAiModelsList,
}

impl InboundProtocol {
    #[allow(dead_code)]
    pub fn from_group(group: RouteGroup) -> Self {
        match group {
            RouteGroup::ClaudeCode => InboundProtocol::ClaudeMessages,
            RouteGroup::Codex => InboundProtocol::CodexResponses,
            RouteGroup::OpenAiChat => InboundProtocol::OpenAiChatCompletions,
        }
    }

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            InboundProtocol::ClaudeMessages => "Claude Messages",
            InboundProtocol::CodexResponses => "Codex Responses",
            InboundProtocol::OpenAiChatCompletions => "OpenAI Chat Completions",
            InboundProtocol::OpenAiModelsList => "Models",
        }
    }
}

/// A single request log entry. Serialized to camelCase for the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Monotonic ID assigned at insert time. Higher = newer.
    pub id: u64,
    /// Unix millis when the inbound request entered the handler.
    pub timestamp_ms: i64,
    pub protocol: InboundProtocol,
    pub inbound_method: String,
    pub inbound_path: String,
    /// Sanitized headers (Authorization, Cookie, x-api-key, x-goog-api-key
    /// values are masked to "***"). May be empty if the inbound request had
    /// no relevant fields.
    pub inbound_headers: Vec<[String; 2]>,
    /// Inbound request body, parsed as JSON. Large bodies are truncated to
    /// [`BODY_PREVIEW_MAX_BYTES`] and `inbound_body_truncated` is set.
    pub inbound_body: Option<serde_json::Value>,
    pub inbound_body_truncated: bool,
    /// Original `model` field extracted from the inbound body, if any.
    pub inbound_model: Option<String>,
    /// Model ID actually sent upstream for the serving attempt. Differs from
    /// `inbound_model` when the provider was matched via a `[1m]` context
    /// variant and the body model was rewritten (see
    /// `ProviderRouter::resolve_providers_for_model`). None if forwarding
    /// failed before an upstream attempt.
    pub upstream_model: Option<String>,

    /// Provider that handled the request (after failover / circuit-breaker
    /// selection). None if forwarding failed before picking one.
    pub provider_id: Option<String>,
    pub provider_name: Option<String>,
    /// Final upstream URL after path construction.
    pub upstream_url: Option<String>,
    pub upstream_status: Option<u16>,
    pub upstream_headers: Vec<[String; 2]>,
    pub upstream_body: Option<String>,
    pub upstream_body_truncated: bool,
    /// True when `stream=true` in the request. We don't read streaming bodies
    /// for logging (would break backpressure); `upstream_body` will be None
    /// and only headers / status are captured.
    pub stream: bool,

    /// Total wall time from handler entry to response ready.
    pub duration_ms: u64,
    pub success: bool,
    pub error: Option<String>,
}

/// 磁盘专用完整日志条目，与内存预览 [`LogEntry`] 分离。
///
/// body 存**脱敏后的全量内容**（凭证字段沿用 [`redact_json_for_log`] /
/// [`redact_json_text_for_log`] 规则打码，prompt / 上下文原样保留），仅序列化
/// 写入 JSONL 日志文件，不进入内存 ring——内存始终只保留预览版 `LogEntry`，
/// 完整 body 在打开详情时按 id 从磁盘按需读取，读完即弃。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskLogEntry {
    /// 单调递增 ID，与对应内存预览条目的 id 一致，用于按需定位读取。
    pub id: u64,
    /// Unix millis when the inbound request entered the handler.
    pub timestamp_ms: i64,
    pub protocol: InboundProtocol,
    pub inbound_method: String,
    pub inbound_path: String,
    /// Sanitized inbound headers (credential values masked to "***").
    pub inbound_headers: Vec<[String; 2]>,
    /// 入站请求体完整 JSON（脱敏后）。超过 [`DISK_BODY_MAX_BYTES`] 时截断并置标志。
    pub inbound_body: Option<serde_json::Value>,
    pub inbound_body_truncated: bool,
    /// 原始 `model` 字段；转发失败未发起上游请求时为 None。
    pub inbound_model: Option<String>,
    /// 实际转发给上游的模型 ID（[1m] 变体回写后）；未发起上游请求时为 None。
    pub upstream_model: Option<String>,
    pub provider_id: Option<String>,
    pub provider_name: Option<String>,
    /// 最终上游 URL（query 中凭证参数已打码）。
    pub upstream_url: Option<String>,
    pub upstream_status: Option<u16>,
    pub upstream_headers: Vec<[String; 2]>,
    /// 上游响应体完整文本（脱敏后）。流式请求不读响应体，为 None。
    pub upstream_body: Option<String>,
    pub upstream_body_truncated: bool,
    /// 是否为流式请求（SSE）。流式响应体不会读入日志。
    pub stream: bool,
    pub duration_ms: u64,
    pub success: bool,
    pub error: Option<String>,
    /// 是否为新版「全量 body」记录。旧格式行（改版前只写 64KB 预览）反序列化
    /// 时该字段缺失、默认 false，前端据此提示完整内容可能不可用。
    #[serde(default)]
    pub full_body: bool,
}

/// 磁盘条目的全量 body 载荷：由 handler 在记录时提供（均已脱敏）。
/// 与预览版 `LogEntry` 分开传递，避免全量内容进入内存 ring。
#[derive(Debug, Clone)]
pub struct LogEntryFull {
    /// 入站请求体完整 JSON（脱敏后）。
    pub inbound_body: Option<serde_json::Value>,
    /// 上游响应体完整文本（脱敏后）；流式请求为 None。
    pub upstream_body: Option<String>,
}

impl LogEntryFull {
    /// 空载荷：用于无 body 可记录的场景（如鉴权失败且无入站内容时）。
    pub fn none() -> Self {
        Self {
            inbound_body: None,
            upstream_body: None,
        }
    }
}

impl DiskLogEntry {
    /// 磁盘完整条目 → 内存预览条目：body 按 64KB 截断，完整内容不入内存。
    ///
    /// 用于 App 重启后把磁盘当天日志回填到列表：详情仍走
    /// `read_entry_by_id` 按需读取完整内容，列表只显示预览。
    pub fn to_preview(&self) -> LogEntry {
        let (inbound_body, inbound_body_truncated) = match &self.inbound_body {
            Some(value) => body_preview_json(
                serde_json::to_string(value).unwrap_or_default().as_bytes(),
                BODY_PREVIEW_MAX_BYTES,
            ),
            None => (None, false),
        };
        let (upstream_body, upstream_truncated) = match &self.upstream_body {
            Some(text) => truncate_for_preview(text.as_bytes(), BODY_PREVIEW_MAX_BYTES),
            None => (String::new(), false),
        };
        LogEntry {
            id: self.id,
            timestamp_ms: self.timestamp_ms,
            protocol: self.protocol,
            inbound_method: self.inbound_method.clone(),
            inbound_path: self.inbound_path.clone(),
            inbound_headers: self.inbound_headers.clone(),
            inbound_body,
            inbound_body_truncated,
            inbound_model: self.inbound_model.clone(),
            upstream_model: self.upstream_model.clone(),
            provider_id: self.provider_id.clone(),
            provider_name: self.provider_name.clone(),
            upstream_url: self.upstream_url.clone(),
            upstream_status: self.upstream_status,
            upstream_headers: self.upstream_headers.clone(),
            upstream_body: self.upstream_body.as_ref().map(|_| upstream_body),
            upstream_body_truncated: upstream_truncated || self.upstream_body_truncated,
            stream: self.stream,
            duration_ms: self.duration_ms,
            success: self.success,
            error: self.error.clone(),
        }
    }
}

/// Thread-safe bounded log store. Cloning the Arc gives a new handle to the
/// same underlying buffer.
#[derive(Clone)]
pub struct LogStore {
    inner: Arc<Inner>,
}

struct Inner {
    capacity: usize,
    entries: RwLock<VecDeque<LogEntry>>,
    next_id: RwLock<u64>,
    /// Serializes ID assignment, ring insertion, and file mirroring so a
    /// concurrent request cannot duplicate or reorder a persisted entry.
    append_lock: AsyncMutex<()>,
    /// Optional on-disk sink. When set, every entry appended to the
    /// in-memory ring is also serialized as a single JSON line and flushed
    /// to disk. Uses a synchronous `std::sync::Mutex` because the file write
    /// itself is synchronous inside `LogFile` and we don't want to lock
    /// across an `.await` for the hot path.
    file: std::sync::Mutex<Option<LogFile>>,
}

impl LogStore {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_LOG_CAP)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                capacity: capacity.max(1),
                entries: RwLock::new(VecDeque::with_capacity(capacity)),
                next_id: RwLock::new(1),
                append_lock: AsyncMutex::new(()),
                file: std::sync::Mutex::new(None),
            }),
        }
    }

    /// Attach an on-disk sink. Existing entries are not replayed; only new
    /// entries written after this call will be persisted. Best-effort: if
    /// the cell is already populated (e.g. setup runs twice), the second
    /// call is ignored.
    pub fn attach_file(&self, file: LogFile) {
        if let Ok(mut slot) = self.inner.file.lock() {
            if slot.is_none() {
                *slot = Some(file);
            }
        }
    }

    /// Path of the on-disk log file, if attached.
    pub fn file_path(&self) -> Option<std::path::PathBuf> {
        self.inner
            .file
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|f| f.path()))
    }

    /// Handle to the on-disk log file, if attached (for on-demand detail reads).
    /// The returned handle is a cheap `Arc` clone; reads never hold the writer lock.
    pub fn file(&self) -> Option<LogFile> {
        self.inner.file.lock().ok().and_then(|guard| guard.clone())
    }

    /// Append a new entry, assigning its ID. FIFO-evicts when full. Also
    /// mirrors the entry to the on-disk file if one is attached: the disk
    /// receives a [`DiskLogEntry`] built from `entry` + `full` (full body
    /// never enters the in-memory ring — it lives on disk only and is read
    /// back on demand by the log-detail command).
    ///
    /// Returns the assigned ID so callers can later append a follow-up full
    /// line when a streamed response finishes (see handler).
    pub async fn push(&self, mut entry: LogEntry, full: LogEntryFull) -> u64 {
        let _append_guard = self.inner.append_lock.lock().await;
        let mut id_guard = self.inner.next_id.write().await;
        let id = *id_guard;
        entry.id = id;
        *id_guard = id_guard.wrapping_add(1);
        drop(id_guard);

        // 磁盘全量条目在写盘路径上组装，不进入内存 ring。
        let disk_entry = build_disk_entry(&entry, full);

        let mut entries = self.inner.entries.write().await;
        while entries.len() >= self.inner.capacity {
            entries.pop_front();
        }
        entries.push_back(entry);
        drop(entries);

        // Move the synchronous file operation off the async runtime. The
        // append guard remains held so persisted lines retain ID order.
        let file = self.inner.file.lock().ok().and_then(|guard| guard.clone());
        if let Some(file) = file {
            let _ = tokio::task::spawn_blocking(move || file.write_entry(&disk_entry)).await;
        }
        id
    }

    /// Snapshot all entries (newest last). Cloned for the frontend consumer.
    pub async fn snapshot(&self) -> Vec<LogEntry> {
        self.inner.entries.read().await.iter().cloned().collect()
    }

    pub async fn clear(&self) {
        let _append_guard = self.inner.append_lock.lock().await;
        self.inner.entries.write().await.clear();
        let file = self.inner.file.lock().ok().and_then(|guard| guard.clone());
        if let Some(file) = file {
            let _ = tokio::task::spawn_blocking(move || file.clear()).await;
        }
    }

    #[allow(dead_code)]
    pub async fn len(&self) -> usize {
        self.inner.entries.read().await.len()
    }

    #[allow(dead_code)]
    pub async fn capacity(&self) -> usize {
        self.inner.capacity
    }
}

impl Default for LogStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Sanitize a header value for logging: mask Authorization / Cookie /
/// token-bearing headers. Everything else is returned unchanged.
pub fn sanitize_header_value(name: &str, value: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "authorization"
            | "cookie"
            | "x-api-key"
            | "x-goog-api-key"
            | "api-key"
            | "proxy-authorization"
    ) {
        return "***".to_string();
    }
    value.to_string()
}

/// Mask credential-like query parameters before an upstream URL is shown in
/// diagnostics. API keys normally travel in headers, but custom gateways may
/// still encode them in the URL.
pub fn sanitize_url_for_log(url: &str) -> String {
    let Some((base, query_and_fragment)) = url.split_once('?') else {
        return url.to_string();
    };
    let (query, fragment) = query_and_fragment
        .split_once('#')
        .map_or((query_and_fragment, None), |(query, fragment)| {
            (query, Some(fragment))
        });
    let query = query
        .split('&')
        .map(|part| {
            let Some((key, value)) = part.split_once('=') else {
                return part.to_string();
            };
            let compact = key.to_ascii_lowercase().replace(['-', '_'], "");
            if compact == "key"
                || compact == "apikey"
                || compact == "token"
                || compact.ends_with("token")
                || compact.contains("password")
                || compact.contains("secret")
                || compact.contains("authorization")
                || compact.contains("credential")
            {
                format!("{key}=***")
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    let fragment = fragment.map_or(String::new(), |value| format!("#{value}"));
    format!("{base}?{query}{fragment}")
}

/// Redact common credential fields before a JSON body is shown in diagnostics.
/// Prompts and tool arguments remain visible, but values under credential-like
/// keys never enter the in-memory or on-disk log.
pub fn redact_json_for_log(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let compact = lower.replace(['-', '_'], "");
                    let redacted = compact == "cookie"
                        || compact == "authorization"
                        || compact == "proxyauthorization"
                        || compact == "apikey"
                        || compact == "xapikey"
                        || compact == "xgoogapikey"
                        || lower == "token"
                        || lower.ends_with("_token")
                        || lower.ends_with("-token")
                        || lower.contains("api_key")
                        || lower.contains("apikey")
                        || lower.contains("auth_token")
                        || lower.contains("access_token")
                        || lower.contains("refresh_token")
                        || lower.contains("password")
                        || lower.contains("secret")
                        || lower.contains("authorization")
                        || lower.contains("credential");
                    (
                        key.clone(),
                        if redacted {
                            serde_json::Value::String("***".to_string())
                        } else {
                            redact_json_for_log(value)
                        },
                    )
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_json_for_log).collect())
        }
        _ => value.clone(),
    }
}

/// Redact credential fields in a JSON response preview when the preview is a
/// complete JSON value. Non-JSON responses are returned unchanged because they
/// cannot be safely transformed without changing their protocol payload.
pub fn redact_json_text_for_log(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .map(|value| redact_json_for_log(&value).to_string())
        .unwrap_or_else(|_| text.to_string())
}

/// Truncate a byte slice to `max` bytes, returning the prefix as `String`
/// (lossy UTF-8 conversion if truncation lands mid-codepoint) and a flag.
pub fn truncate_for_preview(bytes: &[u8], max: usize) -> (String, bool) {
    if bytes.len() <= max {
        (String::from_utf8_lossy(bytes).into_owned(), false)
    } else {
        (String::from_utf8_lossy(&bytes[..max]).into_owned(), true)
    }
}

/// Convenience: try to parse a body as JSON. If parsing fails (e.g. when the
/// body is truncated mid-object), fall back to storing the raw truncated string
/// as a JSON `Value::String` so the log still has something human-readable.
pub fn body_preview_json(raw: &[u8], max: usize) -> (Option<serde_json::Value>, bool) {
    let (preview, truncated) = truncate_for_preview(raw, max);
    match serde_json::from_str::<serde_json::Value>(&preview) {
        Ok(v) => (Some(v), truncated),
        Err(_) => (Some(serde_json::Value::String(preview)), truncated),
    }
}

/// 全量 JSON body 的单侧保护上限截断：正常大小原样保留；超过上限时取序列化
/// 前缀（截断可能落在 UTF-8 边界或 JSON 中途，能解析则存 JSON，否则存字符串
/// 预览），并置 truncated 标志。
fn truncate_full_json(
    value: Option<serde_json::Value>,
    max: usize,
) -> (Option<serde_json::Value>, bool) {
    let Some(value) = value else {
        return (None, false);
    };
    let serialized = serde_json::to_string(&value).unwrap_or_default();
    if serialized.len() <= max {
        return (Some(value), false);
    }
    let preview = String::from_utf8_lossy(&serialized.as_bytes()[..max]).into_owned();
    match serde_json::from_str::<serde_json::Value>(&preview) {
        Ok(parsed) => (Some(parsed), true),
        Err(_) => (Some(serde_json::Value::String(preview)), true),
    }
}

/// 把内存预览条目 + 全量 body 载荷组装为磁盘全量条目。
///
/// body 按 [`DISK_BODY_MAX_BYTES`] 做单侧保护上限截断：正常大小原样全量落盘，
/// 仅异常超大 body 截断并置 truncated 标志，防止日志文件被单条记录撑爆。
pub fn build_disk_entry(entry: &LogEntry, full: LogEntryFull) -> DiskLogEntry {
    let (inbound_body, inbound_body_truncated) =
        truncate_full_json(full.inbound_body, DISK_BODY_MAX_BYTES);
    let (upstream_body, upstream_body_truncated) = match &full.upstream_body {
        Some(body) => (
            Some(truncate_for_preview(body.as_bytes(), DISK_BODY_MAX_BYTES).0),
            body.len() > DISK_BODY_MAX_BYTES,
        ),
        None => (None, false),
    };

    DiskLogEntry {
        id: entry.id,
        timestamp_ms: entry.timestamp_ms,
        protocol: entry.protocol,
        inbound_method: entry.inbound_method.clone(),
        inbound_path: entry.inbound_path.clone(),
        inbound_headers: entry.inbound_headers.clone(),
        inbound_body,
        inbound_body_truncated,
        inbound_model: entry.inbound_model.clone(),
        upstream_model: entry.upstream_model.clone(),
        provider_id: entry.provider_id.clone(),
        provider_name: entry.provider_name.clone(),
        upstream_url: entry.upstream_url.clone(),
        upstream_status: entry.upstream_status,
        upstream_headers: entry.upstream_headers.clone(),
        upstream_body,
        upstream_body_truncated,
        stream: entry.stream,
        duration_ms: entry.duration_ms,
        success: entry.success,
        error: entry.error.clone(),
        full_body: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_credential_headers() {
        assert_eq!(
            sanitize_header_value("Authorization", "Bearer secret"),
            "***"
        );
        assert_eq!(sanitize_header_value("x-api-key", "secret"), "***");
        assert_eq!(
            sanitize_header_value("content-type", "application/json"),
            "application/json"
        );
    }

    #[test]
    fn masks_credential_query_parameters() {
        assert_eq!(
            sanitize_url_for_log(
                "https://example.test/v1?api_key=secret&model=demo&access-token=secret#frag"
            ),
            "https://example.test/v1?api_key=***&model=demo&access-token=***#frag"
        );
    }

    #[test]
    fn redacts_nested_credential_fields() {
        let value = serde_json::json!({
            "prompt": "keep this",
            "api-key": "secret",
            "headers": {"authorization": "Bearer secret", "cookie": "session"},
            "items": [{"access_token": "secret"}],
        });
        let redacted = redact_json_for_log(&value);

        assert_eq!(redacted["prompt"], "keep this");
        assert_eq!(redacted["api-key"], "***");
        assert_eq!(redacted["headers"]["authorization"], "***");
        assert_eq!(redacted["headers"]["cookie"], "***");
        assert_eq!(redacted["items"][0]["access_token"], "***");
    }

    #[test]
    fn inbound_protocol_serializes_to_frontend_contract() {
        // 前端 InboundProtocol 类型与 PROTOCOL_LABEL 使用 camelCase 值；
        // 这里把序列化契约钉死，防止再次漂移导致 UI 显示小写原文。
        let serialized: Vec<String> = [
            InboundProtocol::ClaudeMessages,
            InboundProtocol::CodexResponses,
            InboundProtocol::OpenAiChatCompletions,
            InboundProtocol::OpenAiModelsList,
        ]
        .iter()
        .map(|protocol| serde_json::to_string(protocol).unwrap())
        .collect();
        assert_eq!(
            serialized,
            vec![
                "\"claudeMessages\"",
                "\"codexResponses\"",
                "\"openAiChatCompletions\"",
                "\"openaiModelsList\"",
            ]
        );
    }

    #[tokio::test]
    async fn concurrent_push_assigns_ordered_ids_and_respects_capacity() {
        let store = LogStore::with_capacity(32);
        let mut tasks = Vec::new();
        for index in 0..64 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .push(
                        LogEntry {
                            id: 0,
                            timestamp_ms: index,
                            protocol: InboundProtocol::ClaudeMessages,
                            inbound_method: "POST".into(),
                            inbound_path: "/v1/messages".into(),
                            inbound_headers: Vec::new(),
                            inbound_body: None,
                            inbound_body_truncated: false,
                            inbound_model: None,
                            upstream_model: None,
                            provider_id: None,
                            provider_name: None,
                            upstream_url: None,
                            upstream_status: None,
                            upstream_headers: Vec::new(),
                            upstream_body: None,
                            upstream_body_truncated: false,
                            stream: false,
                            duration_ms: 0,
                            success: true,
                            error: None,
                        },
                        LogEntryFull::none(),
                    )
                    .await;
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        let ids: Vec<u64> = store
            .snapshot()
            .await
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(ids, (33..=64).collect::<Vec<_>>());
    }

    #[test]
    fn truncate_full_json_keeps_small_and_truncates_large() {
        // 正常大小：原样保留，不置截断
        let small = serde_json::json!({ "prompt": "hello", "nested": [1, 2, 3] });
        let (kept, truncated) = truncate_full_json(Some(small.clone()), 4096);
        assert_eq!(kept, Some(small));
        assert!(!truncated);

        // 超大：截断到上限并置标志
        let big_value = serde_json::json!({ "prompt": "x".repeat(8192) });
        let (cut, truncated) = truncate_full_json(Some(big_value), 1024);
        assert!(truncated);
        assert!(cut.is_some());
        let text = match cut.unwrap() {
            serde_json::Value::String(s) => s,
            other => serde_json::to_string(&other).unwrap(),
        };
        assert!(text.len() <= 1024);
    }

    #[test]
    fn disk_entry_roundtrip_and_legacy_default() {
        let entry = LogEntry {
            id: 42,
            timestamp_ms: 1000,
            protocol: InboundProtocol::CodexResponses,
            inbound_method: "POST".into(),
            inbound_path: "/v1/responses".into(),
            inbound_headers: vec![["authorization".into(), "***".into()]],
            inbound_body: Some(serde_json::json!({ "input": "full prompt here", "stream": false })),
            inbound_body_truncated: false,
            inbound_model: Some("gpt-4o".into()),
            upstream_model: Some("gpt-4o".into()),
            provider_id: Some("p1".into()),
            provider_name: Some("Provider One".into()),
            upstream_url: Some("https://example.test/v1/responses".into()),
            upstream_status: Some(200),
            upstream_headers: vec![["content-type".into(), "application/json".into()]],
            upstream_body: Some("full response body".into()),
            upstream_body_truncated: false,
            stream: false,
            duration_ms: 123,
            success: true,
            error: None,
        };
        let full = LogEntryFull {
            inbound_body: Some(serde_json::json!({ "input": "full prompt here", "stream": false })),
            upstream_body: Some("full response body".into()),
        };
        let disk = build_disk_entry(&entry, full);
        assert!(disk.full_body);

        // 新格式：序列化 → 反序列化 roundtrip，body 全量保留
        let json = serde_json::to_string(&disk).unwrap();
        let back: DiskLogEntry = serde_json::from_str(&json).unwrap();
        assert!(back.full_body);
        assert_eq!(
            back.inbound_body,
            Some(serde_json::json!({ "input": "full prompt here", "stream": false }))
        );
        assert_eq!(back.upstream_body.as_deref(), Some("full response body"));

        // 旧格式行（无 fullBody 字段）反序列化：full_body 默认 false
        let legacy: DiskLogEntry = serde_json::from_str(
            r#"{"id":1,"timestampMs":0,"protocol":"claudeMessages","inboundMethod":"POST","inboundPath":"/v1/messages","inboundHeaders":[],"inboundBody":null,"inboundBodyTruncated":false,"inboundModel":null,"upstreamModel":null,"providerId":null,"providerName":null,"upstreamUrl":null,"upstreamStatus":null,"upstreamHeaders":[],"upstreamBody":null,"upstreamBodyTruncated":false,"stream":false,"durationMs":0,"success":true,"error":null}"#,
        )
        .unwrap();
        assert!(!legacy.full_body);
    }
}
