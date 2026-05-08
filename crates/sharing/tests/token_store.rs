use medical_sharing::token_store::{TokenStore, TokenStoreError};
use tempfile::TempDir;

fn open(tmp: &TempDir) -> TokenStore {
    let key = [42u8; 32];
    TokenStore::open(tmp.path().join("tokens.db"), &key).expect("open")
}

#[test]
fn issue_and_validate_round_trip() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);

    let issued = store.issue("Dr. Smith's MacBook").unwrap();
    assert!(!issued.token.is_empty());
    assert!(issued.id > 0);

    let row = store.validate(&issued.token).unwrap().expect("valid");
    assert_eq!(row.label, "Dr. Smith's MacBook");
    assert!(row.revoked_at.is_none());
}

#[test]
fn unknown_token_returns_none() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);
    assert!(store.validate("does-not-exist").unwrap().is_none());
}

#[test]
fn revoked_token_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);
    let issued = store.issue("laptop").unwrap();
    store.revoke(issued.id).unwrap();
    assert!(store.validate(&issued.token).unwrap().is_none());
}

#[test]
fn touch_updates_last_seen() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);
    let issued = store.issue("x").unwrap();
    store.touch(issued.id).unwrap();
    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].last_seen_at.is_some());
}

#[test]
fn list_excludes_revoked_by_default() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);
    let a = store.issue("alive").unwrap();
    let b = store.issue("dead").unwrap();
    store.revoke(b.id).unwrap();
    let active = store.list().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, a.id);
}

#[test]
fn wrong_key_fails() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("t.db");
    {
        let s = TokenStore::open(&path, &[1u8; 32]).unwrap();
        s.issue("x").unwrap();
    }
    let err = TokenStore::open(&path, &[2u8; 32]).unwrap_err();
    assert!(matches!(err, TokenStoreError::Sqlite(_)));
}

#[test]
fn update_label_round_trips() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);
    let issued = store.issue("old name").unwrap();

    store.update_label(issued.id, "Dr. Patel — Room 4").unwrap();

    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].label, "Dr. Patel — Room 4");
}

#[test]
fn update_label_trims_whitespace() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);
    let issued = store.issue("x").unwrap();

    store.update_label(issued.id, "   spaced   ").unwrap();

    let listed = store.list().unwrap();
    assert_eq!(listed[0].label, "spaced");
}

#[test]
fn update_label_rejects_empty() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);
    let issued = store.issue("x").unwrap();

    let err = store.update_label(issued.id, "    ").unwrap_err();
    assert!(matches!(err, TokenStoreError::EmptyLabel));
}

#[test]
fn update_label_truncates_at_eighty_chars() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);
    let issued = store.issue("x").unwrap();

    let long = "a".repeat(200);
    store.update_label(issued.id, &long).unwrap();

    let listed = store.list().unwrap();
    assert_eq!(listed[0].label.chars().count(), 80);
}

#[test]
fn update_label_rejects_revoked() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);
    let issued = store.issue("x").unwrap();
    store.revoke(issued.id).unwrap();

    let err = store.update_label(issued.id, "new").unwrap_err();
    assert!(matches!(err, TokenStoreError::NotFound));
}

#[test]
fn update_label_rejects_unknown_id() {
    let tmp = TempDir::new().unwrap();
    let store = open(&tmp);

    let err = store.update_label(9999, "new").unwrap_err();
    assert!(matches!(err, TokenStoreError::NotFound));
}
