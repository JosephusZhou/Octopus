//! Route aggregation (local proxy server): aggregates multiple AI providers
//! behind a single local endpoint with automatic failover and request cloaking.
//!
//! 仅 passthrough：A→A（Claude Code → Anthropic 兼容 provider）+ OR→OR
//! （Codex CLI → OpenAI Responses 兼容 provider）+ CC→CC（OpenAI SDK →
//! OpenAI Chat Completions 兼容 provider）。路由聚合不做协议翻译。
//!
//! Architecture:
//! - `server` — Axum HTTP server lifecycle (start/stop/config)
//! - `router` — Route registration + request dispatch
//! - `handler` — Request entry points (Claude messages / Codex responses)
//! - `forwarder` — Request forwarding with failover + header injection + SSE passthrough
//! - `provider_router` — Provider selection + circuit breaker management
//! - `circuit_breaker` — Three-state circuit breaker (Closed/Open/HalfOpen)
//! - `cloaking` — Claude Code rectifier + Codex client simulation
//!   （仿照 CLIProxyAPI `internal/runtime/executor/`，由 check_upstream_sync.py 的
//!   client_fingerprint_anchors 跟踪同步状态；详见 sync/SYNC_PLAYBOOK.md §2）
//! - `config` — RouteAggregationConfig load/save (stored in config.json)
//! - `types` — Data structures (DTOs for Tauri commands)
//! - `log` / `logfile` — In-memory ring buffer + on-disk tail-friendly log

pub mod circuit_breaker;
pub mod cloaking;
pub mod config;
pub mod forwarder;
pub mod handler;
pub mod log;
pub mod logfile;
pub mod provider_router;
pub mod router;
pub mod server;
pub mod types;

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub use types::*;

// Re-export key types used across modules and in lib.rs/config.rs.
pub use circuit_breaker::CircuitBreakerSnapshot;
pub use config::RouteAggregationConfig;
pub use log::{LogEntry, LogStore};
pub use logfile::LogFile;

/// Global route aggregation state, shared across Tauri commands via `app.manage()`.
pub struct RouteAggregationState {
    /// Serialize start/stop/reload and config/key writes; never hold config/server locks while stopping.
    pub lifecycle: Mutex<()>,
    /// Server instance (present when running, None when stopped).
    pub server: RwLock<Option<Arc<server::RouteAggregationServer>>>,
    /// Config snapshot — shared with Axum handlers via AppState so that runtime
    /// config updates (e.g. toggling a route group) are visible without restart.
    pub config: Arc<RwLock<RouteAggregationConfig>>,
    /// Provider router (holds circuit breaker state, survives server stop).
    pub provider_router: Arc<provider_router::ProviderRouter>,
    /// In-memory ring buffer of inbound/upstream request logs. Used by the UI's
    /// "进出日志" list.
    pub log_store: LogStore,
}

impl RouteAggregationState {
    pub fn new(config: RouteAggregationConfig) -> Self {
        Self {
            lifecycle: Mutex::new(()),
            server: RwLock::new(None),
            config: Arc::new(RwLock::new(config)),
            provider_router: Arc::new(provider_router::ProviderRouter::new()),
            log_store: LogStore::new(),
        }
    }

    /// Caller holds lifecycle. Preserve the current running/stopped state; autoStart
    /// remains a preference for the next launch, not permission to start on restore.
    pub(crate) async fn apply_runtime_config(
        &self,
        config: RouteAggregationConfig,
    ) -> Result<(), String> {
        let old = self.config.read().await.clone();
        let restart =
            old.listen_address != config.listen_address || old.listen_port != config.listen_port;
        // Keep the old config in active handlers until the old listener has drained.
        if restart {
            let previous = self.server.write().await.take();
            if let Some(server) = previous {
                server.stop().await;
                *self.config.write().await = config;
                let result = server::RouteAggregationServer::start(
                    self.config.clone(),
                    self.provider_router.clone(),
                    self.log_store.clone(),
                )
                .await;
                match result {
                    Ok(server) => *self.server.write().await = Some(Arc::new(server)),
                    Err(error) => {
                        return Err(format!(
                        "路由配置已重载，但服务未启动：{error}。请释放端口或修改监听设置后重新启动"
                    ))
                    }
                }
                return Ok(());
            }
        }
        // No endpoint change: handlers see new auth/cloaking/failover immediately,
        // without interrupting existing connections or starting a stopped service.
        *self.config.write().await = config;
        Ok(())
    }

    /// Caller holds lifecycle. All route groups must use the restored provider DB.
    pub(crate) async fn refresh_restored_pools(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for group in RouteGroup::ALL {
            if let Err(error) = self.provider_router.refresh_pool_fast(group).await {
                // Never leave an old provider/credential pool active after failed restore refresh.
                self.provider_router.clear_pool(group).await;
                errors.push(format!(
                    "路由 {group:?} 供应商池重载失败，已清空旧池：{error}"
                ));
            }
        }
        errors
    }

    /// Build a status snapshot for the frontend.
    pub async fn get_status(&self) -> RouteAggregationStatus {
        let config = self.config.read().await;
        let server_running = self.server.read().await.is_some();
        let providers = self.provider_router.get_merged_statuses().await;

        RouteAggregationStatus {
            server_running,
            listen_address: config.listen_address.clone(),
            listen_port: config.listen_port,
            providers,
        }
    }
}

#[cfg(test)]
mod runtime_reload_tests {
    use super::*;
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    async fn running() -> RouteAggregationState {
        let config = RouteAggregationConfig {
            listen_port: free_port(),
            api_keys: vec!["old-test-placeholder".into()],
            ..Default::default()
        };
        let state = RouteAggregationState::new(config);
        let server = server::RouteAggregationServer::start(
            state.config.clone(),
            state.provider_router.clone(),
            state.log_store.clone(),
        )
        .await
        .unwrap();
        *state.server.write().await = Some(Arc::new(server));
        state
    }
    async fn stop(state: &RouteAggregationState) {
        let server = state.server.write().await.take();
        if let Some(server) = server {
            server.stop().await;
        }
    }
    #[tokio::test]
    async fn restored_key_and_behavior_update_without_restarting_listener() {
        let state = running().await;
        let original = state.server.read().await.as_ref().unwrap().clone();
        let mut config = state.config.read().await.clone();
        let port = config.listen_port;
        config.api_keys = vec!["new-test-placeholder".into()];
        config.max_retries = 1;
        config.claude_strict_mode = true;
        let _guard = state.lifecycle.lock().await;
        state.apply_runtime_config(config).await.unwrap();
        assert!(Arc::ptr_eq(
            &original,
            state.server.read().await.as_ref().unwrap()
        ));
        let response = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/v1/models"))
            .bearer_auth("old-test-placeholder")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(
            state.config.read().await.api_keys,
            vec!["new-test-placeholder"]
        );
        assert_eq!(state.config.read().await.max_retries, 1);
        assert!(state.config.read().await.claude_strict_mode);
        stop(&state).await;
    }
    #[tokio::test]
    async fn restored_endpoint_releases_old_port_and_binds_new_port() {
        let state = running().await;
        let old_port = state.config.read().await.listen_port;
        let mut config = state.config.read().await.clone();
        config.listen_port = free_port();
        let new_port = config.listen_port;
        let _guard = state.lifecycle.lock().await;
        state.apply_runtime_config(config).await.unwrap();
        assert!(state.server.read().await.is_some());
        assert!(std::net::TcpListener::bind(("127.0.0.1", old_port)).is_ok());
        let response = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{new_port}/v1/models"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        stop(&state).await;
    }
    #[tokio::test]
    async fn stopped_service_stays_stopped_even_if_restored_auto_start_enabled() {
        let state = RouteAggregationState::new(RouteAggregationConfig::default());
        let config = RouteAggregationConfig {
            listen_port: free_port(),
            auto_start: true,
            api_keys: vec!["restored-test-placeholder".into()],
            ..Default::default()
        };
        let port = config.listen_port;
        let _guard = state.lifecycle.lock().await;
        state.apply_runtime_config(config).await.unwrap();
        assert!(state.server.read().await.is_none());
        assert!(state.config.read().await.auto_start);
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
    }
    #[tokio::test]
    async fn occupied_restored_port_reports_stopped_not_old_listener_running() {
        let state = running().await;
        let old_port = state.config.read().await.listen_port;
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = occupied.local_addr().unwrap().port();
        let mut config = state.config.read().await.clone();
        config.listen_port = port;
        let _guard = state.lifecycle.lock().await;
        let error = state.apply_runtime_config(config).await.unwrap_err();
        assert!(error.contains("服务未启动"));
        assert!(state.server.read().await.is_none());
        assert_eq!(state.config.read().await.listen_port, port);
        assert!(std::net::TcpListener::bind(("127.0.0.1", old_port)).is_ok());
        drop(occupied);
    }
}
