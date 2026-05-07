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

pub struct TemplatesRemote<'a> {
    pub conn: &'a PairedConnection,
    pub bearer: String,
}

impl<'a> TemplatesRemote<'a> {
    pub fn from(
        conn: &'a PairedConnection,
        bearer: Option<String>,
    ) -> Option<Self> {
        let bearer = bearer?;
        // Templates ride on the same port as the vocab API. Absence means
        // the office server predates the v0.10.31 vocab-sync release;
        // treat that as "templates sync unavailable" and fall back to
        // local.
        conn.ports.vocab?;
        Some(Self { conn, bearer })
    }

    fn base_url(&self) -> Option<String> {
        let port = self.conn.ports.vocab?;
        let host = self.conn.lan.as_deref().or(self.conn.tailscale.as_deref())?;
        Some(http_url(host, port))
    }

    fn client() -> AppResult<reqwest::Client> {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| AppError::Other(format!("templates_remote http client: {e}")))
    }

    pub async fn list(&self) -> AppResult<Vec<ContextTemplate>> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/context-templates");
        let resp = Self::client()?
            .get(&url)
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("templates list: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<ContextTemplate>>()
            .await
            .map_err(|e| AppError::Other(format!("templates list parse: {e}")))
    }

    pub async fn upsert(&self, name: &str, body: &str) -> AppResult<ContextTemplate> {
        #[derive(Serialize)]
        struct B<'a> { name: &'a str, body: &'a str }
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/context-templates/upsert");
        let resp = Self::client()?
            .post(&url)
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
        let resp = Self::client()?
            .post(&url)
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

    pub async fn delete(&self, name: &str) -> AppResult<()> {
        #[derive(Serialize)]
        struct B<'a> { name: &'a str }
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no vocab address".into()))?;
        let url = format!("{base}/v1/context-templates/delete");
        let resp = Self::client()?
            .post(&url)
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
