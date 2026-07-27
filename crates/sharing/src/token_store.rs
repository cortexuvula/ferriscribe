//! Per-client token store for the sharing pairing flow.
//!
//! Stored as a SQLCipher-encrypted SQLite file. Tokens are hashed before
//! persistence; the raw token is returned exactly once at issue time.

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

/// Errors that can occur during token store operations.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    /// Underlying SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Random number generator failure during token generation.
    // Entropy is no longer constructible after the rand 0.9 migration
    // (fill_bytes is infallible on ThreadRng). Retained for API/doc stability.
    #[allow(dead_code)]
    #[error("entropy: {0}")]
    Entropy(String),
    /// Internal mutex was poisoned (a thread panicked while holding the lock).
    #[error("lock poisoned")]
    LockPoisoned,
    /// Attempted to set a client label to an empty or whitespace-only string.
    #[error("label cannot be empty")]
    EmptyLabel,
    /// The client ID does not exist or has already been revoked.
    #[error("client not found or revoked")]
    NotFound,
}

/// Convenience alias for `Result<T, TokenStoreError>`.
pub type Result<T> = std::result::Result<T, TokenStoreError>;

/// The result of a successful [`TokenStore::issue`] call.
///
/// The raw `token` string is returned exactly once and is never persisted --
/// only its SHA-256 hash is stored. The caller must deliver it to the client
/// and cannot recover it later.
#[derive(Debug, Clone)]
pub struct IssuedToken {
    /// Row ID in the `clients` table.
    pub id: i64,
    /// The opaque bearer token (base64url-encoded 32 random bytes).
    pub token: String,
}

/// A single row from the `clients` table representing a paired device.
///
/// Returned by [`TokenStore::validate`] and [`TokenStore::list`]. Revoked
/// rows are filtered out of both queries.
#[derive(Debug, Clone)]
pub struct ClientRow {
    /// Primary key.
    pub id: i64,
    /// Human-readable label (e.g. `"clinic-laptop"`). Max 80 Unicode chars.
    pub label: String,
    /// When the token was issued.
    pub created_at: DateTime<Utc>,
    /// Last time the token was used to authenticate a proxied request.
    /// `None` if the token has never been used.
    pub last_seen_at: Option<DateTime<Utc>>,
    /// When the token was revoked. `None` for active tokens.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Thread-safe wrapper around a `rusqlite::Connection`.
///
/// `rusqlite::Connection` is `Send` but not `Sync`; the internal `Mutex`
/// makes `TokenStore: Send + Sync`, allowing it to be shared via `Arc` from
/// multi-threaded Axum state.
pub struct TokenStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for TokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenStore").finish_non_exhaustive()
    }
}

impl TokenStore {
    /// Open (or create) a SQLCipher-encrypted token store at the given path.
    ///
    /// Creates the `clients` table and unique index on `token_hash` if they
    /// don't already exist. The `key` is a 32-byte SQLCipher encryption key
    /// typically derived from the OS keychain by `medical-security`.
    ///
    /// Note: opening with the wrong key may succeed lazily -- the first
    /// query will fail instead.
    pub fn open<P: AsRef<Path>>(path: P, key: &[u8; 32]) -> Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        let key_hex = hex::encode(key);
        conn.pragma_update(None, "key", format!("x'{key_hex}'"))?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS clients (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                label TEXT NOT NULL,
                token_hash BLOB NOT NULL,
                created_at TEXT NOT NULL,
                last_seen_at TEXT,
                revoked_at TEXT
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_clients_token_hash ON clients(token_hash);
            "#,
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Issue a new bearer token for a client with the given label.
    ///
    /// Generates 32 cryptographically random bytes, base64url-encodes them
    /// (43 chars, no padding), and stores only the SHA-256 hash. The raw
    /// token is returned exactly once via [`IssuedToken::token`].
    ///
    /// # Errors
    ///
    /// Returns [`TokenStoreError::Entropy`] if the system RNG fails.
    pub fn issue(&self, label: &str) -> Result<IssuedToken> {
        let mut raw = [0u8; 32];
        rand::rng().fill_bytes(&mut raw);
        let token = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, raw);
        let hash = Sha256::digest(token.as_bytes()).to_vec();
        let now = Utc::now().to_rfc3339();
        let conn = self
            .conn
            .lock()
            .map_err(|_| TokenStoreError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO clients (label, token_hash, created_at) VALUES (?, ?, ?)",
            params![label, hash, now],
        )?;
        let id = conn.last_insert_rowid();
        Ok(IssuedToken { id, token })
    }

    /// Validate a bearer token against the store.
    ///
    /// Hashes the presented token and looks it up by hash. Returns `Some(row)`
    /// for active (non-revoked) tokens, `None` for unknown or revoked tokens.
    /// Does **not** update `last_seen_at` -- call [`touch`](Self::touch)
    /// separately after a successful validation.
    pub fn validate(&self, token: &str) -> Result<Option<ClientRow>> {
        let hash = Sha256::digest(token.as_bytes()).to_vec();
        let conn = self
            .conn
            .lock()
            .map_err(|_| TokenStoreError::LockPoisoned)?;
        let row = conn
            .query_row(
                "SELECT id, label, created_at, last_seen_at, revoked_at \
                 FROM clients WHERE token_hash = ? AND revoked_at IS NULL",
                params![hash],
                |r| {
                    Ok(ClientRow {
                        id: r.get(0)?,
                        label: r.get(1)?,
                        created_at: parse_ts(r.get::<_, String>(2)?)?,
                        last_seen_at: r.get::<_, Option<String>>(3)?.map(parse_ts).transpose()?,
                        revoked_at: r.get::<_, Option<String>>(4)?.map(parse_ts).transpose()?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Update `last_seen_at` to the current timestamp for the given client ID.
    ///
    /// Called by the auth proxy after each successfully validated request so
    /// the admin UI can show when a client was last active.
    pub fn touch(&self, id: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self
            .conn
            .lock()
            .map_err(|_| TokenStoreError::LockPoisoned)?;
        conn.execute(
            "UPDATE clients SET last_seen_at = ? WHERE id = ?",
            params![now, id],
        )?;
        Ok(())
    }

    /// Revoke a client's token by setting `revoked_at`.
    ///
    /// Idempotent: revoking an already-revoked client is a silent no-op.
    /// After revocation, the token immediately fails [`validate`](Self::validate).
    pub fn revoke(&self, id: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self
            .conn
            .lock()
            .map_err(|_| TokenStoreError::LockPoisoned)?;
        conn.execute(
            "UPDATE clients SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
            params![now, id],
        )?;
        Ok(())
    }

    /// Rename a non-revoked client. Trims whitespace, rejects empty values,
    /// silently truncates to 80 chars (Unicode-aware: counts `char`s, not
    /// bytes, so multi-byte scripts aren't sliced mid-codepoint).
    pub fn update_label(&self, id: i64, new_label: &str) -> Result<()> {
        let trimmed = new_label.trim();
        if trimmed.is_empty() {
            return Err(TokenStoreError::EmptyLabel);
        }
        let truncated: String = trimmed.chars().take(80).collect();

        let conn = self
            .conn
            .lock()
            .map_err(|_| TokenStoreError::LockPoisoned)?;
        let rows = conn.execute(
            "UPDATE clients SET label = ? WHERE id = ? AND revoked_at IS NULL",
            params![truncated, id],
        )?;
        if rows == 0 {
            return Err(TokenStoreError::NotFound);
        }
        Ok(())
    }

    /// List all non-revoked clients ordered by ID ascending.
    ///
    /// Used by the admin UI to display paired devices. Revoked rows are
    /// excluded. Each returned row has `revoked_at == None`.
    pub fn list(&self) -> Result<Vec<ClientRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| TokenStoreError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, label, created_at, last_seen_at, revoked_at \
             FROM clients WHERE revoked_at IS NULL ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ClientRow {
                    id: r.get(0)?,
                    label: r.get(1)?,
                    created_at: parse_ts(r.get::<_, String>(2)?)?,
                    last_seen_at: r.get::<_, Option<String>>(3)?.map(parse_ts).transpose()?,
                    revoked_at: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn parse_ts(s: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_store() -> (TokenStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.db");
        let key = [42u8; 32];
        let store = TokenStore::open(&path, &key).expect("open fresh store");
        (store, dir)
    }

    #[test]
    fn open_creates_fresh_database_with_no_clients() {
        let (store, _dir) = fresh_store();
        let rows = store.list().expect("list");
        assert!(rows.is_empty());
    }

    #[test]
    fn open_reopens_existing_database_with_same_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.db");
        let key = [42u8; 32];

        let token = {
            let store = TokenStore::open(&path, &key).expect("open1");
            let issued = store.issue("first-client").expect("issue");
            issued.token
        };

        // Drop the first store, reopen with the same path + key.
        let store = TokenStore::open(&path, &key).expect("open2");
        let validated = store.validate(&token).expect("validate");
        assert!(
            validated.is_some(),
            "issued token should still validate after reopen"
        );
        assert_eq!(validated.unwrap().label, "first-client");
    }

    #[test]
    fn open_with_wrong_key_rejects_database_access() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.db");
        let key_a = [42u8; 32];
        let key_b = [99u8; 32];

        {
            let store = TokenStore::open(&path, &key_a).expect("open with key_a");
            let _ = store.issue("client-a").expect("issue");
        }

        // Reopening with key_b should fail (SQLCipher key mismatch on any query).
        let reopened = TokenStore::open(&path, &key_b);
        if let Ok(store) = reopened {
            // open() may succeed lazily; the first real query must fail.
            let r = store.list();
            assert!(r.is_err(), "list() with the wrong key must fail; got {r:?}");
        }
        // Err(_) from open() is also acceptable (rejected up front).
    }

    #[test]
    fn issue_returns_id_and_opaque_token() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("alpha").expect("issue");
        assert!(issued.id > 0, "id should be positive: {}", issued.id);
        // Token is base64-url-encoded 32 random bytes → 43 chars without padding.
        assert!(
            issued.token.len() >= 32,
            "token suspiciously short: {} chars",
            issued.token.len()
        );
        assert!(
            issued
                .token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "token must be base64-url-safe: {}",
            issued.token
        );
    }

    #[test]
    fn issue_returns_different_tokens_each_call() {
        let (store, _dir) = fresh_store();
        let t1 = store.issue("a").expect("issue a").token;
        let t2 = store.issue("b").expect("issue b").token;
        let t3 = store.issue("c").expect("issue c").token;
        assert_ne!(t1, t2);
        assert_ne!(t2, t3);
        assert_ne!(t1, t3);
    }

    #[test]
    fn validate_returns_some_for_issued_token() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("clinic-laptop").expect("issue");
        let row = store
            .validate(&issued.token)
            .expect("validate")
            .expect("Some");
        assert_eq!(row.id, issued.id);
        assert_eq!(row.label, "clinic-laptop");
        assert!(row.revoked_at.is_none());
    }

    #[test]
    fn validate_returns_none_for_unknown_token() {
        let (store, _dir) = fresh_store();
        let _ = store.issue("a").expect("issue");
        let row = store.validate("not-a-real-token").expect("validate");
        assert!(row.is_none());
    }

    #[test]
    fn validate_returns_none_for_revoked_token() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("doomed").expect("issue");
        store.revoke(issued.id).expect("revoke");
        let row = store.validate(&issued.token).expect("validate");
        assert!(row.is_none(), "revoked token should not validate");
    }

    #[test]
    fn touch_updates_last_seen_at() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("touched").expect("issue");

        // Before touch: last_seen_at is None.
        let before = store
            .validate(&issued.token)
            .expect("validate before")
            .expect("Some before");
        assert!(before.last_seen_at.is_none());

        store.touch(issued.id).expect("touch");

        let after = store
            .validate(&issued.token)
            .expect("validate after")
            .expect("Some after");
        assert!(
            after.last_seen_at.is_some(),
            "touch must populate last_seen_at"
        );
    }

    #[test]
    fn revoke_is_idempotent() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("x").expect("issue");
        store.revoke(issued.id).expect("first revoke");
        // Second revoke must not error (no-op or successful idempotent UPDATE).
        store
            .revoke(issued.id)
            .expect("second revoke is idempotent");
    }

    #[test]
    fn update_label_changes_visible_label_and_rejects_empty() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("old-name").expect("issue");

        store
            .update_label(issued.id, "renamed")
            .expect("update_label happy path");
        let rows = store.list().expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "renamed");

        // Empty (and whitespace-only) labels are rejected.
        let err = store
            .update_label(issued.id, "")
            .expect_err("empty label must error");
        assert!(matches!(err, TokenStoreError::EmptyLabel));
        let err2 = store
            .update_label(issued.id, "   ")
            .expect_err("whitespace-only label must error");
        assert!(matches!(err2, TokenStoreError::EmptyLabel));

        // Updating a non-existent or revoked id returns NotFound.
        let err3 = store
            .update_label(9999, "ghost")
            .expect_err("updating non-existent id must error");
        assert!(matches!(err3, TokenStoreError::NotFound));
    }

    #[test]
    fn update_label_truncates_to_80_chars() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("orig").expect("issue");
        let long = "a".repeat(200);
        store
            .update_label(issued.id, &long)
            .expect("update with long label");
        let rows = store.list().expect("list");
        assert_eq!(
            rows[0].label.chars().count(),
            80,
            "label truncated to 80 chars"
        );
    }

    #[test]
    fn list_returns_only_non_revoked_rows_in_id_order() {
        let (store, _dir) = fresh_store();
        let a = store.issue("a").expect("a");
        let b = store.issue("b").expect("b");
        let c = store.issue("c").expect("c");
        store.revoke(b.id).expect("revoke b");

        let rows = store.list().expect("list");
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![a.id, c.id], "revoked row should be filtered out");
        // Each listed row has revoked_at == None.
        assert!(rows.iter().all(|r| r.revoked_at.is_none()));
    }
}
