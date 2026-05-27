//! HTTP client for the office server's context-templates API.
//!
//! Used by the client side of the sharing feature: when a paired connection
//! is present and the office server advertised a vocab port (which also
//! hosts /v1/context-templates), the context-templates Tauri commands
//! route through here instead of the local SettingsRepo so the server
//! stays the canonical source of truth.

use medical_core::error::{AppError, AppResult};
use medical_core::types::endpoint::http_url;
use medical_core::types::settings::ContextTemplate;
use serde::Serialize;

use crate::commands::sharing::PairedConnection;

/// HTTP client for the office server's `/v1/context-templates` API.
///
/// Created via [`TemplatesRemote::from`] when a paired connection is present.
/// All methods send bearer-authenticated requests and return `AppResult`.
pub struct TemplatesRemote<'a> {
    pub conn: &'a PairedConnection,
    pub bearer: String,
    pub client: std::sync::Arc<reqwest::Client>,
}

impl<'a> TemplatesRemote<'a> {
    /// Create a `TemplatesRemote` if the paired connection supports template
    /// sync (has a vocab port and a bearer token). Returns `None` when the
    /// office server predates the template sync feature or no bearer is
    /// available -- callers fall back to local DB operations.
    pub fn from(
        conn: &'a PairedConnection,
        bearer: Option<String>,
        client: std::sync::Arc<reqwest::Client>,
    ) -> Option<Self> {
        let bearer = bearer?;
        // Templates ride on the same port as the vocab API. Absence means
        // the office server predates the v0.10.31 vocab-sync release;
        // treat that as "templates sync unavailable" and fall back to
        // local.
        conn.ports.vocab?;
        Some(Self { conn, bearer, client })
    }

    fn base_url(&self) -> Option<String> {
        let port = self.conn.ports.vocab?;
        let host = self.conn.lan.as_deref().or(self.conn.tailscale.as_deref())?;
        Some(http_url(host, port))
    }

    /// List all context templates from the office server, sorted by name.
    pub async fn list(&self) -> AppResult<Vec<ContextTemplate>> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/context-templates");
        let resp = self.client
            .get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("templates list: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<ContextTemplate>>()
            .await
            .map_err(|e| AppError::Other(format!("templates list parse: {e}")))
    }

    /// Create or update a context template by name.
    pub async fn upsert(&self, name: &str, body: &str) -> AppResult<ContextTemplate> {
        #[derive(Serialize)]
        struct B<'a> { name: &'a str, body: &'a str }
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/context-templates/upsert");
        let resp = self.client
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(&B { name, body })
            .send()
            .await
            .map_err(|e| AppError::Other(format!("templates upsert: {e}")))?;
        check_status(&resp).await?;
        resp.json::<ContextTemplate>()
            .await
            .map_err(|e| AppError::Other(format!("templates upsert parse: {e}")))
    }

    /// Rename a context template (atomic rename on the server).
    pub async fn rename(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> AppResult<ContextTemplate> {
        #[derive(Serialize)]
        struct B<'a> { old_name: &'a str, new_name: &'a str }
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/context-templates/rename");
        let resp = self.client
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(&B { old_name, new_name })
            .send()
            .await
            .map_err(|e| AppError::Other(format!("templates rename: {e}")))?;
        check_status(&resp).await?;
        resp.json::<ContextTemplate>()
            .await
            .map_err(|e| AppError::Other(format!("templates rename parse: {e}")))
    }

    /// Delete a context template by name.
    pub async fn delete(&self, name: &str) -> AppResult<()> {
        #[derive(Serialize)]
        struct B<'a> { name: &'a str }
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/context-templates/delete");
        let resp = self.client
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(&B { name })
            .send()
            .await
            .map_err(|e| AppError::Other(format!("templates delete: {e}")))?;
        check_status(&resp).await?;
        Ok(())
    }
}

async fn check_status(resp: &reqwest::Response) -> AppResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::Other(
            "Office server does not support context-templates sync (update it to v0.10.34 or later)."
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
            "A template with that name already exists on the office server.".to_string(),
        ));
    }
    Err(AppError::Other(format!("templates API: HTTP {status}")))
}
