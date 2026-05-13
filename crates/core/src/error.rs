use thiserror::Error;

/// Which remote service produced an `EndpointOffline` error.
/// Serialized as PascalCase strings (`"AiProvider"`, `"RemoteStt"`) so
/// the frontend can pattern-match without depending on Rust's
/// internal naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ServiceKind {
    AiProvider,
    RemoteStt,
}

/// Why a remote endpoint appears offline. Each variant corresponds to a
/// distinct user-visible dialog message in
/// `src/lib/components/EndpointOfflineDialog.svelte`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OfflineReason {
    ConnectionRefused,
    Timeout,
    DnsFailure,
    TlsFailure,
}

/// Top-level application error.
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("{provider_name} at {endpoint} is offline ({reason:?})")]
    EndpointOffline {
        service: ServiceKind,
        endpoint: String,
        reason: OfflineReason,
        provider_name: String,
    },

    #[error("Security error: {0}")]
    Security(String),

    #[error("Audio error: {0}")]
    Audio(String),

    #[error("AI provider error: {0}")]
    AiProvider(String),

    #[error("STT provider error: {0}")]
    SttProvider(String),

    #[error("TTS provider error: {0}")]
    TtsProvider(String),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("RAG error: {0}")]
    Rag(String),

    #[error("Processing error: {0}")]
    Processing(String),

    #[error("Export error: {0}")]
    Export(String),

    #[error("Translation error: {0}")]
    Translation(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// Stable machine-readable discriminant for this error. Matches the variant name.
    pub fn kind_str(&self) -> &'static str {
        match self {
            AppError::Database(_) => "Database",
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
            AppError::Io(_) => "Io",
            AppError::Serialization(_) => "Serialization",
            AppError::Cancelled => "Cancelled",
            AppError::Other(_) => "Other",
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
    fn from(s: String) -> Self {
        AppError::Other(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        AppError::Other(s.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Severity level for error logging and UI display
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ErrorSeverity {
    Critical,
    Error,
    Warning,
    Info,
}

/// Structured error context for logging and debugging
#[derive(Debug, Clone, serde::Serialize)]
pub struct ErrorContext {
    pub operation: String,
    pub error: String,
    pub severity: ErrorSeverity,
    pub error_code: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub additional_info: serde_json::Value,
}

impl ErrorContext {
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

    pub fn with_severity(mut self, severity: ErrorSeverity) -> Self {
        self.severity = severity;
        self
    }

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
        let err = AppError::Database("connection failed".into());
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
}
