//! HTTP client for the office server's condition-chips API.
//!
//! Used by the client side of the sharing feature: when a paired connection
//! is present and the office server advertised a vocab port (which also
//! hosts /v1/condition-chips), the condition-chip Tauri commands route
//! through here instead of the local repo so the server stays the
//! canonical source of truth.

use std::time::Duration;

use futures_util::StreamExt;
use medical_core::error::{AppError, AppResult};
use medical_core::types::condition_chip::ConditionChip;
use medical_core::types::endpoint::http_url;

use crate::commands::sharing::PairedConnection;

/// HTTP client for the office server's `/v1/condition-chips` API.
///
/// Created via [`ConditionsRemote::from`] when a paired connection is present.
/// All methods send bearer-authenticated requests and return `AppResult`.
pub struct ConditionsRemote<'a> {
    pub conn: &'a PairedConnection,
    pub bearer: String,
    pub client: std::sync::Arc<reqwest::Client>,
}

impl<'a> ConditionsRemote<'a> {
    /// Create a `ConditionsRemote` if the paired connection supports chip
    /// sync (has a vocab port and a bearer token). Returns `None` when the
    /// office server predates the chip sync feature or no bearer is
    /// available -- callers fall back to local DB operations.
    pub fn from(
        conn: &'a PairedConnection,
        bearer: Option<String>,
        client: std::sync::Arc<reqwest::Client>,
    ) -> Option<Self> {
        let bearer = bearer?;
        // Condition chips ride on the same port as the vocab API. Absence
        // means the office server predates the chip-sync release; treat
        // that as "chip sync unavailable" and fall back to local.
        conn.ports.vocab?;
        Some(Self {
            conn,
            bearer,
            client,
        })
    }

    fn base_url(&self) -> Option<String> {
        let port = self.conn.ports.vocab?;
        let host = self
            .conn
            .lan
            .as_deref()
            .or(self.conn.tailscale.as_deref())?;
        Some(http_url(host, port))
    }

    /// List all active condition chips from the office server.
    pub async fn list(&self) -> AppResult<Vec<ConditionChip>> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/condition-chips");
        let resp = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("conditions list: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<ConditionChip>>()
            .await
            .map_err(|e| AppError::Other(format!("conditions list parse: {e}")))
    }

    /// Push local chips and receive the server-merged list back.
    ///
    /// The server applies per-item last-write-wins merge against its own
    /// rows and returns the full resulting list.
    pub async fn sync(&self, local_chips: Vec<ConditionChip>) -> AppResult<Vec<ConditionChip>> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/condition-chips/sync");
        let resp = self
            .client
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(&local_chips)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("conditions sync: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<ConditionChip>>()
            .await
            .map_err(|e| AppError::Other(format!("conditions sync parse: {e}")))
    }

    /// Subscribe to SSE change notifications from the office server.
    ///
    /// Returns a stream that yields `()` for each `data: changed` event pushed
    /// by the server's `/v1/condition-chips/events` endpoint. The stream stays
    /// open until the connection drops or the server closes it; callers should
    /// wrap it in a reconnect loop with backoff. The request uses a long
    /// timeout (300s) because SSE is a long-lived connection — reqwest will
    /// keep the response body streaming, and each server push resets the idle
    /// window.
    ///
    /// The `data: connected` initial event is filtered out (only `changed`
    /// events surface to the caller).
    pub async fn subscribe_events(&self) -> AppResult<impl futures_util::Stream<Item = ()>> {
        let url = format!(
            "{}/v1/condition-chips/events",
            self.base_url().ok_or_else(|| {
                AppError::Other("no vocab base URL for conditions remote".into())
            })?
        );
        let resp = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(300))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("SSE connect: {e}")))?;
        if !resp.status().is_success() {
            return Err(AppError::Other(format!(
                "SSE connect failed: {}",
                resp.status()
            )));
        }
        let stream = resp.bytes_stream().filter_map(|chunk| async move {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    for line in text.lines() {
                        if line.starts_with("data: changed") {
                            return Some(());
                        }
                    }
                    None
                }
                Err(_) => None,
            }
        });
        Ok(stream)
    }
}

async fn check_status(resp: &reqwest::Response) -> AppResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::Other(
            "Office server does not support condition-chip sync (update it to a later release)."
                .to_string(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::Other(
            "Office server rejected the bearer token. Try unpair → re-pair from this client."
                .to_string(),
        ));
    }
    if status == reqwest::StatusCode::CONFLICT {
        return Err(AppError::Other(
            "A condition chip conflict occurred on the office server.".to_string(),
        ));
    }
    Err(AppError::Other(format!("conditions API: HTTP {status}")))
}
