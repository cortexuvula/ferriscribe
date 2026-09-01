//! Verified file downloads for runtime-fetched binaries and resources.
//!
//! Single home for the three download paths in the app (pdfium native
//! library, whisper-server binary, STT model files). Everything downloaded
//! here goes through a client with explicit timeouts — the default reqwest
//! client has none, so a stalled CDN otherwise hangs the caller forever.
//!
//! SHA-256 verification is optional at the API level (STT model URLs have no
//! published digest), but any download whose bytes are later executed or
//! dlopen'd (whisper binary, pdfium dylib) MUST pass a pinned hash — the
//! whisper supervisor refuses to download without one, and the pdfium path
//! pins digests per release asset.
//!
//! URLs may be logged; response bodies never are (they are binaries, and
//! the logging rule is lengths/IDs only anyway).

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Error from a verified download.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("HTTP {status} for {url}")]
    Status {
        status: reqwest::StatusCode,
        url: String,
    },
    #[error("network error fetching {url}: {source}")]
    Network { url: String, source: reqwest::Error },
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("sha256 mismatch: expected {expected}, got {got}")]
    HashMismatch { expected: String, got: String },
}

/// Connect timeout shared by both download flavors: fail fast when the host
/// is unreachable; the total timeout carries the actual transfer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Total timeout for in-memory downloads (small archives: pdfium ~3.5 MB,
/// whisper binaries ~tens of MB).
const BYTES_TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

/// Total timeout for streamed file downloads (STT models reach multiple GB;
/// a 5-minute cap would abort legitimate slow clinic links).
const FILE_TOTAL_TIMEOUT: Duration = Duration::from_secs(3600);

fn download_client(total_timeout: Duration) -> Result<reqwest::Client, DownloadError> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(total_timeout)
        .build()
        .map_err(|e| DownloadError::Network {
            url: String::new(),
            source: e,
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn verify_hash(bytes: &[u8], expected: &str) -> Result<(), DownloadError> {
    let got = sha256_hex(bytes);
    if got != expected {
        return Err(DownloadError::HashMismatch {
            expected: expected.to_string(),
            got,
        });
    }
    Ok(())
}

/// Download `url` fully into memory, verifying its SHA-256 when
/// `expected_sha256` is provided. Intended for small-to-medium archives
/// (the pdfium tgz, the whisper-server binary) whose bytes are then verified
/// and extracted. Callers that later execute or dlopen the payload must pass
/// a pinned hash.
pub async fn download_bytes(
    url: &str,
    expected_sha256: Option<&str>,
) -> Result<Vec<u8>, DownloadError> {
    let client = download_client(BYTES_TOTAL_TIMEOUT)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| DownloadError::Network {
            url: url.to_string(),
            source: e,
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(DownloadError::Status {
            status,
            url: url.to_string(),
        });
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| DownloadError::Network {
            url: url.to_string(),
            source: e,
        })?
        .to_vec();
    if let Some(expected) = expected_sha256 {
        verify_hash(&bytes, expected)?;
    }
    Ok(bytes)
}

/// Stream `url` to `dest`, computing the SHA-256 while writing and reporting
/// progress as `(downloaded_bytes, total_bytes)` after each chunk. `total` is
/// `0` when the server does not advertise a content length (model endpoints
/// often stream gzip-encoded without one). `dest` should be a staging path —
/// the atomic rename to the final name is the caller's concern.
///
/// On hash mismatch (or any error) the partially-written `dest` is removed.
pub async fn download_file<F>(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    on_progress: F,
) -> Result<(), DownloadError>
where
    F: Fn(u64, u64) + Send + 'static,
{
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let client = download_client(FILE_TOTAL_TIMEOUT)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| DownloadError::Network {
            url: url.to_string(),
            source: e,
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(DownloadError::Status {
            status,
            url: url.to_string(),
        });
    }
    let total = response.content_length().unwrap_or(0);

    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| DownloadError::Io {
            path: dest.to_path_buf(),
            source: e,
        })?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();
    let stream_err: Option<DownloadError> = loop {
        let chunk = match stream.next().await {
            Some(Ok(bytes)) => bytes,
            Some(Err(e)) => break Some(cleanup_err(dest, e, url).await),
            None => break None,
        };
        hasher.update(&chunk);
        if let Err(e) = file.write_all(&chunk).await {
            break Some(cleanup_io(dest, e).await);
        }
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    };
    if let Some(e) = stream_err {
        return Err(e);
    }
    if let Err(e) = file.flush().await {
        let _ = tokio::fs::remove_file(dest).await;
        return Err(DownloadError::Io {
            path: dest.to_path_buf(),
            source: e,
        });
    }
    if let Some(expected) = expected_sha256 {
        let got = hex::encode(hasher.finalize());
        if got != expected {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(DownloadError::HashMismatch {
                expected: expected.to_string(),
                got,
            });
        }
    }
    Ok(())
}

/// Remove the partial file and produce the network-error variant. Used as a
/// `break` value so the stream loop stays flat.
async fn cleanup_err(dest: &Path, e: reqwest::Error, url: &str) -> DownloadError {
    let _ = tokio::fs::remove_file(dest).await;
    DownloadError::Network {
        url: url.to_string(),
        source: e,
    }
}

async fn cleanup_io(dest: &Path, e: std::io::Error) -> DownloadError {
    let _ = tokio::fs::remove_file(dest).await;
    DownloadError::Io {
        path: dest.to_path_buf(),
        source: e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn download_bytes_verifies_hash() {
        let server = wiremock::MockServer::start().await;
        let body = b"hello pdfium".to_vec();
        let expected = sha256_hex(&body);
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;
        let bytes = download_bytes(&server.uri(), Some(&expected))
            .await
            .unwrap();
        assert_eq!(bytes, body);
    }

    #[tokio::test]
    async fn download_bytes_hash_mismatch() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes("tampered"))
            .mount(&server)
            .await;
        let err = download_bytes(&server.uri(), Some("deadbeef"))
            .await
            .unwrap_err();
        assert!(matches!(err, DownloadError::HashMismatch { .. }));
    }

    #[tokio::test]
    async fn download_bytes_http_error_is_status() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let err = download_bytes(&server.uri(), None).await.unwrap_err();
        assert!(matches!(err, DownloadError::Status { .. }), "got: {err:?}");
    }

    #[tokio::test]
    async fn download_file_streams_and_verifies() {
        let server = wiremock::MockServer::start().await;
        let body = b"model-bytes".to_vec();
        let expected = sha256_hex(&body);
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(body.clone())
                    .insert_header("content-length", "11"),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("downloaded.bin");
        let last = std::sync::Arc::new(std::sync::Mutex::new((0u64, 0u64)));
        let seen = std::sync::Arc::clone(&last);
        download_file(&server.uri(), &dest, Some(&expected), move |d, t| {
            *seen.lock().unwrap() = (d, t)
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(*last.lock().unwrap(), (11, 11));
    }

    #[tokio::test]
    async fn download_file_removes_partial_on_mismatch() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes("evil"))
            .mount(&server)
            .await;
        let dest = std::env::temp_dir().join("ferriscribe-net-test-mismatch");
        let err = download_file(&server.uri(), &dest, Some("deadbeef"), |_, _| {}).await;
        assert!(err.is_err());
        assert!(!dest.exists(), "partial file must be removed on failure");
    }
}
