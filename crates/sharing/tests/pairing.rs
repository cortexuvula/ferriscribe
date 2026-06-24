use medical_sharing::pairing::{PairingState, generate_code};
use medical_sharing::token_store::TokenStore;
use std::sync::Arc;
use tempfile::TempDir;

fn store(tmp: &TempDir) -> Arc<TokenStore> {
    Arc::new(TokenStore::open(tmp.path().join("t.db"), &[7u8; 32]).unwrap())
}

#[test]
fn generated_code_is_six_digits() {
    let c = generate_code();
    assert_eq!(c.len(), 6);
    assert!(c.chars().all(|ch| ch.is_ascii_digit()));
}

#[tokio::test]
async fn enroll_consumes_code_once() {
    let tmp = TempDir::new().unwrap();
    let st = store(&tmp);
    let state = PairingState::new(st.clone());

    let code = state.issue_code().await;
    let token = state
        .enroll(&code, "laptop")
        .await
        .expect("first enroll succeeds");
    assert!(!token.is_empty());

    let err = state.enroll(&code, "another").await.unwrap_err();
    assert!(err.to_string().contains("invalid"));
}

#[tokio::test]
async fn expired_code_rejected() {
    let tmp = TempDir::new().unwrap();
    let st = store(&tmp);
    let state = PairingState::new(st.clone()).with_ttl(std::time::Duration::from_millis(50));

    let code = state.issue_code().await;
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;

    let err = state.enroll(&code, "x").await.unwrap_err();
    assert!(err.to_string().contains("expired") || err.to_string().contains("invalid"));
}
