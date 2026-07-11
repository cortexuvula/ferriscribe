use thiserror::Error;

/// Which remote service produced an [`AppError::EndpointOffline`] error.
///
/// Serialized as PascalCase strings (`"AiProvider"`, `"RemoteStt"`) so the
/// Svelte frontend can pattern-match on the value without depending on
/// Rust-internal naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ServiceKind {
    /// An AI completion provider (Ollama or LM Studio).
    AiProvider,
    /// A remote speech-to-text endpoint.
    RemoteStt,
}

/// Why a remote endpoint appears offline.
///
/// Each variant corresponds to a distinct user-visible dialog message in
/// `src/lib/components/EndpointOfflineDialog.svelte`. The preflight module
/// ([`crate::preflight::classify_reqwest_error`]) maps `reqwest::Error`
/// source chains into one of these discriminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OfflineReason {
    /// The remote host actively refused the TCP connection.
    ConnectionRefused,
    /// The probe exceeded the configured timeout (currently 3 s).
    Timeout,
    /// DNS resolution failed for the endpoint hostname.
    DnsFailure,
    /// TLS / certificate handshake failed.
    TlsFailure,
}

/// Top-level application error propagated across all crate boundaries.
///
/// Every fallible function in the workspace returns
/// [`AppResult<T>`](AppResult), which is `Result<T, AppError>`. Variants
/// cover each subsystem so the Tauri command layer and frontend can
/// distinguish error origins without string parsing.
///
/// # Serialization
///
/// A custom [`serde::Serialize`] impl produces a JSON object with at least
/// `kind` (the variant name as a stable string) and `message` (the
/// `Display` output). Structured variants like
/// [`EndpointOffline`](AppError::EndpointOffline) and
/// [`InvalidEndpoint`](AppError::InvalidEndpoint) add extra fields so the
/// frontend can render targeted UI (e.g. an offline dialog with the
/// provider name).
///
/// # Variant selection guidance
///
/// | Use case | Variant |
/// |---|---|
/// | SQL / migration failure | [`Database`](AppError::Database) |
/// | Endpoint unreachable (preflight or runtime) | [`EndpointOffline`](AppError::EndpointOffline) |
/// | PHI/auth boundary violation | [`Security`](AppError::Security) |
/// | Audio capture or playback failure | [`Audio`](AppError::Audio) |
/// | Completion API error from AI provider | [`AiProvider`](AppError::AiProvider) |
/// | Transcription failure | [`SttProvider`](AppError::SttProvider) |
/// | TTS synthesis failure | [`TtsProvider`](AppError::TtsProvider) |
/// | Agent orchestration failure | [`Agent`](AppError::Agent) |
/// | RAG retrieval / indexing failure | [`Rag`](AppError::Rag) |
/// | Queue / batch processing failure | [`Processing`](AppError::Processing) |
/// | PDF / DOCX / FHIR export failure | [`Export`](AppError::Export) |
/// | Translation failure | [`Translation`](AppError::Translation) |
/// | Settings validation / migration | [`Config`](AppError::Config) |
/// | Endpoint policy violation | [`InvalidEndpoint`](AppError::InvalidEndpoint) |
/// | `std::io::Error` conversion | [`Io`](AppError::Io) |
/// | `serde_json::Error` conversion | [`Serialization`](AppError::Serialization) |
/// | User-initiated cancellation | [`Cancelled`](AppError::Cancelled) |
/// | Poisoned `Mutex` / `RwLock` | [`MutexPoisoned`](AppError::MutexPoisoned) |
/// | reqwest client construction failure | [`HttpClient`](AppError::HttpClient) |
/// | Anything that doesn't fit above | [`Other`](AppError::Other) |
#[derive(Error, Debug)]
pub enum AppError {
    /// A database operation failed (SQL error, migration, connection pool).
    #[error("Database error: {message}")]
    Database {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// A remote endpoint was unreachable during preflight or at call time.
    #[error("{provider_name} at {endpoint} is offline ({reason:?})")]
    EndpointOffline {
        /// Which service category the endpoint belongs to.
        service: ServiceKind,
        /// The base URL that was probed.
        endpoint: String,
        /// The classified reason for the failure.
        reason: OfflineReason,
        /// Human-readable provider name for UI display.
        provider_name: String,
    },

    /// A security boundary was violated (PHI leak attempt, auth failure).
    #[error("Security error: {0}")]
    Security(String),

    /// Audio capture or playback failed.
    #[error("Audio error: {0}")]
    Audio(String),

    /// An AI completion provider returned an error.
    #[error("AI provider error: {0}")]
    AiProvider(String),

    /// A speech-to-text provider returned an error.
    #[error("STT provider error: {0}")]
    SttProvider(String),

    /// A text-to-speech provider returned an error.
    #[error("TTS provider error: {0}")]
    TtsProvider(String),

    /// An agent orchestration step failed.
    #[error("Agent error: {0}")]
    Agent(String),

    /// A RAG retrieval or indexing operation failed.
    #[error("RAG error: {0}")]
    Rag(String),

    /// A background processing task (queue / batch) failed.
    #[error("Processing error: {0}")]
    Processing(String),

    /// A document export operation failed.
    #[error("Export error: {0}")]
    Export(String),

    /// A translation operation failed.
    #[error("Translation error: {0}")]
    Translation(String),

    /// Configuration is invalid or could not be migrated.
    #[error("Configuration error: {0}")]
    Config(String),

    /// An endpoint URL was rejected by the local-only endpoint policy.
    #[error(
        "invalid endpoint '{host}' for {field}: public/unknown endpoints are blocked (kind={kind:?}). Enable 'Allow public endpoints' in Advanced settings to override."
    )]
    InvalidEndpoint {
        /// The settings field being validated (e.g. `"ollama_host"`).
        field: String,
        /// The host string that was rejected.
        host: String,
        /// How the host was classified by the endpoint policy.
        kind: crate::endpoint_policy::EndpointKind,
    },

    /// An I/O error (automatic conversion from [`std::io::Error`]).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON serialization / deserialization error (automatic conversion
    /// from [`serde_json::Error`]).
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// The operation was cancelled by the user (e.g. aborting transcription).
    #[error("Cancelled")]
    Cancelled,

    /// A `Mutex` or `RwLock` was poisoned by a panicking thread.
    #[error("Mutex poisoned: {0}")]
    MutexPoisoned(String),

    /// Building an HTTP client (e.g. via `reqwest::ClientBuilder`) failed.
    #[error("HTTP client error: {0}")]
    HttpClient(String),

    /// Catch-all for errors that don't fit any other variant.
    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// Stable machine-readable discriminant for this error variant.
    ///
    /// Returns the variant name as a `&'static str` (e.g. `"Database"`,
    /// `"EndpointOffline"`). Used by the custom `Serialize` impl and by
    /// the Tauri command layer to produce consistent error `kind` fields
    /// for the frontend.
    pub fn kind_str(&self) -> &'static str {
        match self {
            AppError::Database { .. } => "Database",
            AppError::EndpointOffline { .. } => "EndpointOffline",
            AppError::Security(_) => "Security",
            AppError::Audio(_) => "Audio",
            AppError::AiProvider(_) => "AiProvider",
            AppError::SttProvider(_) => "SttProvider",
            AppError::TtsProvider(_) => "TtsProvider",
            AppError::Agent(_) => "Agent",
            AppError::Rag(_) => "Rag",
            AppError::Processing(_) => "Processing",
            AppError::Export(_) => "Export",
            AppError::Translation(_) => "Translation",
            AppError::Config(_) => "Config",
            AppError::InvalidEndpoint { .. } => "InvalidEndpoint",
            AppError::Io(_) => "Io",
            AppError::Serialization(_) => "Serialization",
            AppError::Cancelled => "Cancelled",
            AppError::MutexPoisoned(_) => "MutexPoisoned",
            AppError::HttpClient(_) => "HttpClient",
            AppError::Other(_) => "Other",
        }
    }

    /// Create a Database error with a source error preserved for
    /// `Error::source()` chain inspection.
    pub fn database_with_source(
        message: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        AppError::Database {
            message: message.into(),
            source: Some(source.into()),
        }
    }

    /// Create a Database error from a plain message (no source preserved).
    pub fn database(message: impl Into<String>) -> Self {
        AppError::Database {
            message: message.into(),
            source: None,
        }
    }
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        match self {
            AppError::EndpointOffline {
                service,
                endpoint,
                reason,
                provider_name,
            } => {
                let mut s = serializer.serialize_struct("AppError", 6)?;
                s.serialize_field("kind", self.kind_str())?;
                s.serialize_field("message", &self.to_string())?;
                s.serialize_field("service", service)?;
                s.serialize_field("endpoint", endpoint)?;
                s.serialize_field("reason", reason)?;
                s.serialize_field("provider_name", provider_name)?;
                s.end()
            }
            AppError::InvalidEndpoint { field, host, kind } => {
                let mut s = serializer.serialize_struct("AppError", 5)?;
                s.serialize_field("kind", self.kind_str())?;
                s.serialize_field("message", &self.to_string())?;
                s.serialize_field("field", field)?;
                s.serialize_field("host", host)?;
                s.serialize_field("endpointKind", kind)?;
                s.end()
            }
            _ => {
                let mut s = serializer.serialize_struct("AppError", 2)?;
                s.serialize_field("kind", self.kind_str())?;
                s.serialize_field("message", &self.to_string())?;
                s.end()
            }
        }
    }
}

impl From<String> for AppError {
    /// Converts a `String` into [`AppError::Other`].
    fn from(s: String) -> Self {
        AppError::Other(s)
    }
}

impl From<&str> for AppError {
    /// Converts a `&str` into [`AppError::Other`].
    fn from(s: &str) -> Self {
        AppError::Other(s.to_string())
    }
}

impl AppError {
    /// Convert an [`EndpointPolicyError`](crate::endpoint_policy::EndpointPolicyError)
    /// into an [`AppError::InvalidEndpoint`] by attaching the settings field
    /// name the caller was validating.
    ///
    /// This is the recommended way to surface endpoint-policy rejections
    /// from Tauri commands — it produces a structured error with `field`,
    /// `host`, and `endpointKind` that the frontend renders into a
    /// user-actionable message.
    pub fn invalid_endpoint_for(
        err: crate::endpoint_policy::EndpointPolicyError,
        field: impl Into<String>,
    ) -> Self {
        let crate::endpoint_policy::EndpointPolicyError::Blocked { host, kind } = err;
        AppError::InvalidEndpoint {
            field: field.into(),
            host,
            kind,
        }
    }
}

/// Convenience alias: `Result<T, AppError>`.
///
/// Every fallible function in the workspace returns this type.
pub type AppResult<T> = Result<T, AppError>;

/// Severity level for error logging and UI display.
///
/// Used inside [`ErrorContext`] to classify how urgently an error needs
/// attention. The frontend maps these to toast styling and log filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ErrorSeverity {
    /// The application cannot continue — data loss or crash.
    Critical,
    /// A user-visible operation failed but the app remains functional.
    Error,
    /// Something unexpected happened but the operation succeeded.
    Warning,
    /// Informational — no action required.
    Info,
}

/// Structured error context for logging and debugging.
///
/// Builder pattern: start with [`ErrorContext::new`], then chain
/// [`with_severity`](ErrorContext::with_severity) and
/// [`with_code`](ErrorContext::with_code). Serializes to JSON for
/// structured logging and frontend consumption.
///
/// # Example
///
/// ```ignore
/// let ctx = ErrorContext::new("save_recording", "disk full")
///     .with_severity(ErrorSeverity::Critical)
///     .with_code("DISK_FULL");
/// ```
#[derive(Debug, Clone, serde::Serialize)]
pub struct ErrorContext {
    /// The operation that was being attempted (e.g. `"save_recording"`).
    pub operation: String,
    /// Human-readable error description.
    pub error: String,
    /// How severe this error is.
    pub severity: ErrorSeverity,
    /// Optional machine-readable error code (e.g. `"DISK_FULL"`).
    pub error_code: Option<String>,
    /// UTC timestamp when the error was created.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Arbitrary extra context (defaults to `null`).
    pub additional_info: serde_json::Value,
}

impl ErrorContext {
    /// Create a new context with [`ErrorSeverity::Error`] and the current
    /// UTC timestamp. Chain [`with_severity`](Self::with_severity) and
    /// [`with_code`](Self::with_code) to customize.
    pub fn new(operation: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            error: error.into(),
            severity: ErrorSeverity::Error,
            error_code: None,
            timestamp: chrono::Utc::now(),
            additional_info: serde_json::Value::Null,
        }
    }

    /// Override the default severity level.
    pub fn with_severity(mut self, severity: ErrorSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Attach a machine-readable error code.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.error_code = Some(code.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_error_display_formats_correctly() {
        let err = AppError::database("connection failed");
        assert_eq!(err.to_string(), "Database error: connection failed");
    }

    #[test]
    fn app_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let app_err: AppError = io_err.into();
        assert!(matches!(app_err, AppError::Io(_)));
        assert!(app_err.to_string().contains("file missing"));
    }

    #[test]
    fn database_with_source_preserves_chain() {
        let io_err = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "truncated");
        let app_err = AppError::database_with_source("DB read failed", io_err);
        assert_eq!(app_err.kind_str(), "Database");
        assert!(app_err.to_string().contains("DB read failed"));
        // The source chain must be preserved for structured error inspection.
        let source = std::error::Error::source(&app_err);
        assert!(source.is_some(), "source should be preserved");
        assert!(
            source.unwrap().to_string().contains("truncated"),
            "source should contain the original error text"
        );
    }

    #[test]
    fn database_without_source_has_none_source() {
        let app_err = AppError::database("simple error");
        let source = std::error::Error::source(&app_err);
        assert!(source.is_none(), "source should be None when not provided");
    }

    #[test]
    fn error_context_builder() {
        let ctx = ErrorContext::new("save_recording", "disk full")
            .with_severity(ErrorSeverity::Critical)
            .with_code("DISK_FULL");
        assert_eq!(ctx.operation, "save_recording");
        assert_eq!(ctx.severity, ErrorSeverity::Critical);
        assert_eq!(ctx.error_code.as_deref(), Some("DISK_FULL"));
    }

    #[test]
    fn error_context_serializes_to_json() {
        let ctx = ErrorContext::new("test_op", "test_err");
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["operation"], "test_op");
        assert_eq!(json["error"], "test_err");
        assert!(json["timestamp"].is_string());
    }

    #[test]
    fn app_error_serializes_with_kind_and_message() {
        let err = AppError::AiProvider("bad API key".into());
        let json = serde_json::to_value(&err).expect("serialize");
        assert_eq!(json["kind"], "AiProvider");
        assert_eq!(json["message"], "AI provider error: bad API key");
    }

    #[test]
    fn app_error_io_serializes_with_io_kind() {
        let err: AppError = std::io::Error::new(std::io::ErrorKind::NotFound, "x").into();
        let json = serde_json::to_value(&err).expect("serialize");
        assert_eq!(json["kind"], "Io");
        assert!(
            json["message"].as_str().unwrap().contains("x"),
            "message must contain the underlying error text"
        );
    }

    #[test]
    fn app_error_cancelled_serializes() {
        let err = AppError::Cancelled;
        let json = serde_json::to_value(&err).expect("serialize");
        assert_eq!(json["kind"], "Cancelled");
        assert_eq!(json["message"], "Cancelled");
    }

    #[test]
    fn endpoint_offline_serializes_with_structured_fields() {
        let err = AppError::EndpointOffline {
            service: ServiceKind::AiProvider,
            endpoint: "http://192.168.1.10:11434".into(),
            reason: OfflineReason::ConnectionRefused,
            provider_name: "Ollama".into(),
        };
        let json = serde_json::to_value(&err).expect("serialize");
        assert_eq!(json["kind"], "EndpointOffline");
        assert_eq!(json["service"], "AiProvider");
        assert_eq!(json["endpoint"], "http://192.168.1.10:11434");
        assert_eq!(json["reason"], "ConnectionRefused");
        assert_eq!(json["provider_name"], "Ollama");
        assert!(
            json["message"].as_str().unwrap().contains("Ollama"),
            "message should contain provider_name for log readability"
        );
    }

    #[test]
    fn endpoint_offline_kind_str_is_stable() {
        let err = AppError::EndpointOffline {
            service: ServiceKind::RemoteStt,
            endpoint: "http://x:1".into(),
            reason: OfflineReason::Timeout,
            provider_name: "Whisper STT".into(),
        };
        assert_eq!(err.kind_str(), "EndpointOffline");
    }

    #[test]
    fn service_kind_serializes_as_pascalcase() {
        let json = serde_json::to_value(ServiceKind::RemoteStt).unwrap();
        assert_eq!(json, serde_json::json!("RemoteStt"));
    }

    #[test]
    fn offline_reason_serializes_as_pascalcase() {
        let json = serde_json::to_value(OfflineReason::DnsFailure).unwrap();
        assert_eq!(json, serde_json::json!("DnsFailure"));
    }

    #[test]
    fn mutex_poisoned_variant_includes_context() {
        let err = AppError::MutexPoisoned("capture_handle: poisoned lock".to_string());
        let msg = err.to_string();
        assert!(
            msg.contains("capture_handle"),
            "message should include the lock name for debugging, got: {msg}"
        );
        assert!(
            msg.contains("poisoned lock"),
            "message should describe the failure, got: {msg}"
        );
        assert_eq!(err.kind_str(), "MutexPoisoned");
    }

    #[test]
    fn http_client_variant_includes_context() {
        let err = AppError::HttpClient("failed to build client: TLS error".to_string());
        let msg = err.to_string();
        assert!(
            msg.contains("failed to build client"),
            "message should include the construction failure, got: {msg}"
        );
        assert!(
            msg.contains("TLS error"),
            "message should include the underlying cause, got: {msg}"
        );
        assert_eq!(err.kind_str(), "HttpClient");
    }
}
