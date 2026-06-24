use std::sync::Arc;
use std::time::Duration;

use axum::{Router, routing::post};
use medical_sharing::auth_proxy::{ProxyConfig, spawn_auth_proxy};
use medical_sharing::token_store::TokenStore;
use tempfile::TempDir;
use tokio::net::TcpListener;

async fn fake_backend(port: u16) {
    let app = Router::new().route(
        "/api/chat",
        post(|body: String| async move { format!("echo:{body}") }),
    );
    let l = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    axum::serve(l, app).await.unwrap();
}

async fn next_free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

#[tokio::test]
async fn missing_bearer_returns_401_with_reason_header() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(TokenStore::open(tmp.path().join("t.db"), &[3u8; 32]).unwrap());

    let backend_port = next_free_port().await;
    tokio::spawn(fake_backend(backend_port));

    let proxy_port = next_free_port().await;
    let cfg = ProxyConfig {
        listen_port: proxy_port,
        backend_url: format!("http://127.0.0.1:{backend_port}"),
        path_prefix: "/api".to_string(),
        inject_api_key: None,
    };
    spawn_auth_proxy(cfg, store.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{proxy_port}/api/chat"))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.headers()
            .get("x-auth-reason")
            .and_then(|v| v.to_str().ok()),
        Some("missing-bearer"),
        "401 from missing Authorization should be tagged"
    );
}

#[tokio::test]
async fn valid_bearer_forwards_body() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(TokenStore::open(tmp.path().join("t.db"), &[4u8; 32]).unwrap());
    let issued = store.issue("test-laptop").unwrap();

    let backend_port = next_free_port().await;
    tokio::spawn(fake_backend(backend_port));

    let proxy_port = next_free_port().await;
    let cfg = ProxyConfig {
        listen_port: proxy_port,
        backend_url: format!("http://127.0.0.1:{backend_port}"),
        path_prefix: "/api".to_string(),
        inject_api_key: None,
    };
    spawn_auth_proxy(cfg, store.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{proxy_port}/api/chat"))
        .bearer_auth(&issued.token)
        .body("ping")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "echo:ping");
}

#[tokio::test]
async fn unknown_or_revoked_bearer_returns_401_with_reason_header() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(TokenStore::open(tmp.path().join("t.db"), &[5u8; 32]).unwrap());
    let issued = store.issue("evil").unwrap();
    store.revoke(issued.id).unwrap();

    let backend_port = next_free_port().await;
    tokio::spawn(fake_backend(backend_port));

    let proxy_port = next_free_port().await;
    let cfg = ProxyConfig {
        listen_port: proxy_port,
        backend_url: format!("http://127.0.0.1:{backend_port}"),
        path_prefix: "/api".to_string(),
        inject_api_key: None,
    };
    spawn_auth_proxy(cfg, store.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{proxy_port}/api/chat"))
        .bearer_auth(&issued.token)
        .body("ping")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.headers()
            .get("x-auth-reason")
            .and_then(|v| v.to_str().ok()),
        Some("unknown-token"),
        "401 from revoked/unknown token should be tagged"
    );
}
