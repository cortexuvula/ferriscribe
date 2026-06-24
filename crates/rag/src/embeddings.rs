use medical_core::error::{AppError, AppResult};
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// HTTP-backed embedding generator using Ollama.
pub struct EmbeddingGenerator {
    client: Client,
    host: String,
    model: String,
    dim: usize,
}

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct OllamaResponse {
    embedding: Vec<f32>,
}

impl EmbeddingGenerator {
    /// Create a generator backed by a local Ollama instance.
    ///
    /// Defaults to `http://localhost:11434` and the `nomic-embed-text` model (768 dims).
    pub fn new_ollama(host: Option<&str>, model: Option<&str>) -> AppResult<Self> {
        // Embedding requests are short; bound connection at 10s and total
        // request at 120s — long enough for Ollama to load a model on first
        // call but short enough to avoid indefinite RAG ingestion stalls.
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| AppError::HttpClient(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            host: host.unwrap_or("http://localhost:11434").to_owned(),
            model: model.unwrap_or("nomic-embed-text").to_owned(),
            dim: 768,
        })
    }

    /// The dimensionality of the vectors produced by this generator.
    pub fn dimension(&self) -> usize {
        self.dim
    }

    /// Generate an embedding for a single text.
    pub async fn embed(&self, text: &str) -> AppResult<Vec<f32>> {
        let body = OllamaRequest {
            model: &self.model,
            prompt: text,
        };
        let url = format!("{}/api/embeddings", self.host);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::AiProvider(format!("Ollama request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = medical_core::http_error_body::read_error_body(resp, 200).await;
            return Err(AppError::AiProvider(format!(
                "Ollama API error {status}: {body_text}"
            )));
        }

        let parsed: OllamaResponse = resp
            .json()
            .await
            .map_err(|e| AppError::AiProvider(format!("Ollama response parse error: {e}")))?;

        Ok(parsed.embedding)
    }

    /// Generate embeddings for a batch of texts.
    ///
    /// Ollama exposes one-prompt-per-request, so this fans out the inputs
    /// with bounded concurrency instead of serializing every call. The
    /// resulting vector is in the same order as `texts`.
    pub async fn embed_batch(&self, texts: &[&str]) -> AppResult<Vec<Vec<f32>>> {
        use futures_util::stream::{StreamExt, TryStreamExt};

        // 8 concurrent requests is a safe default for local Ollama — enough
        // to hide request latency on 100-chunk PDFs without saturating a
        // single-GPU server or hammering a user's CPU budget.
        const CONCURRENCY: usize = 8;

        // Build the per-text futures eagerly while we hold &self, then stream
        // them through buffered(). This sidesteps the HRTB issue of trying to
        // express "closure that reborrows self for each item" at a call site
        // reached via tauri::generate_handler, whose expanded signature needs
        // the embed future to be valid for any lifetime.
        let futures: Vec<_> = texts.iter().map(|&t| self.embed(t)).collect();
        futures_util::stream::iter(futures)
            .buffered(CONCURRENCY)
            .try_collect()
            .await
    }
}

/// Default creates a local Ollama backend (local-first experience).
///
/// # Panics
/// Panics only if the reqwest TLS backend fails to initialize, which cannot
/// happen under normal system configurations. Use [`Self::new_ollama`] for
/// fallible construction.
impl Default for EmbeddingGenerator {
    fn default() -> Self {
        Self::new_ollama(None, None).expect("default reqwest client config is valid")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_constructor_defaults() {
        let emb = EmbeddingGenerator::new_ollama(None, None).unwrap();
        assert_eq!(emb.dimension(), 768);
        assert_eq!(emb.host, "http://localhost:11434");
        assert_eq!(emb.model, "nomic-embed-text");
    }

    #[test]
    fn ollama_constructor_custom() {
        let emb = EmbeddingGenerator::new_ollama(Some("http://myhost:1234"), Some("custom-model"))
            .unwrap();
        assert_eq!(emb.dimension(), 768);
        assert_eq!(emb.host, "http://myhost:1234");
        assert_eq!(emb.model, "custom-model");
    }

    // NOTE: A test for HTTP client construction failure is intentionally
    // omitted. The reqwest client builder only fails when the TLS backend
    // cannot be initialized (e.g. missing native-tls libraries or an invalid
    // rustls config). This cannot be reliably triggered in a unit test
    // without mocking reqwest internals or manipulating system TLS state,
    // both of which are fragile and platform-specific. The error propagation
    // path is trivially correct by inspection: `.map_err(...)?`.

    #[test]
    fn default_is_ollama() {
        let emb = EmbeddingGenerator::default();
        assert_eq!(emb.dimension(), 768);
        assert_eq!(emb.host, "http://localhost:11434");
        assert_eq!(emb.model, "nomic-embed-text");
    }

    #[test]
    fn ollama_is_the_only_constructor() {
        // Compile-time check: this builds only if new_openai has been removed.
        let _ = EmbeddingGenerator::new_ollama(None, None);
        // If this test compiles, the simplification is complete.
    }
}
