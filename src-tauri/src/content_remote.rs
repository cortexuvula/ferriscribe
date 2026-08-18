//! HTTP client for the office server's `/v1/content/*` API.
//!
//! This is the client side of the content-sync feature: when a paired
//! connection is present, the content-sync Tauri commands route through here
//! instead of the local repo so the server stays the canonical source of
//! truth across machines.
//!
//! # Transport security gate
//!
//! Unlike [`crate::conditions_remote::ConditionsRemote`], which will fall
//! back to a LAN address, this client routes **exclusively over Tailscale**.
//! The [`ContentRemote::from`] constructor returns `None` when
//! `conn.tailscale` is absent — there is no LAN fallback. This is deliberate:
//! content sync carries PHI (transcripts, SOAP notes, ...) and must only
//! traverse the encrypted tailnet, never the LAN broadcast domain.
//!
//! # HIPAA note
//!
//! No transcript / SOAP / referral / letter / chat / audio content is ever
//! logged by this module. Logging is restricted to counts, IDs, and byte
//! lengths. The wire payload itself (request and response bodies) is handled
//! by `reqwest`'s JSON and byte decoders and never stringified for logs.

use std::time::Duration;

use medical_core::error::{AppError, AppResult};
use medical_core::types::endpoint::http_url;
use medical_db::content_sync::{MergeConflict, PurgedRef, SyncRecording};

use crate::commands::sharing::PairedConnection;

/// Server-side diagnostics returned by `GET /v1/content/sync/meta`.
///
/// The client uses `recording_count` and `latest_updated_at` to decide
/// whether a full re-pull is warranted (e.g. after a long offline period).
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)] // fields read by callers that opt into meta-based re-pull heuristics
pub struct ServerMeta {
    pub server_time: String,
    pub recording_count: i64,
    pub latest_updated_at: Option<String>,
}

/// Response to `GET /v1/content/sync` — one page of the delta stream.
///
/// `has_more` is `true` when another page is available; the caller advances
/// the cursor to the last record's `updated_at` and pulls again.
///
/// `purged` carries the server's purge notifications (ledger entries newer
/// than our cursor); the caller tombstones any stale local live copy.
/// `#[serde(default)]` keeps this deserializable against an older server
/// that omits the field (pre-purge binaries) — wire compatibility in both
/// directions.
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)] // server_time is part of the wire contract; read by diagnostics
pub struct PullResponse {
    pub recordings: Vec<SyncRecording>,
    pub server_time: String,
    pub has_more: bool,
    #[serde(default)]
    pub purged: Vec<PurgedRef>,
}

/// Response to `POST /v1/content/sync` — the fields where the server's local
/// copy won the last-write-wins merge (i.e. the client's push was older).
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)] // server_time is part of the wire contract
pub struct PushResponse {
    pub conflicts: Vec<MergeConflict>,
    pub server_time: String,
}

/// Build a dedicated `reqwest::Client` for a long-lived SSE stream.
///
/// Unlike the shared `state.http_client` (which carries a 30s total timeout
/// appropriate for request/response calls), this client has **no total
/// timeout** — only a connect timeout and TCP keepalive. SSE streams must not
/// be capped by a hard deadline; liveness is maintained by the server's
/// keep-alive comments and the caller's reconnect loop. See
/// [`ContentRemote::subscribe_events_async`] for the rationale.
fn sse_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .tcp_keepalive(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// HTTP client for the office server's `/v1/content/*` API.
///
/// Created via [`ContentRemote::from`] only when the paired connection
/// exposes a Tailscale address (the transport security gate). All methods
/// send bearer-authenticated requests and return `AppResult`.
///
/// Timeouts are tiered by payload sensitivity to latency:
/// * `meta` — 10s (small diagnostic payload).
/// * `pull` / `push` — 30s (batches up to 200 recordings of field metadata).
/// * audio fetch / upload — 120s (large binary blobs).
/// * SSE subscribe — long-lived; relies on per-chunk activity rather than a
///   single total timeout.
pub struct ContentRemote<'a> {
    pub conn: &'a PairedConnection,
    pub bearer: String,
    pub client: std::sync::Arc<reqwest::Client>,
}

impl<'a> ContentRemote<'a> {
    /// Create a `ContentRemote` if content sync can run securely.
    ///
    /// Returns `None` (and the caller falls back to local-only operation)
    /// when **any** of these transport-security gates fails:
    ///
    /// 1. No bearer token (unpaired or keychain miss).
    /// 2. No vocab port (the office server predates the content-sync release
    ///    or did not advertise the port).
    /// 3. **No Tailscale address** — the critical gate. Content sync must
    ///    never traverse the LAN; PHI rides on these requests.
    pub fn from(
        conn: &'a PairedConnection,
        bearer: Option<String>,
        client: std::sync::Arc<reqwest::Client>,
    ) -> Option<Self> {
        let bearer = bearer?;
        // Gate: the vocab port hosts the /v1/content/* routes.
        conn.ports.vocab?;
        // CRITICAL transport-security gate: route exclusively over Tailscale.
        // No LAN fallback for content sync — PHI must not leave the tailnet.
        conn.tailscale.as_ref()?;
        Some(Self {
            conn,
            bearer,
            client,
        })
    }

    /// Build the base URL `http://{tailscale}:{vocab}`.
    ///
    /// Returns `None` only if the Tailscale host or vocab port vanished
    /// between construction and use (shouldn't happen for an immutable
    /// `PairedConnection`, but checked defensively).
    fn base_url(&self) -> Option<String> {
        let port = self.conn.ports.vocab?;
        let host = self.conn.tailscale.as_deref()?;
        Some(http_url(host, port))
    }

    /// `GET /v1/content/sync/meta` — server diagnostics.
    ///
    /// Used to decide whether a full re-pull is warranted (e.g. the server's
    /// `latest_updated_at` is newer than our cursor by a wide margin).
    #[allow(dead_code)] // part of the client API surface; used by re-pull heuristics
    pub async fn meta(&self) -> AppResult<ServerMeta> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("content remote has no tailscale base URL".into()))?;
        let url = format!("{base}/v1/content/sync/meta");
        let resp = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(10))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("content meta: {e}")))?;
        check_status(&resp).await?;
        resp.json::<ServerMeta>()
            .await
            .map_err(|e| AppError::Other(format!("content meta parse: {e}")))
    }

    /// `GET /v1/content/sync?since={cursor}&limit=200` — incremental delta pull.
    ///
    /// Pass `since = None` for the initial full pull. The response is ordered
    /// by `updated_at` ascending; advance the cursor to the last record's
    /// `updated_at` and pull again while `has_more` is true.
    pub async fn pull(&self, since: Option<&str>) -> AppResult<PullResponse> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("content remote has no tailscale base URL".into()))?;
        let url = match since {
            Some(cursor) => format!("{base}/v1/content/sync?since={cursor}&limit=200"),
            None => format!("{base}/v1/content/sync?limit=200"),
        };
        let resp = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(30))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("content pull: {e}")))?;
        check_status(&resp).await?;
        resp.json::<PullResponse>()
            .await
            .map_err(|e| AppError::Other(format!("content pull parse: {e}")))
    }

    /// `POST /v1/content/sync` — push local recordings (two-way merge).
    ///
    /// The server applies per-field last-write-wins merge against its own
    /// rows and returns the fields where the server's copy won. After a
    /// successful merge the server broadcasts on its content-changed SSE
    /// channel so other clients refresh.
    pub async fn push(&self, recordings: Vec<SyncRecording>) -> AppResult<PushResponse> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("content remote has no tailscale base URL".into()))?;
        let url = format!("{base}/v1/content/sync");
        // Log the count only — never the payload (PHI).
        let count = recordings.len();
        // Wrap in an object: the server expects { "recordings": [...] }, not a
        // bare array. Sending a bare array caused HTTP 422 (deserialization
        // failure) on every push — the client's recordings never reached the
        // server.
        let body = serde_json::json!({ "recordings": recordings });
        let resp = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(30))
            .bearer_auth(&self.bearer)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("content push: {e}")))?;
        check_status(&resp).await?;
        let parsed = resp
            .json::<PushResponse>()
            .await
            .map_err(|e| AppError::Other(format!("content push parse: {e}")))?;
        tracing::debug!(
            pushed_count = count,
            conflict_count = parsed.conflicts.len(),
            "content push ok"
        );
        Ok(parsed)
    }

    /// `GET /v1/content/audio/{id}` — download decrypted audio bytes.
    ///
    /// Returns the raw plaintext audio bytes. The caller re-encrypts them
    /// locally before writing to disk; this function never persists plaintext.
    pub async fn fetch_audio(&self, recording_id: &str) -> AppResult<Vec<u8>> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("content remote has no tailscale base URL".into()))?;
        let url = format!("{base}/v1/content/audio/{recording_id}");
        let resp = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(120))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("content audio fetch: {e}")))?;
        check_status(&resp).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AppError::Other(format!("content audio fetch body: {e}")))?;
        let byte_count = bytes.len();
        tracing::debug!(
            recording_id_len = recording_id.len(),
            byte_count,
            "content audio fetched"
        );
        Ok(bytes.to_vec())
    }

    /// `PUT /v1/content/audio/{id}` — upload plaintext audio bytes
    /// (first-write-wins).
    ///
    /// The body is the raw plaintext audio. The server encrypts it at rest.
    /// A `409 Conflict` response (the file already exists on the server) is
    /// treated as success — the first writer wins and there is nothing more
    /// for us to do.
    pub async fn upload_audio(&self, recording_id: &str, data: Vec<u8>) -> AppResult<()> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("content remote has no tailscale base URL".into()))?;
        let url = format!("{base}/v1/content/audio/{recording_id}");
        let byte_count = data.len();
        let resp = self
            .client
            .put(&url)
            .timeout(Duration::from_secs(120))
            .bearer_auth(&self.bearer)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(data)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("content audio upload: {e}")))?;

        let status = resp.status();
        // First-write-wins: a 409 means the server already has this audio;
        // treat as success.
        if status == reqwest::StatusCode::CONFLICT {
            tracing::debug!(
                recording_id_len = recording_id.len(),
                byte_count,
                "content audio upload: server already has file (409 treated as success)"
            );
            return Ok(());
        }
        if status.is_success() {
            tracing::debug!(
                recording_id_len = recording_id.len(),
                byte_count,
                "content audio upload ok"
            );
            return Ok(());
        }
        Err(map_status_error(status, "content audio upload"))
    }

    /// `GET /v1/content/events` — open the SSE change-notification stream.
    ///
    /// Returns the raw `reqwest::Response`; the caller reads the body as a
    /// byte stream and parses `data: changed` lines.
    ///
    /// **No total timeout is set on the SSE request.** SSE is a long-lived
    /// stream; reqwest's `.timeout()` is a *hard total deadline from request
    /// start* (it does NOT reset on stream chunks, despite a common
    /// misconception). Capping it at 300s — as this code did before — forced
    /// a reconnect storm every 5 minutes (311 warnings/day in user logs).
    /// Liveness is instead maintained by the server's SSE `keep_alive`
    /// comments (every 15s, preventing NAT/relay idle closes) and the caller's
    /// reconnect loop with backoff (which handles genuine disconnects).
    ///
    /// A dedicated client is built here (rather than reusing `self.client`)
    /// because the shared client carries a 30s total timeout that would cap
    /// the stream even without an explicit per-request timeout.
    ///
    /// Unlike [`ConditionsRemote::subscribe_events`](crate::conditions_remote::ConditionsRemote::subscribe_events),
    /// this returns the raw response rather than a parsed stream so the
    /// caller owns the reconnect/backoff loop (it also needs to re-evaluate
    /// the sync target on each reconnect in case pairing changed).
    pub async fn subscribe_events_async(&self) -> AppResult<reqwest::Response> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("content remote has no tailscale base URL".into()))?;
        let url = format!("{base}/v1/content/events");
        let resp = sse_client()
            .get(&url)
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("content SSE connect: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(map_status_error(status, "content SSE connect"));
        }
        Ok(resp)
    }
}

/// Map a non-success HTTP status to a user-actionable `AppError`.
///
/// Shared by all methods. Always returns an error; the `ctx` string is folded
/// into the message so logs identify which call failed.
async fn check_status(resp: &reqwest::Response) -> AppResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    Err(map_status_error(status, "content API"))
}

fn map_status_error(status: reqwest::StatusCode, ctx: &str) -> AppError {
    if status == reqwest::StatusCode::NOT_FOUND {
        return AppError::Other(format!(
            "{ctx}: office server does not support content sync (update it to a later release)"
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return AppError::Other(format!(
            "{ctx}: office server rejected the bearer token (try unpair → re-pair)"
        ));
    }
    AppError::Other(format!("{ctx}: HTTP {status}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An older server (pre-purge release) omits `purged` entirely; the
    /// client must deserialize it as an empty vec, not error — wire
    /// compatibility for the new-client → old-server direction.
    #[test]
    fn pull_response_defaults_missing_purged_to_empty() {
        let legacy = serde_json::json!({
            "recordings": [],
            "server_time": "2026-08-17T00:00:00+00:00",
            "has_more": false
        });
        let resp: PullResponse = serde_json::from_value(legacy).expect("legacy payload parses");
        assert!(resp.recordings.is_empty());
        assert!(resp.purged.is_empty());
    }

    /// A newer server includes `purged`; the ids and timestamps round-trip.
    #[test]
    fn pull_response_parses_purged_refs() {
        let payload = serde_json::json!({
            "recordings": [],
            "server_time": "2026-08-17T00:00:00+00:00",
            "has_more": false,
            "purged": [
                { "id": "11111111-1111-1111-1111-111111111111", "purged_at": "2026-08-16T00:00:00+00:00" }
            ]
        });
        let resp: PullResponse = serde_json::from_value(payload).expect("payload parses");
        assert_eq!(
            resp.purged,
            vec![PurgedRef {
                id: "11111111-1111-1111-1111-111111111111".to_string(),
                purged_at: "2026-08-16T00:00:00+00:00".to_string(),
            }]
        );
    }
}
