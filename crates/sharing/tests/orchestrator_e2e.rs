//! End-to-end smoke test for the sharing orchestrator's HTTP layer.
//!
//! Spins up the orchestrator pointing at fake Ollama / whisper backends,
//! pairs a "client", uses the issued bearer to call through both auth
//! proxies, and verifies the bodies forward correctly.
//!
//! mDNS, whisper.cpp child process, and persistent-service install are
//! NOT exercised — they have their own targeted coverage.

use std::sync::Arc;
use std::time::Duration;

use axum::{Router, routing::post};
use rand::RngCore;
use tempfile::TempDir;
use tokio::net::TcpListener;

async fn fake_ollama(port: u16) {
    let app = Router::new().route(
        "/api/chat",
        post(|body: String| async move { format!("ollama:{body}") }),
    );
    let l = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    axum::serve(l, app).await.unwrap();
}

async fn fake_whisper(port: u16, expected_api_key: String) {
    let app = Router::new().route(
        "/v1/audio/transcriptions",
        post(move |headers: axum::http::HeaderMap, body: String| {
            let auth = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            async move {
                if auth.as_deref() == Some(&format!("Bearer {expected_api_key}")) {
                    format!("whisper:{body}")
                } else {
                    String::from("auth-mismatch")
                }
            }
        }),
    );
    let l = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    axum::serve(l, app).await.unwrap();
}

async fn next_free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_and_proxy_round_trip() {
    use medical_sharing::auth_proxy::{ProxyConfig, spawn_auth_proxy};
    use medical_sharing::pairing::PairingState;
    use medical_sharing::token_store::TokenStore;

    let tmp = TempDir::new().unwrap();
    let mut key = [0u8; 32];
    rand::rng().fill_bytes(&mut key);

    // 1. Set up token store + pairing state.
    let store = Arc::new(TokenStore::open(tmp.path().join("tokens.db"), &key).unwrap());
    let pairing = Arc::new(PairingState::new(store.clone()));

    // 2. Issue a code, simulate enroll-side trade for a token.
    let code = pairing.issue_code().await;
    let token = pairing
        .enroll(&code, "test-laptop")
        .await
        .expect("enroll succeeds");
    assert!(!token.is_empty(), "enroll returned a token");

    // 3. Stand up fake Ollama + Whisper backends.
    let ollama_port = next_free_port().await;
    tokio::spawn(fake_ollama(ollama_port));

    let whisper_port = next_free_port().await;
    let whisper_api_key = "internal-shared-key".to_string();
    tokio::spawn(fake_whisper(whisper_port, whisper_api_key.clone()));

    // 4. Stand up the auth proxies in front of them.
    let ollama_proxy_port = next_free_port().await;
    let _h1 = spawn_auth_proxy(
        ProxyConfig {
            listen_port: ollama_proxy_port,
            backend_url: format!("http://127.0.0.1:{ollama_port}"),
            path_prefix: "/".to_string(),
            inject_api_key: None,
        },
        store.clone(),
    )
    .await
    .expect("ollama proxy binds");

    let whisper_proxy_port = next_free_port().await;
    let _h2 = spawn_auth_proxy(
        ProxyConfig {
            listen_port: whisper_proxy_port,
            backend_url: format!("http://127.0.0.1:{whisper_port}"),
            path_prefix: "/".to_string(),
            inject_api_key: Some(whisper_api_key.clone()),
        },
        store.clone(),
    )
    .await
    .expect("whisper proxy binds");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 5. Hit Ollama through the proxy with the bearer.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{ollama_proxy_port}/api/chat"))
        .bearer_auth(&token)
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ollama:hello");

    // 6. Hit Whisper through the proxy. The proxy strips our bearer and
    //    injects the shared `whisper_api_key`; the fake whisper backend
    //    asserts it sees that key.
    let resp = client
        .post(format!(
            "http://127.0.0.1:{whisper_proxy_port}/v1/audio/transcriptions"
        ))
        .bearer_auth(&token)
        .body("audio-bytes")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "whisper:audio-bytes");

    // 7. Revoke the client; subsequent calls should 401.
    let row = store.validate(&token).unwrap().expect("still valid");
    store.revoke(row.id).unwrap();

    let resp = client
        .post(format!("http://127.0.0.1:{ollama_proxy_port}/api/chat"))
        .bearer_auth(&token)
        .body("after-revoke")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "revoked bearer must be rejected");
}
