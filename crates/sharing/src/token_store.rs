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

#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("entropy: {0}")]
    Entropy(String),
    #[error("lock poisoned")]
    LockPoisoned,
    #[error("label cannot be empty")]
    EmptyLabel,
    #[error("client not found or revoked")]
    NotFound,
}

pub type Result<T> = std::result::Result<T, TokenStoreError>;

#[derive(Debug, Clone)]
pub struct IssuedToken {
    pub id: i64,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct ClientRow {
    pub id: i64,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
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
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn issue(&self, label: &str) -> Result<IssuedToken> {
        let mut raw = [0u8; 32];
        rand::thread_rng()
            .try_fill_bytes(&mut raw)
            .map_err(|e| TokenStoreError::Entropy(e.to_string()))?;
        let token = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            raw,
        );
        let hash = Sha256::digest(token.as_bytes()).to_vec();
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| TokenStoreError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO clients (label, token_hash, created_at) VALUES (?, ?, ?)",
            params![label, hash, now],
        )?;
        let id = conn.last_insert_rowid();
        Ok(IssuedToken { id, token })
    }

    pub fn validate(&self, token: &str) -> Result<Option<ClientRow>> {
        let hash = Sha256::digest(token.as_bytes()).to_vec();
        let conn = self.conn.lock().map_err(|_| TokenStoreError::LockPoisoned)?;
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
                        last_seen_at: r
                            .get::<_, Option<String>>(3)?
                            .map(parse_ts)
                            .transpose()?,
                        revoked_at: r
                            .get::<_, Option<String>>(4)?
                            .map(parse_ts)
                            .transpose()?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn touch(&self, id: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| TokenStoreError::LockPoisoned)?;
        conn.execute(
            "UPDATE clients SET last_seen_at = ? WHERE id = ?",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn revoke(&self, id: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| TokenStoreError::LockPoisoned)?;
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

        let conn = self.conn.lock().map_err(|_| TokenStoreError::LockPoisoned)?;
        let rows = conn.execute(
            "UPDATE clients SET label = ? WHERE id = ? AND revoked_at IS NULL",
            params![truncated, id],
        )?;
        if rows == 0 {
            return Err(TokenStoreError::NotFound);
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<ClientRow>> {
        let conn = self.conn.lock().map_err(|_| TokenStoreError::LockPoisoned)?;
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
                    last_seen_at: r
                        .get::<_, Option<String>>(3)?
                        .map(parse_ts)
                        .transpose()?,
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
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
}
