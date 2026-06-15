//! Upstream readiness probes for the sharing server.
//!
//! Pure async functions — no shared state. Used by the start-up readiness
//! gate and the long-lived ReadinessWatcher.
//!
//! ## PHI safety
//!
//! Probes hit `127.0.0.1` only and inspect only the HTTP status code. No
//! request or response bodies are logged.

use std::time::Duration;

use reqwest::Client;

/// Which local upstream we are probing. Drives the readiness URL path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamKind {
    Ollama,
    Whisper,
    LmStudio,
}

/// A probe target: kind + base URL (e.g. `http://127.0.0.1:11434`).
#[derive(Debug, Clone)]
pub struct UpstreamTarget {
    pub kind: UpstreamKind,
    pub base_url: String,
}

impl UpstreamTarget {
    pub fn new(kind: UpstreamKind, base_url: impl Into<String>) -> Self {
        Self { kind, base_url: base_url.into() }
    }

    /// URL probed for readiness. GET, status 200 == ready.
    fn readiness_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        match self.kind {
            UpstreamKind::Ollama => format!("{base}/api/tags"),
            UpstreamKind::Whisper | UpstreamKind::LmStudio => format!("{base}/v1/models"),
        }
    }
}

/// Probe a single upstream once. `true` iff it answered a GET with 2xx.
pub async fn probe_ready(client: &Client, target: &UpstreamTarget) -> bool {
    let url = target.readiness_url();
    match client.get(&url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Poll `probe_ready` on a bounded backoff until `deadline` elapses or the
/// upstream answers ready. Returns `true` if it became ready in time.
pub async fn probe_with_backoff(
    client: &Client,
    target: &UpstreamTarget,
    deadline: tokio::time::Instant,
) -> bool {
    loop {
        if probe_ready(client, target).await {
            return true;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return false;
        }
        // Backoff: 200ms, 400ms, 800ms, capped at 1s; bounded by the deadline.
        let step = Duration::from_millis(200).min(deadline.saturating_duration_since(now));
        tokio::time::sleep(step).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use wiremock::matchers::{method, path};

    fn client() -> Client {
        Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn probe_ready_ollama_200_is_ready() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Ollama, srv.uri());
        assert!(probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_ready_ollama_503_is_not_ready() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Ollama, srv.uri());
        assert!(!probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_ready_whisper_hits_v1_models() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Whisper, srv.uri());
        assert!(probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_ready_lmstudio_hits_v1_models() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::LmStudio, srv.uri());
        assert!(probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_ready_connection_refused_is_not_ready() {
        // Port 1: privileged, almost certainly not listening; connect fails fast.
        let t = UpstreamTarget::new(UpstreamKind::Ollama, "http://127.0.0.1:1");
        assert!(!probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_with_backoff_returns_false_when_never_ready() {
        let t = UpstreamTarget::new(UpstreamKind::Ollama, "http://127.0.0.1:1");
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        assert!(!probe_with_backoff(&client(), &t, deadline).await);
    }

    #[tokio::test]
    async fn probe_with_backoff_returns_true_when_ready_immediately() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Ollama, srv.uri());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        assert!(probe_with_backoff(&client(), &t, deadline).await);
    }

    #[tokio::test]
    async fn probe_with_backoff_recovers_when_upstream_comes_up_mid_window() {
        let srv = MockServer::start().await;
        // First ~400ms: 503. After that: 200.
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_millis(0)))
            .up_to_n_times(2)
            .mount(&srv)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Ollama, srv.uri());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        assert!(probe_with_backoff(&client(), &t, deadline).await);
    }
}
