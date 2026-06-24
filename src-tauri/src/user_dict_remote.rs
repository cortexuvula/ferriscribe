//! HTTP client for the office server's user-dictionary CRUD API.
//!
//! Mirrors `vocab_remote.rs`: when a paired connection is present and the
//! office server advertised a `vocab_port`, the user-dictionary Tauri
//! commands route through here instead of the local SQLite repo so the
//! server stays the canonical source of truth.

use medical_core::error::{AppError, AppResult};
use medical_core::types::endpoint::http_url;
use serde::Serialize;

use crate::commands::sharing::PairedConnection;

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
}

#[derive(Debug, Serialize)]
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
