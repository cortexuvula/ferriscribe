//! Route-level tests for the sharing vocab API — the HTTP surface paired
//! clients drive against the machine holding the PHI database. These were
//! the app's least-tested code before 2026-08-25 (~1,900 lines, one test):
//! routing, bearer auth, payload validation, and the CRUD/merge handlers
//! are exercised here through the real axum router (`tower::ServiceExt::
//! oneshot`), with an in-memory database, a real `TokenStore` in a temp
//! dir, and tauri's `MockRuntime` app handle.
//!
//! Deliberately NOT covered here: SSE streams (`/events` endpoints need a
//! long-lived connection shape that oneshot can't express) and the audio
//! byte round-trip (encrypted-file plumbing) — both are covered end-to-end
//! by the medical-sharing integration tests from the client side.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use medical_db::Database;
use medical_sharing::token_store::TokenStore;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{ApiState, build_router};

struct TestApp {
    router: Router,
    token: String,
    tokens: Arc<TokenStore>,
    /// Keeps the mock app (event loop owner) and the temp token-store dir
    /// alive for the lifetime of the test.
    _app: tauri::App<tauri::test::MockRuntime>,
    _tmp: tempfile::TempDir,
}

async fn test_app() -> TestApp {
    let db = Arc::new(Database::open_in_memory().expect("in-memory db"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let tokens =
        Arc::new(TokenStore::open(tmp.path().join("tokens.db"), &[7u8; 32]).expect("token store"));
    let issued = tokens.issue("route-tests").expect("issue token");
    let tokens_for_state = Arc::clone(&tokens);
    let app = tauri::test::mock_app();
    let state = ApiState {
        db,
        tokens: tokens_for_state,
        chips_changed_tx: tokio::sync::broadcast::channel(16).0,
        dict_changed_tx: tokio::sync::broadcast::channel(16).0,
        content_changed_tx: tokio::sync::broadcast::channel(32).0,
        data_dir: tmp.path().to_path_buf(),
        app_handle: app.handle().clone(),
        merge_lock: Arc::new(tokio::sync::Mutex::new(())),
        fail_limiter: Arc::new(std::sync::Mutex::new(
            medical_security::rate_limiter::RateLimiter::new(5),
        )),
    };
    TestApp {
        router: build_router(state),
        token: issued.token,
        tokens,
        _app: app,
        _tmp: tmp,
    }
}

/// One-shot request through the real router. Returns (status, parsed body).
async fn req(
    app: &TestApp,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(b) = bearer {
        builder = builder.header("authorization", format!("Bearer {b}"));
    }
    let body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };
    let response = app
        .router
        .clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, json)
}

fn authed(app: &TestApp) -> Option<&str> {
    Some(app.token.as_str())
}

// ── Auth ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn auth_rejects_missing_and_wrong_bearer_on_every_route_group() {
    // Fresh app per route: authorize now consumes a shared fail-limiter
    // bucket on bad/missing bearers (5 slots), and 12 failures in one app
    // would trip 429s instead of the exact 401s asserted here.
    for uri in [
        "/v1/vocabulary",
        "/v1/context-templates",
        "/v1/user-dictionary",
        "/v1/condition-chips",
        "/v1/content/sync/meta",
        "/v1/content/audio/00000000-0000-0000-0000-000000000000",
    ] {
        let app = test_app().await;
        let (status, _) = req(&app, "GET", uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "no bearer on {uri}");
        let (status, _) = req(&app, "GET", uri, Some("forged-token"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "bad bearer on {uri}");
    }
}

#[tokio::test]
async fn auth_rate_limits_after_budget_exhausted() {
    let app = test_app().await;
    // 5-slot bucket: the first 5 forged attempts get honest 401s, the 6th
    // onward get 429 until the bucket refills.
    let mut saw_401 = 0;
    let mut saw_429 = false;
    for _ in 0..8 {
        let (status, _) = req(&app, "GET", "/v1/vocabulary", Some("forged-token"), None).await;
        match status {
            StatusCode::UNAUTHORIZED => saw_401 += 1,
            StatusCode::TOO_MANY_REQUESTS => saw_429 = true,
            other => panic!("expected 401 or 429, got {other} on /v1/vocabulary"),
        }
    }
    assert_eq!(saw_401, 5, "exactly the bucket size gets honest 401s");
    assert!(saw_429, "budget exhaustion must throttle");
}

#[tokio::test]
async fn auth_rejects_revoked_token() {
    let app = test_app().await;
    let (status, _) = req(&app, "GET", "/v1/vocabulary", authed(&app), None).await;
    assert_eq!(status, StatusCode::OK);

    // Revoke every client row; the previously valid token must now 401 —
    // pairing revocation is the PHI boundary for this whole API.
    for row in app.tokens.list().expect("list clients") {
        app.tokens.revoke(row.id).expect("revoke");
    }
    let (status, _) = req(&app, "GET", "/v1/vocabulary", authed(&app), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── Vocabulary ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn vocabulary_crud_roundtrip() {
    let app = test_app().await;

    // Insert.
    let (status, entry) = req(
        &app,
        "POST",
        "/v1/vocabulary",
        authed(&app),
        Some(json!({"find_text": "hte", "replacement": "the"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "insert: {entry}");
    let id = entry["id"].as_str().expect("id").to_string();

    // List contains it; count is (total, enabled).
    let (status, list) = req(&app, "GET", "/v1/vocabulary", authed(&app), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().expect("list").len(), 1);
    let (status, count) = req(&app, "GET", "/v1/vocabulary/count", authed(&app), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count, json!([1, 1]), "count = {count}");

    // Update by id.
    let (status, updated) = req(
        &app,
        "PUT",
        &format!("/v1/vocabulary/{id}"),
        authed(&app),
        Some(json!({"find_text": "hte", "replacement": "THE", "enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update: {updated}");
    assert_eq!(updated["replacement"], "THE");
    assert_eq!(updated["enabled"], false);

    // Malformed uuid → 400, not 500.
    let (status, _) = req(
        &app,
        "PUT",
        "/v1/vocabulary/not-a-uuid",
        authed(&app),
        Some(json!({"find_text": "x", "replacement": "y"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Delete one → 204, list empty again.
    let (status, _) = req(
        &app,
        "DELETE",
        &format!("/v1/vocabulary/{id}"),
        authed(&app),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, count) = req(&app, "GET", "/v1/vocabulary/count", authed(&app), None).await;
    assert_eq!(count, json!([0, 0]));
}

// ── Context templates ───────────────────────────────────────────────────────

#[tokio::test]
async fn templates_upsert_rename_delete() {
    let app = test_app().await;

    let (status, tpl) = req(
        &app,
        "POST",
        "/v1/context-templates/upsert",
        authed(&app),
        Some(json!({"name": "Cardio follow-up", "body": "Cardiac history: ..."})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "upsert: {tpl}");
    assert_eq!(tpl["name"], "Cardio follow-up");

    let (_, list) = req(&app, "GET", "/v1/context-templates", authed(&app), None).await;
    assert_eq!(list.as_array().expect("list").len(), 1);

    let (status, renamed) = req(
        &app,
        "POST",
        "/v1/context-templates/rename",
        authed(&app),
        Some(json!({"old_name": "Cardio follow-up", "new_name": "Cardio FU"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rename: {renamed}");
    assert_eq!(renamed["name"], "Cardio FU");

    let (status, _) = req(
        &app,
        "POST",
        "/v1/context-templates/delete",
        authed(&app),
        Some(json!({"name": "Cardio FU"})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Deleting a missing template errors (500 today — 'not found' maps to
    // the generic handler error), never silently 204s. Pin whichever it is
    // so a change is noticed.
    let (status, _) = req(
        &app,
        "POST",
        "/v1/context-templates/delete",
        authed(&app),
        Some(json!({"name": "missing"})),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

// ── User dictionary ─────────────────────────────────────────────────────────

#[tokio::test]
async fn dictionary_add_list_remove_roundtrip() {
    let app = test_app().await;

    let (status, added) = req(
        &app,
        "POST",
        "/v1/user-dictionary",
        authed(&app),
        Some(json!({"word": "metformin"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(added, json!(true));

    // Duplicate add is a no-op, not an error.
    let (status, added_again) = req(
        &app,
        "POST",
        "/v1/user-dictionary",
        authed(&app),
        Some(json!({"word": "metformin"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(added_again, json!(false));

    let (_, words) = req(&app, "GET", "/v1/user-dictionary", authed(&app), None).await;
    assert_eq!(words, json!(["metformin"]));

    let (status, removed) = req(
        &app,
        "DELETE",
        "/v1/user-dictionary/metformin",
        authed(&app),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed, json!(true));

    let (status, removed_again) = req(
        &app,
        "DELETE",
        "/v1/user-dictionary/metformin",
        authed(&app),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed_again, json!(false));

    let (_, words) = req(&app, "GET", "/v1/user-dictionary", authed(&app), None).await;
    assert_eq!(words, json!([]));
}

// ── Condition chips ─────────────────────────────────────────────────────────

#[tokio::test]
async fn condition_chips_sync_merges_and_returns_full_list() {
    let app = test_app().await;

    // Client pushes one active chip; server merges and returns the full
    // list (active + tombstones) per the sync contract.
    let chip = json!({
        "id": "hypertension",
        "text": "Hypertension",
        "updated_at": "2026-08-25T00:00:00Z",
        "deleted_at": null,
        "sort_order": 0,
        "use_count": 0,
    });
    let (status, merged) = req(
        &app,
        "POST",
        "/v1/condition-chips/sync",
        authed(&app),
        Some(json!([chip])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "sync: {merged}");
    let arr = merged.as_array().expect("merged list");
    assert!(arr.iter().any(|c| c["id"] == "hypertension"));

    // The list endpoint returns the same full view.
    let (status, list) = req(&app, "GET", "/v1/condition-chips", authed(&app), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().expect("list").len(), arr.len());

    // A tombstone pushed by another client must propagate: same id with a
    // newer timestamp and deleted_at set. The GET contract is the FULL list
    // (active + tombstones) — clients derive the active view locally via
    // merge_incoming, so deletions travel through this endpoint too.
    let tombstone = json!({
        "id": "hypertension",
        "text": "Hypertension",
        "updated_at": "2026-08-26T00:00:00Z",
        "deleted_at": "2026-08-26T00:00:00Z",
        "sort_order": 0,
        "use_count": 0,
    });
    let (status, _) = req(
        &app,
        "POST",
        "/v1/condition-chips/sync",
        authed(&app),
        Some(json!([tombstone])),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = req(&app, "GET", "/v1/condition-chips", authed(&app), None).await;
    let stored = list
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "hypertension")
        .expect("tombstone must survive in the full list");
    assert!(
        stored["deleted_at"].is_string(),
        "tombstoned chip must carry deleted_at: {stored}"
    );
}
