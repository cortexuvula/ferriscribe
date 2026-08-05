//! HTTP client for the office server's user-dictionary CRUD + sync API.
//!
//! Mirrors `vocab_remote.rs` and `conditions_remote.rs`: when a paired
//! connection is present and the office server advertised a `vocab_port`,
//! the user-dictionary Tauri commands route through here instead of the
//! local SQLite repo so the server stays the canonical source of truth.
//!
//! In addition to the original list/add/remove CRUD, this client exposes a
//! `sync` (two-way merge) method and a `subscribe_events` (SSE) method that
//! mirror `conditions_remote`.

use std::time::Duration;

use futures_util::StreamExt;
use medical_core::error::{AppError, AppResult};
use medical_core::types::endpoint::http_url;
use medical_core::types::user_dict_entry::UserDictEntry;
use serde::Serialize;

use crate::commands::sharing::PairedConnection;

/// Build a dedicated `reqwest::Client` for a long-lived SSE stream (no total
/// timeout; liveness via server keep-alive + caller reconnect loop). See
/// [`UserDictRemote::subscribe_events`] for the rationale.
fn sse_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .tcp_keepalive(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// HTTP client for the office server's `/v1/user-dictionary` CRUD API.
///
/// Created via [`UserDictRemote::from`] when a paired connection is present.
/// The dictionary API rides on the same port as the vocab API.
pub struct UserDictRemote<'a> {
    pub conn: &'a PairedConnection,
    pub bearer: String,
    pub client: std::sync::Arc<reqwest::Client>,
}

impl<'a> UserDictRemote<'a> {
    /// Returns `Some(...)` when the paired connection has a `vocab_port`
    /// (the dictionary API rides on the same port as the vocab API) AND a
    /// bearer is available. Otherwise `None` — caller falls back to local DB.
    pub fn from(
        conn: &'a PairedConnection,
        bearer: Option<String>,
        client: std::sync::Arc<reqwest::Client>,
    ) -> Option<Self> {
        let bearer = bearer?;
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

    /// List all words in the user dictionary.
    ///
    /// (Legacy CRUD endpoint. The bidirectional-sync path uses [`Self::sync`]
    /// instead, which both pushes local changes and returns the merged list.
    /// Retained for callers that only want a read-only pull without pushing.)
    #[allow(dead_code)]
    pub async fn list(&self) -> AppResult<Vec<String>> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no dictionary address".into()))?;
        let url = format!("{base}/v1/user-dictionary");
        let resp = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("dict list: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<String>>()
            .await
            .map_err(|e| AppError::Other(format!("dict list parse: {e}")))
    }

    /// Add a word to the user dictionary. Returns `true` if inserted.
    ///
    /// (Legacy CRUD endpoint. The sync path writes locally then pushes via
    /// [`Self::sync`]. Retained for direct server-side mutation.)
    #[allow(dead_code)]
    pub async fn add(&self, word: &str) -> AppResult<bool> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no dictionary address".into()))?;
        let url = format!("{base}/v1/user-dictionary");
        let body = AddBody {
            word: word.to_string(),
        };
        let resp = self
            .client
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("dict add: {e}")))?;
        check_status(&resp).await?;
        resp.json::<bool>()
            .await
            .map_err(|e| AppError::Other(format!("dict add parse: {e}")))
    }

    /// Remove a word from the user dictionary. Returns `true` if deleted.
    ///
    /// (Legacy CRUD endpoint. The sync path soft-deletes locally then pushes
    /// via [`Self::sync`]. Retained for direct server-side mutation.)
    #[allow(dead_code)]
    pub async fn remove(&self, word: &str) -> AppResult<bool> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no dictionary address".into()))?;
        let encoded = urlencoding::encode(word);
        let url = format!("{base}/v1/user-dictionary/{encoded}");
        let resp = self
            .client
            .delete(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("dict remove: {e}")))?;
        check_status(&resp).await?;
        resp.json::<bool>()
            .await
            .map_err(|e| AppError::Other(format!("dict remove parse: {e}")))
    }

    /// Push local entries (active + tombstones) and receive the server-merged
    /// active word list back.
    ///
    /// The server applies per-item last-write-wins merge against its own rows
    /// and returns the full resulting active list (as `Vec<String>` of words).
    pub async fn sync(&self, local_entries: Vec<UserDictEntry>) -> AppResult<Vec<String>> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no dictionary address".into()))?;
        let url = format!("{base}/v1/user-dictionary/sync");
        let resp = self
            .client
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(&local_entries)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("dict sync: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<String>>()
            .await
            .map_err(|e| AppError::Other(format!("dict sync parse: {e}")))
    }

    /// Subscribe to SSE change notifications from the office server.
    ///
    /// Returns a stream that yields `()` for each `data: changed` event pushed
    /// by the server's `/v1/user-dictionary/events` endpoint. The stream
    /// stays open until the connection drops or the server closes it; callers
    /// should wrap it in a reconnect loop with backoff.
    ///
    /// **No total timeout is set on the SSE request.** reqwest's `.timeout()`
    /// is a hard total deadline from request start (it does NOT reset on
    /// stream chunks). Capping at 300s — as before — forced a reconnect every
    /// 5 min. Liveness is maintained by the server's keep-alive comments +
    /// the caller's reconnect loop. A dedicated client is built here because
    /// the shared client carries a 30s total timeout that would cap the
    /// stream.
    ///
    /// The `data: connected` initial event is filtered out (only `changed`
    /// events surface to the caller).
    pub async fn subscribe_events(&self) -> AppResult<impl futures_util::Stream<Item = ()>> {
        let url = format!(
            "{}/v1/user-dictionary/events",
            self.base_url()
                .ok_or_else(|| { AppError::Other("no vocab base URL for dict remote".into()) })?
        );
        let resp = sse_client()
            .get(&url)
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("dict SSE connect: {e}")))?;
        if !resp.status().is_success() {
            return Err(AppError::Other(format!(
                "dict SSE connect failed: {}",
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

#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct AddBody {
    word: String,
}

async fn check_status(resp: &reqwest::Response) -> AppResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::Other(
            "Office server does not support dictionary sync (update it to v0.10.84 or later)."
                .to_string(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::Other(
            "Office server rejected the bearer token. Try unpair → re-pair from this client."
                .to_string(),
        ));
    }
    Err(AppError::Other(format!("dictionary API: HTTP {status}")))
}
