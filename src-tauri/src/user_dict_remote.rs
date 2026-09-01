//! HTTP client for the office server's user-dictionary CRUD + sync API.
//!
//! Mirrors `vocab_remote.rs` and `conditions_remote.rs`: when a paired
//! connection is present and the office server advertised a `vocab_port`,
//! the user-dictionary Tauri commands route through here instead of the
//! local SQLite repo so the server stays the canonical source of truth.
//!
//! In addition to the original list/add/remove CRUD, this client exposes a
//! `sync` (two-way merge, legacy word-only response) method, a
//! [`UserDictRemote::sync_full`] (full-fidelity merge whose response carries
//! tombstones, with a legacy fallback), and a `subscribe_events` (SSE)
//! method that mirror `conditions_remote`.

use std::time::Duration;

use futures_util::StreamExt;
use medical_core::error::{AppError, AppResult};
use medical_core::types::endpoint::http_url;
use medical_core::types::user_dict_entry::{UserDictEntry, deterministic_id};
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

    /// Full-fidelity sync (entries incl. tombstones) so deletions propagate.
    ///
    /// Pushes the local full list (active + tombstones) and receives the
    /// server's post-merge FULL entry list, which the caller merges back
    /// into the local store — a deletion on the server (or on another
    /// client) then converges here too, mirroring the condition-chips path.
    ///
    /// Falls back to the legacy word-only [`Self::sync`] when the server
    /// predates the endpoint: a 404 (route unknown) or a 405 (the path
    /// falls through to the DELETE-only `/v1/user-dictionary/{word}` route,
    /// so an older axum server answers POST with Method Not Allowed).
    /// Deletions still propagate TO the server in that mode (the legacy
    /// handler accepts full entries); only the response loses tombstones.
    /// Legacy words are converted to entries with the repo's deterministic
    /// id derivation (see [`legacy_word_to_entry`]) so the local merge has a
    /// uniform shape. Synthesized entries are active with `updated_at`
    /// captured BEFORE the legacy request (see the fallback comment).
    ///
    /// Returns the entry list plus a `legacy` flag (true when the fallback
    /// path ran): legacy responses cannot carry tombstones, so callers
    /// should display the server's active words rather than the local list
    /// (deletions made elsewhere are otherwise invisible in the UI until
    /// the server is upgraded).
    pub async fn sync_full(
        &self,
        local_entries: Vec<UserDictEntry>,
    ) -> AppResult<(Vec<UserDictEntry>, bool)> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no dictionary address".into()))?;
        let url = format!("{base}/v1/user-dictionary/sync-full");
        let resp = self
            .client
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(&local_entries)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("dict sync-full: {e}")))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
        {
            // Server predates /sync-full — degrade to the legacy word-only
            // sync and synthesize entries from the returned words.
            //
            // The synthesis timestamp is captured BEFORE the legacy request:
            // a deletion landing on the server while the response is in
            // transit must not be outrun by a fresher synthesized stamp
            // (that would resurrect the word on the next push). Stamping at
            // request time guarantees any such deletion is strictly newer
            // and wins; exact ties are covered by the merge's
            // tombstone-wins tie-break.
            let now = chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string();
            let words = self.sync(local_entries).await?;
            return Ok((
                words
                    .iter()
                    // Match `UserDictionaryRepo::add`, which skips empty input.
                    .filter(|w| !w.trim().is_empty())
                    .map(|w| legacy_word_to_entry(w, &now))
                    .collect(),
                true,
            ));
        }
        check_status(&resp).await?;
        let entries = resp
            .json::<Vec<UserDictEntry>>()
            .await
            .map_err(|e| AppError::Other(format!("dict sync-full parse: {e}")))?;
        Ok((entries, false))
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

/// Convert one legacy (word-only) sync response word into a full entry.
///
/// Uses the exact id + normalization derivation `UserDictionaryRepo::add`
/// applies — trim, then [`deterministic_id`] (UUID v5 of the lowercased
/// trimmed word), keeping the trimmed word's case in `word`. A mismatched
/// synthetic id would make the local merge see two distinct entries for
/// one word and duplicate it; the equality is pinned by
/// `legacy_word_to_entry_matches_repo_add` below.
fn legacy_word_to_entry(word: &str, now_iso: &str) -> UserDictEntry {
    let trimmed = word.trim();
    UserDictEntry {
        id: deterministic_id(trimmed),
        word: trimmed.to_string(),
        updated_at: now_iso.to_string(),
        deleted_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The legacy-fallback word → entry synthesis must produce the SAME id
    /// and word value that `UserDictionaryRepo::add` wrote for that word —
    /// a mismatched synthetic id would make the local merge treat one word
    /// as two entries and duplicate it. Round-trips several words (mixed
    /// case, padding) through the repo's real write path and compares
    /// against the stored rows.
    #[test]
    fn legacy_word_to_entry_matches_repo_add() {
        let db = medical_db::Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");
        let stored_at = "2026-08-17T00:00:00.000Z";

        for raw in ["Lisinopril", "  atenolol  ", "METFORMIN", "hctz"] {
            // The real write path (trims + derives the id internally).
            medical_db::user_dictionary::UserDictionaryRepo::add(&conn, raw, stored_at)
                .expect("add via repo");

            // What the legacy server echoes back: the stored (trimmed) word.
            let stored = medical_db::user_dictionary::UserDictionaryRepo::list_all(&conn)
                .expect("list_all")
                .into_iter()
                .find(|e| e.word == raw.trim())
                .expect("stored entry");

            // Synthesis at a LATER timestamp — only id and word must match;
            // updated_at is deliberately "now" for the merge clock.
            let synthesized = legacy_word_to_entry(&stored.word, "2026-08-17T12:00:00.000Z");
            assert_eq!(
                synthesized.id, stored.id,
                "synthesized id must equal the repo-derived id"
            );
            assert_eq!(
                synthesized.word, stored.word,
                "synthesized word must equal the repo-stored word"
            );
            assert!(
                synthesized.deleted_at.is_none(),
                "legacy words are active by construction"
            );
        }
    }
}
