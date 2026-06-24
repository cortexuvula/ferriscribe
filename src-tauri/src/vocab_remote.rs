//! HTTP client for the office server's vocab CRUD API.
//!
//! Used by the client side of the sharing feature: when a paired connection
//! is present and the office server advertised a `vocab_port`, vocabulary
//! Tauri commands route through here instead of the local SQLite repo so
//! the server stays the canonical source of truth.

use medical_core::error::{AppError, AppResult};
use medical_core::types::endpoint::http_url;
use medical_core::types::vocabulary::VocabularyEntry;
use serde::Serialize;
use uuid::Uuid;

use crate::commands::sharing::PairedConnection;

/// HTTP client for the office server's `/v1/vocabulary` CRUD API.
///
/// Created via [`VocabRemote::from`] when a paired connection is present.
/// All methods send bearer-authenticated requests and return `AppResult`.
pub struct VocabRemote<'a> {
    pub conn: &'a PairedConnection,
    pub bearer: String,
    pub client: std::sync::Arc<reqwest::Client>,
}

impl<'a> VocabRemote<'a> {
    /// Create a `VocabRemote` if the paired connection has a vocab port AND a
    /// reachable address (LAN preferred, Tailscale fallback) and a bearer is
    /// available. Returns `None` when the office server predates the vocab-sync
    /// feature -- callers fall back to local DB operations.
    pub fn from(
        conn: &'a PairedConnection,
        bearer: Option<String>,
        client: std::sync::Arc<reqwest::Client>,
    ) -> Option<Self> {
        let bearer = bearer?;
        // No vocab_port → office server is older than the vocab-sync feature.
        conn.ports.vocab?;
        Some(Self {
            conn,
            bearer,
            client,
        })
    }

    fn base_url(&self) -> Option<String> {
        let port = self.conn.ports.vocab?;
        // Prefer LAN; fall back to Tailscale. We don't probe reachability —
        // the request itself does, and reqwest's connect timeout will surface
        // a useful error.
        let host = self
            .conn
            .lan
            .as_deref()
            .or(self.conn.tailscale.as_deref())?;
        Some(http_url(host, port))
    }

    /// List vocabulary entries from the office server, optionally filtered by
    /// category.
    pub async fn list(&self, category: Option<&str>) -> AppResult<Vec<VocabularyEntry>> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let mut url = format!("{base}/v1/vocabulary");
        if let Some(c) = category {
            url.push_str(&format!("?category={}", urlencoding::encode(c)));
        }
        let resp = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("vocab list: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<VocabularyEntry>>()
            .await
            .map_err(|e| AppError::Other(format!("vocab list parse: {e}")))
    }

    /// Get vocabulary counts as `(total, enabled)`.
    pub async fn count(&self) -> AppResult<(u32, u32)> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/vocabulary/count");
        let resp = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("vocab count: {e}")))?;
        check_status(&resp).await?;
        resp.json::<(u32, u32)>()
            .await
            .map_err(|e| AppError::Other(format!("vocab count parse: {e}")))
    }

    /// Insert a new vocabulary entry on the office server.
    pub async fn insert(&self, body: &UpsertBody) -> AppResult<VocabularyEntry> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/vocabulary");
        let resp = self
            .client
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(body)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("vocab insert: {e}")))?;
        check_status(&resp).await?;
        resp.json::<VocabularyEntry>()
            .await
            .map_err(|e| AppError::Other(format!("vocab insert parse: {e}")))
    }

    /// Replace an existing vocabulary entry by UUID.
    pub async fn update(&self, id: Uuid, body: &UpsertBody) -> AppResult<VocabularyEntry> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/vocabulary/{id}");
        let resp = self
            .client
            .put(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(body)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("vocab update: {e}")))?;
        check_status(&resp).await?;
        resp.json::<VocabularyEntry>()
            .await
            .map_err(|e| AppError::Other(format!("vocab update parse: {e}")))
    }

    /// Delete a single vocabulary entry by UUID.
    pub async fn delete(&self, id: Uuid) -> AppResult<()> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/vocabulary/{id}");
        let resp = self
            .client
            .delete(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("vocab delete: {e}")))?;
        check_status(&resp).await?;
        Ok(())
    }

    /// Delete all vocabulary entries on the office server.
    pub async fn delete_all(&self) -> AppResult<u32> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/vocabulary");
        let resp = self
            .client
            .delete(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("vocab delete_all: {e}")))?;
        check_status(&resp).await?;
        resp.json::<u32>()
            .await
            .map_err(|e| AppError::Other(format!("vocab delete_all parse: {e}")))
    }
}

#[derive(Debug, Serialize)]
pub struct UpsertBody {
    pub find_text: String,
    pub replacement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_sensitive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

async fn check_status(resp: &reqwest::Response) -> AppResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::Other(
            "Office server does not support vocabulary sync (update it to v0.10.31 or later)."
                .to_string(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::Other(
            "Office server rejected the bearer token. Try unpair → re-pair from this client."
                .to_string(),
        ));
    }
    Err(AppError::Other(format!("vocab API: HTTP {status}")))
}
